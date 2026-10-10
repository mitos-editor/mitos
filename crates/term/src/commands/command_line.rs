//! Terminal command prompt, dispatch, help, and completion.

use std::{borrow::Cow, collections::HashMap, fmt::Write, ops};

use ::command_line::{self, Args, Flag, Signature, Token, TokenKind};
use anyhow::anyhow;
use editor_core::fuzzy::fuzzy_match;
use tui::text::Span;
use view::{
    custom_commands::{CustomCommand, CustomCommands},
    expansion, Editor,
};

use super::{
    catalog::{CommandCompleter, TypableCommand, TYPABLE_COMMAND_LIST, TYPABLE_COMMAND_MAP},
    context::{CommandCompletion, CommandInvocation, Context},
    mappable::MappableCommand,
};
use crate::{
    compositor,
    events::CommandOrigin,
    ui::{self, completers::Completer, Prompt, PromptEvent},
};

pub(super) struct CommandLineExecution {
    pub callbacks: Vec<compositor::Callback>,
    pub error: Option<String>,
}

impl CommandLineExecution {
    pub fn finish(mut self, editor: &mut Editor) -> Vec<compositor::Callback> {
        if let Some(error) = self.error {
            editor.set_error(|| error.clone());
            if !self.callbacks.is_empty() {
                // Earlier custom-command callbacks still run if a later child
                // fails. Keep that final failure visible after those callbacks.
                self.callbacks.push(Box::new(move |_, cx| {
                    cx.editor.set_error(|| error);
                }));
            }
        }
        self.callbacks
    }
}

pub(super) fn execute_command_line_with_invocation(
    cx: &mut compositor::Context,
    input: &str,
    event: PromptEvent,
    invocation: CommandInvocation,
) -> CommandLineExecution {
    let mut callbacks = Vec::new();
    let result = execute_command_line(cx, input, event, invocation, &mut callbacks);
    CommandLineExecution {
        callbacks,
        error: result.err().map(|error| error.to_string()),
    }
}

fn execute_command_line(
    cx: &mut compositor::Context,
    input: &str,
    event: PromptEvent,
    invocation: CommandInvocation,
    callbacks: &mut Vec<compositor::Callback>,
) -> anyhow::Result<()> {
    let (command, args, _) = command_line::split(input);
    if command.is_empty() {
        return Ok(());
    }

    let (escaped, command) = command
        .strip_prefix(CustomCommand::ESCAPE)
        .map_or((false, command), |command| (true, command));

    if !escaped {
        let custom_commands = cx.editor.config().commands.clone();
        if let Some(custom) = custom_commands.get(command) {
            let positional_args =
                Args::parse(args, Signature::DEFAULT, false, |token| Ok(token.content))
                    .expect("argument parsing cannot fail when validation is disabled");
            let mut invocation = invocation;
            invocation.origin = CommandOrigin::Custom;
            invocation.custom_command = Some(command.to_owned());

            for configured in &custom.commands {
                if let Some(typable) = configured.strip_prefix(':') {
                    let (name, args, _) = command_line::split(typable);
                    let name = name.strip_prefix(CustomCommand::ESCAPE).unwrap_or(name);
                    execute_named_command(
                        cx,
                        name,
                        args,
                        &positional_args,
                        event,
                        invocation.clone(),
                    )?;
                } else if event == PromptEvent::Validate {
                    let command: MappableCommand = configured.parse().map_err(|error| {
                        if cx.jobs.should_track_command(cx.editor) {
                            CommandCompletion::new(cx.editor, configured, invocation.clone())
                                .finish(cx.editor, Some(format!("{error}")), false);
                        }
                        error
                    })?;
                    let mut command_cx = super::Context {
                        config: cx.config,
                        register: invocation.register,
                        count: invocation.count.and_then(std::num::NonZeroUsize::new),
                        editor: cx.editor,
                        callback: Vec::new(),
                        on_next_key_callback: None,
                        jobs: cx.jobs,
                    };
                    command.execute_with_invocation(&mut command_cx, invocation.clone());
                    callbacks.extend(command_cx.callback);
                    if let Some(callback) = command_cx.on_next_key_callback {
                        callbacks.push(Box::new(move |compositor, _| {
                            if let Some(editor) = compositor.find::<ui::EditorView>() {
                                editor.set_next_key_callback(callback);
                            }
                        }));
                    }
                }
            }
            return Ok(());
        }
    }

    // If command is numeric, interpret as line number and go there.
    if command.parse::<usize>().is_ok() && args.trim().is_empty() {
        let cmd = TYPABLE_COMMAND_MAP.get("goto").unwrap();
        return execute_command(cx, cmd, command, &Args::empty(), event, invocation);
    }

    execute_named_command(cx, command, args, &Args::empty(), event, invocation)
}

fn execute_named_command(
    cx: &mut compositor::Context,
    command: &str,
    args: &str,
    positional_args: &Args,
    event: PromptEvent,
    invocation: CommandInvocation,
) -> anyhow::Result<()> {
    if let Some(cmd) = TYPABLE_COMMAND_MAP.get(command) {
        return execute_command(cx, cmd, args, positional_args, event, invocation);
    }
    if event != PromptEvent::Validate {
        return Ok(());
    }
    let completion = cx.jobs.should_track_command(cx.editor).then(|| {
        let mut completion = CommandCompletion::new(cx.editor, command, invocation);
        completion.raw_args = args.to_owned();
        cx.jobs.begin_command(cx.editor, completion)
    });
    let scope = completion
        .as_ref()
        .map(|completion| cx.jobs.enter_command(cx.editor, completion.clone()));
    let result = (|| {
        if let Some(specification) = cx.editor.plugin_command_arguments(command) {
            // Plugin signatures declare only positional values. Use the native
            // end-of-flags marker so literals such as -1 and -- are forwarded.
            let positional_input = format!("-- {args}");
            let args = Args::parse(
                &positional_input,
                plugin_signature(&specification),
                true,
                |token| {
                    expansion::expand(cx.editor, token, positional_args.as_slice())
                        .map_err(|err| err.into())
                },
            )
            .map_err(|err| anyhow!("'{command}': {err}"))?;
            let args = args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
            if let Some(completion) = &completion {
                completion.metadata(|completion| completion.args = args.clone());
            }
            if cx
                .editor
                .execute_plugin_command(command, args)
                .map_err(|err| anyhow!("'{command}': {err:#}"))?
            {
                return Ok(());
            }
        }
        Err(anyhow!("no such command: '{command}'"))
    })();
    if let Some(completion) = &completion {
        completion.capture_effects(cx.editor);
    }
    if let Some(scope) = scope {
        cx.jobs.leave_command(cx.editor, scope);
    }
    if let Some(completion) = completion {
        completion.finish_dispatch(
            cx.editor,
            result.as_ref().err().map(ToString::to_string),
            false,
        );
    }
    result
}

fn execute_command(
    cx: &mut compositor::Context,
    cmd: &TypableCommand,
    raw_args: &str,
    positional_args: &Args,
    event: PromptEvent,
    invocation: CommandInvocation,
) -> anyhow::Result<()> {
    let completion = (event == PromptEvent::Validate && cx.jobs.should_track_command(cx.editor))
        .then(|| {
            let mut completion = CommandCompletion::new(cx.editor, cmd.name, invocation);
            completion.raw_args = raw_args.to_owned();
            cx.jobs.begin_command(cx.editor, completion)
        });
    let scope = completion
        .as_ref()
        .map(|completion| cx.jobs.enter_command(cx.editor, completion.clone()));
    let result = (|| {
        let args = if event == PromptEvent::Validate {
            Args::parse(raw_args, cmd.signature, true, |token| {
                expansion::expand(cx.editor, token, positional_args.as_slice())
                    .map_err(|err| err.into())
            })
            .map_err(|err| anyhow!("'{}': {err}", cmd.name))?
        } else {
            Args::parse(raw_args, cmd.signature, false, |token| {
                expansion::expand_only_arg(token, positional_args.as_slice())
                    .map_err(|err| err.into())
            })
            .map_err(|err| anyhow!("'{}': {err}", cmd.name))?
        };
        if let Some(completion) = &completion {
            completion.metadata(|completion| {
                completion.args = args.iter().map(|arg| arg.to_string()).collect();
                completion.flags = cmd
                    .signature
                    .flags
                    .iter()
                    .filter_map(|flag| {
                        let value = if flag.completions.is_some() {
                            args.get_flag(flag.name)
                        } else {
                            args.has_flag(flag.name).then_some("")
                        };
                        value.map(|value| (flag.name.to_owned(), value.to_owned()))
                    })
                    .collect();
            });
        }
        (cmd.fun)(cx, args, event).map_err(|err| anyhow!("'{}': {err}", cmd.name))
    })();
    if let Some(completion) = &completion {
        completion.capture_effects(cx.editor);
    }
    if let Some(scope) = scope {
        cx.jobs.leave_command(cx.editor, scope);
    }
    if let Some(completion) = completion {
        completion.finish_dispatch(
            cx.editor,
            result.as_ref().err().map(ToString::to_string),
            false,
        );
    }
    result
}

#[allow(clippy::unnecessary_unwrap)]
pub(super) fn command_mode(cx: &mut Context) {
    let mut prompt = Prompt::new_with_callback(
        ":".into(),
        Some(':'),
        complete_command_line,
        move |cx: &mut compositor::Context, input: &str, event: PromptEvent| {
            let callbacks =
                execute_command_line_with_invocation(cx, input, event, CommandInvocation::prompt())
                    .finish(cx.editor);
            (!callbacks.is_empty()).then(|| {
                Box::new(
                    move |compositor: &mut compositor::Compositor, cx: &mut compositor::Context| {
                        for callback in callbacks {
                            callback(compositor, cx);
                        }
                    },
                ) as compositor::Callback
            })
        },
    );

    let custom_commands = cx.editor.config().commands.clone();
    let plugin_docs = cx
        .editor
        .plugin_commands()
        .into_iter()
        .map(|command| {
            let doc = format_plugin_doc(&command.name, &command.doc, &command.arguments);
            (command.name, doc)
        })
        .collect();
    prompt.doc_fn =
        Box::new(move |input| command_line_doc_with_plugins(input, &custom_commands, &plugin_docs));

    // Calculate initial completion
    prompt.recalculate_completion(cx.editor);
    cx.push_layer(Box::new(prompt));
}

fn command_line_doc(input: &str) -> Option<Cow<'_, str>> {
    let (command, _, _) = command_line::split(input);
    let command = command
        .strip_prefix(CustomCommand::ESCAPE)
        .unwrap_or(command);
    let command = TYPABLE_COMMAND_MAP.get(command)?;

    if command.aliases.is_empty() && command.signature.flags.is_empty() {
        return Some(Cow::Borrowed(command.doc));
    }

    let mut doc = command.doc.to_string();

    if !command.aliases.is_empty() {
        doc.push_str("\nAliases: ");
        for (index, alias) in command.aliases.iter().enumerate() {
            if index > 0 {
                doc.push_str(", ");
            }
            write!(doc, "`:{alias}`").unwrap();
        }
    }

    if !command.signature.flags.is_empty() {
        const ARG_PLACEHOLDER: &str = " <arg>";

        fn flag_len(flag: &Flag) -> usize {
            let name_len = flag.name.len();
            let alias_len = if let Some(alias) = flag.alias {
                "/-".len() + alias.len_utf8()
            } else {
                0
            };
            let arg_len = if flag.completions.is_some() {
                ARG_PLACEHOLDER.len()
            } else {
                0
            };
            name_len + alias_len + arg_len
        }

        doc.push_str("\nFlags:");

        let max_flag_len = command.signature.flags.iter().map(flag_len).max().unwrap();

        for flag in command.signature.flags {
            let mut buf = [0u8; 4];
            let this_flag_len = flag_len(flag);
            write!(
                doc,
                "\n  `--{flag_text}`{spacer:spacing$}  {doc}",
                doc = flag.doc,
                // `fmt::Arguments` does not respect width controls so we must place the spacers
                // explicitly:
                spacer = "",
                spacing = max_flag_len - this_flag_len,
                flag_text = format_args!(
                    "{}{}{}{}",
                    flag.name,
                    // Ideally this would be written as a `format_args!` too but the borrow
                    // checker is not yet smart enough.
                    if flag.alias.is_some() { "/-" } else { "" },
                    if let Some(alias) = flag.alias {
                        alias.encode_utf8(&mut buf)
                    } else {
                        ""
                    },
                    if flag.completions.is_some() {
                        ARG_PLACEHOLDER
                    } else {
                        ""
                    }
                ),
            )
            .unwrap();
        }
    }

    Some(Cow::Owned(doc))
}

fn command_line_doc_with_custom<'a>(
    input: &'a str,
    custom_commands: &CustomCommands,
) -> Option<Cow<'a, str>> {
    let (command, _, _) = command_line::split(input);
    if !command.starts_with(CustomCommand::ESCAPE)
        && let Some(command) = custom_commands.get(command)
    {
        return (!command.hidden).then(|| Cow::Owned(command.prompt()));
    }
    command_line_doc(input)
}

fn command_line_doc_with_plugins<'a>(
    input: &'a str,
    custom_commands: &CustomCommands,
    plugin_docs: &HashMap<String, String>,
) -> Option<Cow<'a, str>> {
    let (command, _, _) = command_line::split(input);
    if !command.starts_with(CustomCommand::ESCAPE) && custom_commands.get(command).is_some() {
        return command_line_doc_with_custom(input, custom_commands);
    }
    command_line_doc_with_custom(input, custom_commands).or_else(|| {
        let command = command
            .strip_prefix(CustomCommand::ESCAPE)
            .unwrap_or(command);
        plugin_docs.get(command).cloned().map(Cow::Owned)
    })
}

fn complete_command_line(editor: &Editor, input: &str) -> Vec<ui::prompt::Completion> {
    let (command, rest, complete_command) = command_line::split(input);
    let config = editor.config();
    let (escaped, command) = command
        .strip_prefix(CustomCommand::ESCAPE)
        .map_or((false, command), |command| (true, command));

    if complete_command {
        let plugins = editor
            .plugin_commands()
            .into_iter()
            .map(|command| Cow::Owned(command.name));
        if escaped {
            fuzzy_match(
                command,
                TYPABLE_COMMAND_LIST
                    .iter()
                    .map(|command| Cow::Borrowed(command.name))
                    .chain(plugins),
                false,
            )
            .into_iter()
            .map(|(name, _)| (0.., format!("^{}", name).into()))
            .collect()
        } else {
            // PERF: prompt completions require `'static` spans, so names from reloadable config
            // must be copied. Revisit if the prompt can accept completion spans tied to config.
            let custom = config
                .commands
                .visible_names()
                .map(|name| Cow::Owned(name.to_owned()));
            let builtins = TYPABLE_COMMAND_LIST
                .iter()
                .map(|command| Cow::Borrowed(command.name));

            // Reuse the plugin command completion integration from Helix PR #8675.
            fuzzy_match(command, custom.chain(builtins).chain(plugins), false)
                .into_iter()
                .map(|(name, _)| (0.., name.into()))
                .collect()
        }
    } else if escaped {
        TYPABLE_COMMAND_MAP.get(command).map_or_else(
            || {
                editor
                    .plugin_command_arguments(command)
                    .map_or_else(Vec::new, |specification| {
                        complete_plugin_args(&specification, rest, command.len() + 2)
                    })
            },
            |cmd| {
                let args_offset = command.len() + 2;
                complete_command_args(editor, cmd.signature, &cmd.completer, rest, args_offset)
            },
        )
    } else {
        let completer_command = config
            .commands
            .get(command)
            .and_then(|custom| custom.completer.as_deref())
            .unwrap_or(command);
        TYPABLE_COMMAND_MAP.get(completer_command).map_or_else(
            || {
                editor
                    .plugin_command_arguments(completer_command)
                    .map_or_else(Vec::new, |specification| {
                        complete_plugin_args(&specification, rest, command.len() + 1)
                    })
            },
            |cmd| {
                let args_offset = command.len() + 1;
                complete_command_args(editor, cmd.signature, &cmd.completer, rest, args_offset)
            },
        )
    }
}

fn plugin_signature(specification: &plugin_api::commands::CommandArguments) -> Signature {
    Signature {
        positionals: (specification.min, Some(specification.max)),
        ..Signature::DEFAULT
    }
}

pub(super) fn format_plugin_doc(
    name: &str,
    doc: &str,
    specification: &plugin_api::commands::CommandArguments,
) -> String {
    let arguments = match (specification.min, specification.max) {
        (0, 0) => "no arguments".into(),
        (min, max) if min == max => format!("{min} argument{}", if min == 1 { "" } else { "s" }),
        (min, max) => format!("{min}–{max} arguments"),
    };
    format!("`:{name}` — {arguments}\n\n{doc}")
}

fn complete_plugin_args(
    specification: &plugin_api::commands::CommandArguments,
    input: &str,
    offset: usize,
) -> Vec<ui::prompt::Completion> {
    use command_line::{CompletionState, Tokenizer};
    let mut tokenizer = Tokenizer::new(input, false);
    let mut args = Args::new(plugin_signature(specification), false);
    args.push(Cow::Borrowed("--")).unwrap();
    let mut final_token = None;
    let mut trailing_space = true;
    while let Some(token) = args
        .read_token(&mut tokenizer)
        .expect("unvalidated argument parsing cannot fail")
    {
        trailing_space = tokenizer.pos() < input.len();
        final_token = Some(token.clone());
        args.push(token.content).unwrap();
    }
    let token = if trailing_space {
        let token = Token::empty_at(input.len());
        args.push(token.content.clone()).unwrap();
        token
    } else {
        final_token.unwrap()
    };
    if token.is_terminated
        || !matches!(token.kind, TokenKind::Unquoted | TokenKind::Quoted(_))
        || !matches!(args.completion_state(), CompletionState::Positional)
    {
        return Vec::new();
    }
    let Some(candidates) = specification.completions.get(args.len().saturating_sub(1)) else {
        return Vec::new();
    };
    fuzzy_match(&token.content, candidates.iter(), false)
        .into_iter()
        .map(|(candidate, _)| {
            if matches!(token.kind, TokenKind::Unquoted) && candidate.contains(['\'', '"']) {
                return (
                    (offset + token.content_start)..,
                    format!("'{}'", candidate.replace('\'', "''")).into(),
                );
            }
            quote_completion(&token, 0.., candidate.clone().into(), offset)
        })
        .collect()
}

pub fn complete_command_args(
    editor: &Editor,
    signature: Signature,
    completer: &CommandCompleter,
    input: &str,
    offset: usize,
) -> Vec<ui::prompt::Completion> {
    use command_line::{CompletionState, ExpansionKind, Tokenizer};

    // TODO: completion should depend on the location of the cursor instead of the end of the
    // string. This refactor is left for the future but the below completion code should respect
    // the cursor position if it becomes a parameter.
    let cursor = input.len();
    let prefix = &input[..cursor];
    let mut tokenizer = Tokenizer::new(prefix, false);
    let mut args = Args::new(signature, false);
    let mut final_token = None;
    let mut is_last_token = true;

    while let Some(token) = args
        .read_token(&mut tokenizer)
        .expect("arg parsing cannot fail when validation is turned off")
    {
        final_token = Some(token.clone());
        args.push(token.content)
            .expect("arg parsing cannot fail when validation is turned off");
        if tokenizer.pos() >= cursor {
            is_last_token = false;
        }
    }

    // Use a fake final token when the input is not terminated with a token. This simulates an
    // empty argument, causing completion on an empty value whenever you type space/tab. For
    // example if you say `":open README.md "` (with that trailing space) you should see the
    // files in the current dir - completing `""` rather than completions for `"README.md"` or
    // `"README.md "`.
    let token = if is_last_token {
        let token = Token::empty_at(prefix.len());
        args.push(token.content.clone()).unwrap();
        token
    } else {
        final_token.unwrap()
    };

    // Don't complete on closed tokens, for example after writing a closing double quote.
    if token.is_terminated {
        return Vec::new();
    }

    match token.kind {
        TokenKind::Unquoted | TokenKind::Quoted(_) => {
            match args.completion_state() {
                CompletionState::Positional => {
                    // If the completion state is positional there must be at least one positional
                    // in `args`.
                    let n = args
                        .len()
                        .checked_sub(1)
                        .expect("completion state to be positional");
                    let completer = completer.for_argument_number(n);

                    completer(editor, &token.content)
                        .into_iter()
                        .map(|(range, span)| quote_completion(&token, range, span, offset))
                        .collect()
                }
                CompletionState::Flag(_) => fuzzy_match(
                    token.content.trim_start_matches('-'),
                    signature.flags.iter().map(|flag| flag.name),
                    false,
                )
                .into_iter()
                .map(|(name, _)| ((offset + token.content_start).., format!("--{name}").into()))
                .collect(),
                CompletionState::FlagArgument(flag) => fuzzy_match(
                    &token.content,
                    flag.completions
                        .expect("flags in FlagArgument always have completions"),
                    false,
                )
                .into_iter()
                .map(|(value, _)| ((offset + token.content_start).., (*value).into()))
                .collect(),
            }
        }
        TokenKind::Expand | TokenKind::Expansion(ExpansionKind::Shell) => {
            // See the comment about the checked sub expect above.
            let arg_completer = matches!(args.completion_state(), CompletionState::Positional)
                .then(|| {
                    let n = args
                        .len()
                        .checked_sub(1)
                        .expect("completion state to be positional");
                    completer.for_argument_number(n)
                });
            complete_expand(editor, &token, arg_completer, offset + token.content_start)
        }
        TokenKind::Expansion(ExpansionKind::Variable) => {
            complete_variable_expansion(&token.content, offset + token.content_start)
        }
        TokenKind::Expansion(ExpansionKind::Unicode) => Vec::new(),
        TokenKind::Expansion(ExpansionKind::Register) => {
            complete_register_expansion(editor, &token.content, offset + token.content_start)
        }
        TokenKind::Expansion(ExpansionKind::Arg) => Vec::new(),
        TokenKind::ExpansionKind => {
            complete_expansion_kind(&token.content, offset + token.content_start)
        }
    }
}

/// Replace the content and optionally update the range of a positional's completion to account
/// for quoting.
///
/// This is used to handle completions of file or directory names for example. When completing a
/// file with a space, tab or percent character in the name, the space should be escaped by
/// quoting the entire token. If the token being completed is already quoted, any quotes within
/// the completion text should be escaped by doubling them.
fn quote_completion<'a>(
    token: &Token,
    range: ops::RangeFrom<usize>,
    mut span: Span<'a>,
    offset: usize,
) -> (ops::RangeFrom<usize>, Span<'a>) {
    fn replace<'a>(text: Cow<'a, str>, from: char, to: &str) -> Cow<'a, str> {
        if text.contains(from) {
            Cow::Owned(text.replace(from, to))
        } else {
            text
        }
    }

    match token.kind {
        TokenKind::Unquoted if span.content.contains([' ', '\t', '%']) => {
            span.content = Cow::Owned(format!(
                "'{}{}'",
                // Escape any inner single quotes by doubling them.
                replace(token.content[..range.start].into(), '\'', "''"),
                replace(span.content, '\'', "''")
            ));
            // Ignore `range.start` here since we're replacing the entire token. We used
            // `range.start` above to emulate the replacement that using `range.start` would have
            // done.
            ((offset + token.content_start).., span)
        }
        TokenKind::Quoted(quote) => {
            span.content = replace(span.content, quote.char(), quote.escape());
            ((range.start + offset + token.content_start).., span)
        }
        TokenKind::Expand => {
            // NOTE: `token.content_start` is already accounted for in `offset` for `Expand`
            // tokens.
            span.content = replace(span.content, '"', "\"\"");
            ((range.start + offset).., span)
        }
        _ => ((range.start + offset + token.content_start).., span),
    }
}

fn complete_expand(
    editor: &Editor,
    token: &Token,
    completer: Option<&Completer>,
    offset: usize,
) -> Vec<ui::prompt::Completion> {
    use command_line::{ExpansionKind, Tokenizer};

    let mut start = 0;

    // If the expand token contains expansions, complete those.
    while let Some(idx) = token.content[start..].find('%') {
        let idx = start + idx;
        if token.content.as_bytes().get(idx + '%'.len_utf8()).copied() == Some(b'%') {
            // Two percents together are skipped.
            start = idx + ('%'.len_utf8() * 2);
        } else {
            let mut tokenizer = Tokenizer::new(&token.content[idx..], false);
            let token = tokenizer
                .parse_percent_token()
                .map(|token| token.expect("arg parser cannot fail when validation is disabled"));
            start = idx + tokenizer.pos();

            // Like closing quote characters in `complete_command_args` above, don't provide
            // completions if the token is already terminated. This also skips expansions
            // which have already been fully written, for example
            // `"%{cursor_line}:%{cursor_col` should complete `cursor_column` instead of
            // `cursor_line`.
            let Some(token) = token.filter(|t| !t.is_terminated) else {
                continue;
            };

            let local_offset = offset + idx + token.content_start;
            match token.kind {
                TokenKind::Expansion(ExpansionKind::Variable) => {
                    return complete_variable_expansion(&token.content, local_offset);
                }
                TokenKind::Expansion(ExpansionKind::Shell) => {
                    return complete_expand(editor, &token, None, local_offset);
                }
                TokenKind::ExpansionKind => {
                    return complete_expansion_kind(&token.content, local_offset);
                }
                _ => continue,
            }
        }
    }

    match completer {
        // If no expansions were found and an argument is being completed,
        Some(completer) if start == 0 => completer(editor, &token.content)
            .into_iter()
            .map(|(range, span)| quote_completion(token, range, span, offset))
            .collect(),
        _ => Vec::new(),
    }
}

fn complete_variable_expansion(content: &str, offset: usize) -> Vec<ui::prompt::Completion> {
    use expansion::Variable;

    fuzzy_match(
        content,
        Variable::VARIANTS.iter().map(Variable::as_str),
        false,
    )
    .into_iter()
    .map(|(name, _)| (offset.., (*name).into()))
    .collect()
}

fn complete_register_expansion(
    editor: &Editor,
    content: &str,
    offset: usize,
) -> Vec<ui::prompt::Completion> {
    let register_names: Vec<String> = editor
        .registers
        .iter_preview()
        .map(|(ch, _)| ch.to_string())
        .collect();
    fuzzy_match(content, register_names, false)
        .into_iter()
        .map(|(name, _)| (offset.., name.to_string().into()))
        .collect()
}

fn complete_expansion_kind(content: &str, offset: usize) -> Vec<ui::prompt::Completion> {
    use command_line::ExpansionKind;

    fuzzy_match(
        content,
        // Skip `ExpansionKind::Variable` since its kind string is empty.
        ExpansionKind::VARIANTS
            .iter()
            .skip(1)
            .map(ExpansionKind::as_str),
        false,
    )
    .into_iter()
    .map(|(name, _)| (offset.., (*name).into()))
    .collect()
}

#[cfg(test)]
mod command_line_doc_tests {
    use super::*;

    #[test]
    fn plugin_literal_argument_completion_respects_position_and_quotes() {
        let specification = plugin_api::commands::CommandArguments {
            min: 1,
            max: 2,
            completions: vec![
                vec!["two words".into(), "say'hi".into(), "-1".into()],
                vec!["μ".into()],
            ],
        };
        let first = complete_plugin_args(&specification, "", 10);
        assert!(first
            .iter()
            .any(|(range, span)| range.start == 10 && span.content == "'two words'"));
        assert!(first.iter().any(|(_, span)| span.content == "'say''hi'"));
        assert!(first.iter().any(|(_, span)| span.content == "-1"));
        let second = complete_plugin_args(&specification, "one μ", 10);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].0.start, 14);
        assert_eq!(second[0].1.content, "μ");
        assert!(complete_plugin_args(&specification, "%sh{echo x}", 0).is_empty());
        assert!(
            Args::parse("", plugin_signature(&specification), true, |token| Ok(
                token.content
            ))
            .is_err()
        );
        assert!(Args::parse(
            "one two three",
            plugin_signature(&specification),
            true,
            |token| Ok(token.content)
        )
        .is_err());
        assert!(
            format_plugin_doc("fixture.run", "Example", &specification).contains("1–2 arguments")
        );
    }

    #[test]
    fn formats_command_metadata_as_markdown() {
        let exit_doc = command_line_doc("exit").unwrap();
        assert!(exit_doc.contains("(`:exit some/path.txt`)"));
        assert!(exit_doc.contains("Aliases: `:x`, `:xit`"));
        assert!(exit_doc.contains("\n  `--no-format`"));

        let sort_doc = command_line_doc("sort").unwrap();
        assert!(sort_doc.contains("\n  `--insensitive/-i`"));
        assert!(sort_doc.contains("\n  `--reverse/-r`"));
    }

    #[test]
    fn formats_custom_command_docs_as_markdown() {
        let commands = CustomCommands::new(vec![CustomCommand {
            name: "save-and-close".into(),
            description: Some("Save *and* close the buffer".into()),
            commands: vec![":write".into(), ":buffer-close".into()],
            accepts: Some("<path>".into()),
            ..CustomCommand::default()
        }]);

        let doc = command_line_doc_with_custom("save-and-close", &commands).unwrap();
        assert!(doc.starts_with("`:save-and-close` `<path>` — Save *and* close"));
        assert!(doc.contains("Maps to: `:write` → `:buffer-close`"));
    }

    #[test]
    fn plugin_docs_respect_custom_command_shadowing_and_escape() {
        let plugin_docs =
            HashMap::from([("example.hello".into(), "Greet from WebAssembly".into())]);
        let empty = CustomCommands::default();
        assert_eq!(
            command_line_doc_with_plugins("example.hello 'hello world'", &empty, &plugin_docs)
                .unwrap(),
            "Greet from WebAssembly"
        );

        let commands = CustomCommands::new(vec![CustomCommand {
            name: "example.hello".into(),
            commands: vec![":write".into()],
            hidden: true,
            ..CustomCommand::default()
        }]);
        assert!(command_line_doc_with_plugins("example.hello", &commands, &plugin_docs).is_none());
        assert_eq!(
            command_line_doc_with_plugins("^example.hello", &commands, &plugin_docs).unwrap(),
            "Greet from WebAssembly"
        );
    }
}
