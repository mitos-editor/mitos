//! Submitted command sequences pause at asynchronous argument expansion.

use command_line::{Args, Signature, Tokenizer};
use editor_config::CustomCommand;
use futures_util::{future::BoxFuture, FutureExt};
use std::{borrow::Cow, collections::VecDeque};
use view::{expansion, features::shell::ShellBehavior, Editor};

use super::super::{
    catalog::{TypableCommand, SHELL_SIGNATURE, TYPABLE_COMMAND_MAP},
    mappable::MappableCommand,
    navigation::GotoLinePreview,
};
use crate::{compositor, job, ui::PromptEvent};

enum Invocation {
    Typable(TypableCommand),
    Shell(ShellBehavior),
}

struct Command {
    invocation: Invocation,
    input: String,
    positional: Vec<Cow<'static, str>>,
}

enum Step {
    Command(Command),
    Mappable(MappableCommand),
}

struct Sequence {
    steps: VecDeque<Step>,
    preview: GotoLinePreview,
}

/// Capture target identity as well as text/selection. Switching away and back
/// cannot make a command valid against a replacement binding.
struct Target {
    owner: view::callbacks::EditorCallbackSender,
    view: view::ViewId,
    binding: u64,
    document: view::DocumentId,
    version: i32,
    selection: editor_core::Selection,
}

impl Target {
    fn capture(editor: &Editor) -> Self {
        let (view, doc) = view::current_ref!(editor);
        Self {
            owner: editor.callback_sender().clone(),
            view: view.id,
            binding: view.binding_revision(),
            document: doc.id(),
            version: doc.version(),
            selection: doc.selection(view.id).clone(),
        }
    }

    fn is_current(&self, editor: &Editor) -> bool {
        self.owner.same_destination(editor.callback_sender())
            && editor.tree.focus() == self.view
            && editor.tree.try_get(self.view).is_some_and(|view| {
                view.document_id() == self.document && view.binding_revision() == self.binding
            })
            && editor.document(self.document).is_some_and(|doc| {
                doc.version() == self.version && doc.selection(self.view) == &self.selection
            })
    }
}

impl Command {
    fn arguments(
        &self,
        editor: &Editor,
        interactive_shell: expansion::InteractiveShell,
    ) -> BoxFuture<'static, anyhow::Result<Args<'static>>> {
        let context = expansion::Context::capture(editor, &self.input)
            .with_interactive_shell(interactive_shell);
        let input = self.input.clone();
        let positional = self.positional.clone();
        let signature = match &self.invocation {
            Invocation::Typable(command) => command.signature,
            Invocation::Shell(_) => SHELL_SIGNATURE,
        };
        let name = match &self.invocation {
            Invocation::Typable(command) => Some(command.name),
            Invocation::Shell(_) => None,
        };
        async move {
            let mut parser = Tokenizer::new(&input, true);
            let mut args = Args::new(signature, true);
            while let Some(token) = args
                .read_token(&mut parser)
                .map_err(|error| anyhow::anyhow!("{error}"))?
            {
                let value = context.expand(token, &positional).await?;
                args.push(Cow::Owned(value.into_owned()))
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
            }
            args.finish().map_err(|error| anyhow::anyhow!("{error}"))?;
            Ok(args)
        }
        .map(move |result: anyhow::Result<Args<'static>>| {
            result.map_err(|error| match name {
                Some(name) => anyhow::anyhow!("'{name}': {error:#}"),
                None => error,
            })
        })
        .boxed()
    }

    fn apply(
        self,
        cx: &mut compositor::Context,
        args: Args,
        preview: &mut GotoLinePreview,
    ) -> anyhow::Result<()> {
        match self.invocation {
            Invocation::Typable(command) => if command.name == "goto" {
                preview.handle(cx.editor, args, PromptEvent::Validate)
            } else {
                preview.abort(cx.editor);
                (command.fun)(cx, args, PromptEvent::Validate)
            }
            .map_err(|error| anyhow::anyhow!("'{}': {error}", command.name)),
            Invocation::Shell(behavior) => {
                cx.editor
                    .request_shell_filter(cx.editor.tree.focus(), &args.join(" "), behavior);
                Ok(())
            }
        }
    }
}

impl Sequence {
    fn run(mut self, cx: &mut compositor::Context) -> anyhow::Result<Vec<compositor::Callback>> {
        let mut callbacks = Vec::new();
        while let Some(step) = self.steps.pop_front() {
            match step {
                Step::Mappable(command) => {
                    let mut context = super::super::Context {
                        config: cx.config,
                        register: None,
                        count: None,
                        editor: cx.editor,
                        callback: Vec::new(),
                        on_next_key_callback: None,
                        jobs: cx.jobs,
                    };
                    command.execute(&mut context);
                    if !context.callback.is_empty() {
                        callbacks.extend(context.callback);
                        callbacks.push(Box::new(move |compositor, cx| match self.run(cx) {
                            Ok(callbacks) => {
                                for callback in callbacks {
                                    callback(compositor, cx);
                                }
                            }
                            Err(error) => cx.editor.set_error(|| error.to_string()),
                        }));
                        return Ok(callbacks);
                    }
                }
                Step::Command(command) => {
                    let mut args = command.arguments(cx.editor, cx.jobs.interactive_shell());
                    if let Some(result) = args.as_mut().now_or_never() {
                        if let Err(error) =
                            result.and_then(|args| command.apply(cx, args, &mut self.preview))
                        {
                            self.preview.abort(cx.editor);
                            return Err(error);
                        }
                        continue;
                    }
                    let target = Target::capture(cx.editor);
                    let cancel = cx.jobs.command_expansion.restart();
                    cx.jobs.callback(async move {
                        let result = event::cancelable_future(args, &cancel).await;
                        Ok(job::Callback::Context(Box::new(move |compositor, cx| {
                            if cancel.is_canceled() || !target.is_current(cx.editor) {
                                return;
                            }
                            let Some(result) = result else {
                                return;
                            };
                            let result = match result
                                .and_then(|args| command.apply(cx, args, &mut self.preview))
                            {
                                Ok(()) => self.run(cx),
                                Err(error) => {
                                    self.preview.abort(cx.editor);
                                    Err(error)
                                }
                            };
                            match result {
                                Ok(callbacks) => {
                                    for callback in callbacks {
                                        callback(compositor, cx);
                                    }
                                }
                                Err(error) => cx.editor.set_error(|| error.to_string()),
                            }
                        })))
                    });
                    return Ok(callbacks);
                }
            }
        }
        self.preview.abort(cx.editor);
        Ok(callbacks)
    }
}

fn command(command: &TypableCommand, input: &str, positional: &[Cow<'_, str>]) -> Step {
    Step::Command(Command {
        invocation: Invocation::Typable(command.clone()),
        input: input.to_owned(),
        positional: positional
            .iter()
            .map(|value| Cow::Owned(value.to_string()))
            .collect(),
    })
}

#[derive(Default)]
pub(super) struct Session {
    preview: GotoLinePreview,
}

impl Session {
    pub fn execute(
        &mut self,
        cx: &mut compositor::Context,
        input: &str,
        event: PromptEvent,
    ) -> anyhow::Result<Vec<compositor::Callback>> {
        let result = self.execute_inner(cx, input, event);
        if result.is_err() {
            self.preview.abort(cx.editor);
        }
        result
    }

    fn execute_inner(
        &mut self,
        cx: &mut compositor::Context,
        input: &str,
        event: PromptEvent,
    ) -> anyhow::Result<Vec<compositor::Callback>> {
        let steps = resolve(cx.editor, input, event == PromptEvent::Validate)?;
        if event == PromptEvent::Validate {
            cx.jobs.command_expansion.cancel();
            return Sequence {
                steps,
                preview: std::mem::take(&mut self.preview),
            }
            .run(cx);
        }
        let mut preview_updated = false;
        for step in steps {
            let Step::Command(command) = step else {
                continue;
            };
            let Invocation::Typable(cmd) = command.invocation else {
                continue;
            };
            let args = preview_arguments(&cmd, &command.input, &command.positional)?;
            if cmd.name == "goto" {
                preview_updated = true;
                self.preview.handle(cx.editor, args, event)?;
            } else {
                (cmd.fun)(cx, args, event)
                    .map_err(|error| anyhow::anyhow!("'{}': {error}", cmd.name))?;
            }
        }
        if !preview_updated {
            self.preview.abort(cx.editor);
        }
        Ok(Vec::new())
    }
}

fn preview_arguments<'a>(
    cmd: &TypableCommand,
    input: &'a str,
    positional: &[Cow<'a, str>],
) -> anyhow::Result<Args<'a>> {
    Args::parse(input, cmd.signature, false, |token| {
        expansion::expand_only_arg(token, positional).map_err(Into::into)
    })
    .map_err(|error| anyhow::anyhow!("'{}': {error}", cmd.name))
}

fn resolve(editor: &Editor, input: &str, validate: bool) -> anyhow::Result<VecDeque<Step>> {
    let (name, args, _) = command_line::split(input);
    if name.is_empty() {
        return Ok(VecDeque::new());
    }
    let (escaped, name) = name
        .strip_prefix(CustomCommand::ESCAPE)
        .map_or((false, name), |name| (true, name));
    let custom = editor.config().commands.clone();
    let mut steps = VecDeque::new();
    if let Some(custom) = (!escaped).then(|| custom.get(name)).flatten() {
        let positional = Args::parse(args, Signature::DEFAULT, false, |token| Ok(token.content))
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        for configured in &custom.commands {
            if let Some(typable) = configured.strip_prefix(':') {
                let (name, args, _) = command_line::split(typable);
                let cmd = TYPABLE_COMMAND_MAP
                    .get(name)
                    .ok_or_else(|| anyhow::anyhow!("no such command: '{name}'"))?;
                steps.push_back(command(cmd, args, positional.as_slice()));
            } else {
                steps.push_back(Step::Mappable(configured.parse()?));
            }
        }
    } else {
        let (name, args) = if name.parse::<usize>().is_ok() && args.trim().is_empty() {
            ("goto", name)
        } else {
            (name, args)
        };
        let Some(cmd) = TYPABLE_COMMAND_MAP.get(name) else {
            if validate {
                anyhow::bail!("no such command: '{name}'");
            }
            return Ok(VecDeque::new());
        };
        steps.push_back(command(cmd, args, &[]));
    }
    Ok(steps)
}

pub(in crate::commands) fn execute_command(
    cx: &mut compositor::Context,
    cmd: &TypableCommand,
    input: &str,
    positional: &Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        let args = preview_arguments(cmd, input, positional.as_slice())?;
        return (cmd.fun)(cx, args, event);
    }
    // Key bindings use the same expansion and cancellation path as prompt submissions.
    cx.jobs.command_expansion.cancel();
    let callbacks = Sequence {
        steps: VecDeque::from([command(cmd, input, positional.as_slice())]),
        preview: GotoLinePreview::default(),
    }
    .run(cx)?;
    debug_assert!(callbacks.is_empty());
    Ok(())
}

pub(in crate::commands) fn execute_shell(
    cx: &mut compositor::Context,
    input: &str,
    behavior: ShellBehavior,
) -> anyhow::Result<()> {
    cx.jobs.command_expansion.cancel();
    Sequence {
        steps: VecDeque::from([Step::Command(Command {
            invocation: Invocation::Shell(behavior),
            input: input.to_owned(),
            positional: Vec::new(),
        })]),
        preview: GotoLinePreview::default(),
    }
    .run(cx)?;
    Ok(())
}
