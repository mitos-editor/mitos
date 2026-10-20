use futures_util::{future::BoxFuture, FutureExt};
use std::{borrow::Cow, collections::HashMap, path::PathBuf};

use command_line::{ExpansionKind, Token, TokenKind, Tokenizer};

use anyhow::{anyhow, ensure, Result};

use crate::Editor;

/// Variables that can be expanded in the command mode (`:`) via the expansion syntax.
///
/// For example `%{cursor_line}`.
//
// To add a new variable follow these steps:
//
// * Add the new enum member to `Variable` below.
// * Add an item to the `VARIANTS` constant - this enables completion.
// * Add a branch in `Variable::as_str`, converting the name from TitleCase to snake_case.
// * Add a branch in `Variable::from_name` with the reverse association.
// * Add a branch in the `expand_variable` function to read the value from the editor.
// * Add the new variable to the documentation in `book/src/command-line.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variable {
    /// The one-indexed line number of the primary cursor in the currently focused document.
    CursorLine,
    /// The one-indexed column number of the primary cursor in the currently focused document.
    ///
    /// Note that this is the count of grapheme clusters from the start of the line (regardless of
    /// softwrap) - the same as the `position` element in the statusline.
    CursorColumn,
    /// The display name of the currently focused document.
    ///
    /// This corresponds to `crate::Document::display_name`.
    BufferName,
    /// The absolute path of the currently focused document. For scratch buffers this will default
    /// to the current working directory.
    FilePathAbsolute,
    /// A string containing the line-ending of the currently focused document.
    LineEnding,
    /// Curreng working directory
    CurrentWorkingDirectory,
    /// Nearest ancestor directory of the current working directory that contains `.git`, `.svn`, `jj` or `.mitos`
    WorkspaceDirectory,
    // The name of current buffers language as set in `languages.toml`
    Language,
    // Primary selection
    Selection,
    // The one-indexed line number of the start of the primary selection in the currently focused document.
    SelectionLineStart,
    // The one-indexed line number of the end of the primary selection in the currently focused document.
    SelectionLineEnd,
}

impl Variable {
    pub const VARIANTS: &'static [Self] = &[
        Self::CursorLine,
        Self::CursorColumn,
        Self::BufferName,
        Self::FilePathAbsolute,
        Self::LineEnding,
        Self::CurrentWorkingDirectory,
        Self::WorkspaceDirectory,
        Self::Language,
        Self::Selection,
        Self::SelectionLineStart,
        Self::SelectionLineEnd,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::CursorLine => "cursor_line",
            Self::CursorColumn => "cursor_column",
            Self::BufferName => "buffer_name",
            Self::FilePathAbsolute => "file_path_absolute",
            Self::LineEnding => "line_ending",
            Self::CurrentWorkingDirectory => "current_working_directory",
            Self::WorkspaceDirectory => "workspace_directory",
            Self::Language => "language",
            Self::Selection => "selection",
            Self::SelectionLineStart => "selection_line_start",
            Self::SelectionLineEnd => "selection_line_end",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "cursor_line" => Some(Self::CursorLine),
            "cursor_column" => Some(Self::CursorColumn),
            "buffer_name" => Some(Self::BufferName),
            "file_path_absolute" => Some(Self::FilePathAbsolute),
            "line_ending" => Some(Self::LineEnding),
            "workspace_directory" => Some(Self::WorkspaceDirectory),
            "current_working_directory" => Some(Self::CurrentWorkingDirectory),
            "language" => Some(Self::Language),
            "selection" => Some(Self::Selection),
            "selection_line_start" => Some(Self::SelectionLineStart),
            "selection_line_end" => Some(Self::SelectionLineEnd),
            _ => None,
        }
    }
}

/// Editor values captured before asynchronous command evaluation.
/// Only referenced variables and registers are read; shell output is never expanded again.
pub struct Context {
    variables: HashMap<Variable, Cow<'static, str>>,
    registers: HashMap<char, Cow<'static, str>>,
    shell: Vec<String>,
    directory: PathBuf,
}

impl Context {
    pub fn capture(editor: &Editor, source: &str) -> Self {
        let mut context = Self {
            variables: HashMap::new(),
            registers: HashMap::new(),
            shell: editor.config().shell.clone(),
            directory: stdx::env::current_working_dir(),
        };
        context.capture_inner(editor, source);
        context
    }

    // Scan raw input too: signatures may consume the remainder as a single expanded token.
    // Ignore syntax errors here; the actual parser reports them with command context.
    fn capture_inner(&mut self, editor: &Editor, source: &str) {
        let mut start = 0;
        while let Some(offset) = source[start..].find('%') {
            let index = start + offset;
            if source.as_bytes().get(index + 1) == Some(&b'%') {
                start = index + 2;
                continue;
            }
            let mut parser = Tokenizer::new(&source[index..], true);
            match parser.parse_percent_token() {
                Some(Ok(token)) => {
                    match token.kind {
                        TokenKind::Expansion(ExpansionKind::Variable) => {
                            if let Some(variable) = Variable::from_name(&token.content) {
                                self.variables
                                    .entry(variable)
                                    .or_insert_with(|| expand_variable(editor, variable));
                            }
                        }
                        TokenKind::Expansion(ExpansionKind::Register) => {
                            if let Ok(name) = register_name(&token.content) {
                                self.registers.entry(name).or_insert_with(|| {
                                    editor
                                        .read_register(name)
                                        .map(|values| Cow::Owned(values.collect()))
                                        .unwrap_or_default()
                                });
                            }
                        }
                        TokenKind::Expansion(ExpansionKind::Shell) | TokenKind::Expand => {
                            self.capture_inner(editor, &token.content)
                        }
                        _ => {}
                    }
                    start = index + parser.pos();
                }
                _ => start = index + 1,
            }
        }
    }

    /// Evaluate using captured editor inputs. Dropping this future kills an active shell.
    pub fn expand<'a>(
        &'a self,
        token: Token<'a>,
        positional_args: &'a [Cow<'a, str>],
    ) -> BoxFuture<'a, Result<Cow<'a, str>>> {
        async move {
            // Note: see the `TokenKind` documentation for more details on how each branch should expand.
            match token.kind {
                TokenKind::Unquoted | TokenKind::Quoted(_) => Ok(token.content),
                TokenKind::Expansion(ExpansionKind::Variable) => {
                    let var = Variable::from_name(&token.content)
                        .ok_or_else(|| anyhow!("unknown variable '{}'", token.content))?;

                    self.variables
                        .get(&var)
                        .cloned()
                        .ok_or_else(|| anyhow!("variable '{}' was not captured", var.as_str()))
                }
                TokenKind::Expansion(ExpansionKind::Unicode) => {
                    if let Some(ch) = u32::from_str_radix(token.content.as_ref(), 16)
                        .ok()
                        .and_then(char::from_u32)
                    {
                        Ok(Cow::Owned(ch.to_string()))
                    } else {
                        Err(anyhow!(
                            "could not interpret '{}' as a Unicode character code",
                            token.content
                        ))
                    }
                }
                TokenKind::Expand => self.expand_inner(token.content, positional_args).await,
                TokenKind::Expansion(ExpansionKind::Shell) => {
                    self.expand_shell(token.content, positional_args).await
                }
                TokenKind::Expansion(ExpansionKind::Register) => {
                    let name = register_name(&token.content)?;
                    self.registers
                        .get(&name)
                        .cloned()
                        .ok_or_else(|| anyhow!("register '{name}' was not captured"))
                }
                TokenKind::Expansion(ExpansionKind::Arg) => {
                    expand_arg(&token.content, positional_args)
                }
                // Note: see the docs for this variant.
                TokenKind::ExpansionKind => unreachable!(
            "expansion name tokens cannot be emitted when command line validation is enabled"
        ),
            }
        }
        .boxed()
    }
}

/// Expands only positional-argument placeholders in a token.
///
/// Prompt updates use this path so potentially expensive or side-effecting expansions are only
/// evaluated when the command is submitted.
pub fn expand_only_arg<'a>(
    token: Token<'a>,
    positional_args: &[Cow<'a, str>],
) -> Result<Cow<'a, str>> {
    match token.kind {
        TokenKind::Expansion(ExpansionKind::Arg) => expand_arg(&token.content, positional_args),
        _ => Ok(token.content),
    }
}

fn expand_arg<'a>(
    content: &Cow<'a, str>,
    positional_args: &[Cow<'a, str>],
) -> Result<Cow<'a, str>> {
    let index = content
        .parse::<usize>()
        .map_err(|_| anyhow!("invalid positional argument index '{content}'"))?;
    Ok(positional_args.get(index).cloned().unwrap_or_default())
}

impl Context {
    async fn expand_shell<'a>(
        &self,
        content: Cow<'a, str>,
        positional_args: &[Cow<'a, str>],
    ) -> Result<Cow<'a, str>> {
        let content = self.expand_inner(content, positional_args).await?;
        ensure!(!self.shell.is_empty(), "No shell set");
        let mut process = tokio::process::Command::new(&self.shell[0]);
        process
            .args(&self.shell[1..])
            .arg(content.as_ref())
            .current_dir(&self.directory);
        let output = crate::features::process::output(process, None).await?;
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        if !output.stderr.is_empty() {
            log::warn!(
                "Shell expansion command `{content}` failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        if text.ends_with('\n') {
            text.pop();
            if text.ends_with('\r') {
                text.pop();
            }
        }
        Ok(Cow::Owned(text))
    }
}

fn register_name(content: &str) -> Result<char> {
    let mut chars = content.chars();
    let name = chars
        .next()
        .ok_or_else(|| anyhow!("Missing register name"))?;
    ensure!(
        chars.next().is_none(),
        "Invalid register `{content}`: should only be a single character"
    );
    Ok(name)
}

impl Context {
    /// Expand a token's contents recursively.
    async fn expand_inner<'a>(
        &self,
        content: Cow<'a, str>,
        positional_args: &[Cow<'a, str>],
    ) -> Result<Cow<'a, str>> {
        let mut escaped = String::new();
        let mut start = 0;
        let mut changed = false;

        while let Some(offset) = content[start..].find('%') {
            changed = true;
            let idx = start + offset;
            if content.as_bytes().get(idx + '%'.len_utf8()).copied() == Some(b'%') {
                // Treat two percents in a row as an escaped percent.
                escaped.push_str(&content[start..=idx]);
                // Skip over both percents.
                start = idx + ('%'.len_utf8() * 2);
            } else {
                // Otherwise interpret the percent as an expansion. Push up to (but not
                // including) the percent token.
                escaped.push_str(&content[start..idx]);
                // Then parse the expansion,
                let mut tokenizer = Tokenizer::new(&content[idx..], true);
                let token = tokenizer
                    .parse_percent_token()
                    .unwrap()
                    .map_err(|err| anyhow!("{err}"))?;
                // expand it (this is the recursive part),
                let expanded = self.expand(token, positional_args).await?;
                escaped.push_str(expanded.as_ref());
                // and move forward to the end of the expansion.
                start = idx + tokenizer.pos();
            }
        }

        if !changed {
            Ok(content)
        } else {
            escaped.push_str(&content[start..]);
            Ok(Cow::Owned(escaped))
        }
    }
}

// Note: the lifetime of the expanded variable (the `Cow`) must not be tied to the lifetime of
// the borrow of `Editor`. That would prevent commands from mutating the `Editor` until the
// command consumed or cloned all arguments - this is poor ergonomics. A sensible thing for this
// function to return then, instead, would normally be a `String`. We can return some statically
// known strings like the scratch buffer name or line ending strings though, so this function
// returns a `Cow<'static, str>` instead.
fn expand_variable(editor: &Editor, variable: Variable) -> Cow<'static, str> {
    let (view, doc) = current_ref!(editor);
    let text = doc.text().slice(..);

    match variable {
        Variable::CursorLine => {
            let cursor_line = doc.selection(view.id).primary().cursor_line(text);
            Cow::Owned((cursor_line + 1).to_string())
        }
        Variable::CursorColumn => {
            let cursor = doc.selection(view.id).primary().cursor(text);
            let position = editor_core::coords_at_pos(text, cursor);
            Cow::Owned((position.col + 1).to_string())
        }
        Variable::BufferName => {
            // Note: usually we would use `Document::display_name` but we can statically borrow
            // the scratch buffer name by partially reimplementing `display_name`.
            if let Some(path) = doc.display_path() {
                Cow::Owned(path.to_string_lossy().into_owned())
            } else {
                Cow::Borrowed(crate::document::SCRATCH_BUFFER_NAME)
            }
        }
        Variable::FilePathAbsolute => {
            let path = match doc.path() {
                Some(path) => path.to_owned(),
                None => stdx::env::current_working_dir(),
            }
            .to_string_lossy()
            .to_string();
            Cow::Owned(path)
        }
        Variable::LineEnding => Cow::Borrowed(doc.line_ending.as_str()),
        Variable::CurrentWorkingDirectory => std::borrow::Cow::Owned(
            stdx::env::current_working_dir()
                .to_string_lossy()
                .to_string(),
        ),
        Variable::WorkspaceDirectory => {
            std::borrow::Cow::Owned(workspace::find_workspace().0.to_string_lossy().to_string())
        }
        Variable::Language => match doc.language_name() {
            Some(lang) => Cow::Owned(lang.to_owned()),
            None => Cow::Borrowed("text"),
        },
        Variable::Selection => {
            Cow::Owned(doc.selection(view.id).primary().fragment(text).to_string())
        }
        Variable::SelectionLineStart => {
            let start_line = doc.selection(view.id).primary().line_range(text).0;
            Cow::Owned((start_line + 1).to_string())
        }
        Variable::SelectionLineEnd => {
            let end_line = doc.selection(view.id).primary().line_range(text).1;
            Cow::Owned((end_line + 1).to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_positional_arguments() {
        let args = [Cow::Borrowed("first"), Cow::Borrowed("second")];

        assert_eq!(expand_arg(&Cow::Borrowed("1"), &args).unwrap(), "second");
        assert_eq!(expand_arg(&Cow::Borrowed("2"), &args).unwrap(), "");
        assert!(expand_arg(&Cow::Borrowed("invalid"), &args).is_err());
    }
}
