//! Formatter execution and provider selection for explicit formatting and saves.
use std::fmt::Display;

use command_line::Token;
use editor_core::{indent::IndentStyle, Rope, Transaction};
use futures_util::{future::BoxFuture, FutureExt};
use language_config::LanguageServerFeature;
use lsp_client::lsp;

use crate::{
    expansion,
    features::process::{self, Error as ProcessError},
    Document, Editor, ViewId,
};

impl Document {
    /// The same as [`Self::format`], but only returns formatting changes if auto-formatting
    /// is configured.
    pub fn auto_format(
        &self,
        editor: &Editor,
    ) -> Option<BoxFuture<'static, Result<Transaction, FormatterError>>> {
        self.auto_format_for_view(editor, self.focused_view(editor))
    }

    pub(crate) fn auto_format_for_view(
        &self,
        editor: &Editor,
        origin: Option<ViewId>,
    ) -> Option<BoxFuture<'static, Result<Transaction, FormatterError>>> {
        if self
            .language_config()?
            .auto_format
            .unwrap_or(editor.config().format.on_save)
        {
            self.format_snapshot(editor, self.snapshot(), origin)
        } else {
            None
        }
    }

    /// Compute edits using the configured formatter or the first capable language server.
    pub fn format(
        &self,
        editor: &Editor,
    ) -> Option<BoxFuture<'static, Result<Transaction, FormatterError>>> {
        self.format_snapshot(editor, self.snapshot(), self.focused_view(editor))
    }

    fn focused_view(&self, editor: &Editor) -> Option<ViewId> {
        editor
            .tree
            .try_get(editor.tree.focus())
            .filter(|view| view.doc == self.id())
            .map(|view| view.id)
    }

    pub(crate) fn format_snapshot(
        &self,
        editor: &Editor,
        snapshot: buffer::Snapshot,
        origin: Option<ViewId>,
    ) -> Option<BoxFuture<'static, Result<Transaction, FormatterError>>> {
        let text = snapshot.text().clone();
        if let Some((fmt_cmd, fmt_args, timeout)) = self
            .language_config()
            .and_then(|c| c.formatter.as_ref())
            .and_then(|formatter| {
                Some((
                    stdx::env::which(&formatter.command).ok()?,
                    &formatter.args,
                    formatter.timeout,
                ))
            })
        {
            log::debug!(
                "formatting '{}' with command '{}', args {fmt_args:?}",
                self.display_name(),
                fmt_cmd.display(),
            );

            let mut process = tokio::process::Command::new(&fmt_cmd);

            // Keep project-relative formatter arguments and shell expansions in the
            // same workspace. Documents without a workspace use their parent directory.
            let directory = self.workspace_root().to_path_buf();
            process.current_dir(&directory);

            let expansion = expansion::Context::capture_document(
                editor,
                self,
                origin.and_then(|view| self.try_selection(view)),
                directory,
                &fmt_args.join(" "),
            );
            let fmt_args = fmt_args.clone();
            let formatting_future = async move {
                let expansion = expansion
                    .map_err(|error| FormatterError::ExpansionFailed(error.to_string()))?;
                for content in &fmt_args {
                    let arg = expansion
                        .expand(Token::expand(content), &[])
                        .await
                        .map_err(|error| FormatterError::ExpansionFailed(error.to_string()))?;
                    process.arg(arg.as_ref());
                }
                let output = process::output(process, Some(&text))
                    .await
                    .map_err(|error| match error {
                        ProcessError::Spawn(error) => FormatterError::SpawningFailed {
                            command: fmt_cmd.to_string_lossy().into(),
                            error: error.kind(),
                        },
                        ProcessError::Input(_) => FormatterError::BrokenStdin,
                        ProcessError::Wait(_) => FormatterError::WaitForOutputFailed,
                    })?;

                if !output.status.success() {
                    if !output.stderr.is_empty() {
                        let err = String::from_utf8_lossy(&output.stderr).to_string();
                        log::error!("Formatter error: {}", err);
                        return Err(FormatterError::NonZeroExitStatus(Some(err)));
                    }

                    return Err(FormatterError::NonZeroExitStatus(None));
                } else if !output.stderr.is_empty() {
                    log::debug!(
                        "Formatter printed to stderr: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                }

                let str = std::str::from_utf8(&output.stdout)
                    .map_err(|_| FormatterError::InvalidUtf8Output)?;

                Ok(text_diff::compare_ropes(&text, &Rope::from(str)))
            };
            return Some(
                async move {
                    tokio::time::timeout(std::time::Duration::from_secs(timeout), formatting_future)
                        .await
                        .map_err(|_| FormatterError::Timeout(timeout))?
                }
                .boxed(),
            );
        };

        // finds first language server that supports formatting and then formats
        let language_server = self
            .language_servers_with_feature(LanguageServerFeature::Format)
            .next()?;
        let offset_encoding = language_server.offset_encoding();
        let request = language_server.text_document_formatting(
            self.identifier(),
            lsp::FormattingOptions {
                tab_size: self.indent_width() as u32,
                insert_spaces: matches!(self.indent_style, IndentStyle::Spaces(_)),
                ..Default::default()
            },
            None,
        )?;

        let fut = async move {
            let edits = request
                .await
                .unwrap_or_else(|e| {
                    log::warn!("LSP formatting failed: {}", e);
                    Default::default()
                })
                .unwrap_or_default();
            crate::language_servers::edits::generate_transaction_from_edits(
                &text,
                edits,
                offset_encoding,
            )
            .map_err(FormatterError::InvalidEdits)
        };
        Some(fut.boxed())
    }
}

#[derive(Clone, Debug)]
pub enum FormatterError {
    ExpansionFailed(String),
    InvalidEdits(crate::language_servers::edits::TextEditError),
    SpawningFailed {
        command: String,
        error: std::io::ErrorKind,
    },
    Timeout(u64),
    BrokenStdin,
    WaitForOutputFailed,
    InvalidUtf8Output,
    NonZeroExitStatus(Option<String>),
}

impl std::error::Error for FormatterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidEdits(error) => Some(error),
            _ => None,
        }
    }
}

impl Display for FormatterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExpansionFailed(error) => {
                write!(f, "Failed to expand formatter arguments: {error}")
            }
            Self::InvalidEdits(error) => write!(f, "Invalid formatter edits: {error}"),
            Self::SpawningFailed { command, error } => {
                write!(f, "Failed to spawn formatter {}: {:?}", command, error)
            }
            Self::Timeout(seconds) => write!(f, "Formatter timed out after {seconds} seconds"),
            Self::BrokenStdin => write!(f, "Could not write to formatter stdin"),
            Self::WaitForOutputFailed => write!(f, "Waiting for formatter output failed"),
            Self::InvalidUtf8Output => write!(f, "Invalid UTF-8 formatter output"),
            Self::NonZeroExitStatus(Some(output)) => write!(f, "Formatter error: {}", output),
            Self::NonZeroExitStatus(None) => {
                write!(f, "Formatter exited with non zero exit status")
            }
        }
    }
}
