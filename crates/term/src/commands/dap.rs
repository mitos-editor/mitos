use super::Context;
use crate::{
    compositor::{self, Compositor},
    job::{Callback, Jobs},
    ui::{self, overlay::overlaid, Picker, Popup, Prompt, PromptEvent, Text},
};
use dap::{self as dap, StackFrame, Thread};
use editor_core::SmallVec;
use language_config::{DebugConfigCompletion, DebugTemplate};
use std::path::PathBuf;
use tui::text::Line;
use view::{
    debugger::{Breakpoint, BreakpointProperty, ThreadStates},
    Editor,
};

fn debugger_query<T: Send + 'static>(
    jobs: &mut Jobs,
    task: view::debugger::DebuggerTask<T>,
    callback: impl FnOnce(&mut Editor, &mut Compositor, view::debugger::DebuggerContext, T)
        + Send
        + 'static,
) {
    jobs.callback(async move {
        let update = task.run().await;
        Ok(Callback::EditorCompositor(Box::new(
            move |editor, compositor| {
                let Some(update) = update else { return };
                let context = update.context();
                match update.into_result(editor) {
                    Some(Ok(result)) => callback(editor, compositor, context, result),
                    Some(Err(error)) => {
                        editor.set_error(|| format!("Debugger request failed: {error}"))
                    }
                    None => {}
                }
            },
        )))
    });
}

fn thread_picker(
    cx: &mut Context,
    callback_fn: impl Fn(&mut Editor, &view::debugger::DebuggerContext, &dap::Thread) + Send + 'static,
) {
    let task = match cx.editor.debugger_threads() {
        Ok(task) => task,
        Err(error) => {
            cx.editor.set_error(|| error.to_string());
            return;
        }
    };
    debugger_query(
        cx.jobs,
        task,
        move |editor, compositor, context, threads| {
            if threads.len() == 1 {
                callback_fn(editor, &context, &threads[0]);
                return;
            }
            let thread_states = editor
                .debugger()
                .get_active_session()
                .unwrap()
                .thread_states()
                .clone();
            let columns = [
                ui::PickerColumn::new("name", |item: &Thread, _| item.name.as_str().into()),
                ui::PickerColumn::new("state", |item: &Thread, states: &ThreadStates| {
                    states
                        .get(&item.id)
                        .map(String::as_str)
                        .unwrap_or("unknown")
                        .into()
                }),
            ];
            let preview_context = context.clone();
            let picker = Picker::new(
                editor.redraw().clone(),
                columns,
                0,
                threads,
                thread_states,
                move |cx, thread, _action| {
                    if context.is_current(cx.editor) {
                        callback_fn(cx.editor, &context, thread);
                    }
                },
            )
            .with_preview(move |editor, thread| {
                if !preview_context.is_current(editor) {
                    return None;
                }
                let frames = editor
                    .debugger()
                    .get_active_session()?
                    .stack_frames()
                    .get(&thread.id)?;
                let frame = frames.first()?;
                let path = frame.source.as_ref()?.path.as_ref()?.as_path();
                Some((
                    path.into(),
                    Some((
                        frame.line.saturating_sub(1),
                        frame.end_line.unwrap_or(frame.line).saturating_sub(1),
                    )),
                ))
            });
            compositor.push(Box::new(picker));
        },
    );
}

fn get_breakpoint_at_current_line(editor: &Editor) -> Option<Breakpoint> {
    let (view, doc) = current_ref!(editor);
    let line = doc
        .selection(view.id)
        .primary()
        .cursor_line(doc.text().slice(..));
    editor
        .debugger()
        .breakpoint_at_line(doc.path()?, line)
        .cloned()
}

// -- DAP

pub fn dap_start_impl(
    cx: &mut compositor::Context,
    name: Option<&str>,
    socket: Option<std::net::SocketAddr>,
    params: Option<Vec<std::borrow::Cow<str>>>,
) -> Result<(), anyhow::Error> {
    let document = doc!(cx.editor).id();
    start_for_document(cx, document, name, socket, params)
}

fn start_for_document(
    cx: &mut compositor::Context,
    document: view::DocumentId,
    name: Option<&str>,
    socket: Option<std::net::SocketAddr>,
    params: Option<Vec<std::borrow::Cow<str>>>,
) -> anyhow::Result<()> {
    let params = params.map(|params| {
        params
            .into_iter()
            .map(|value| value.into_owned())
            .collect::<Vec<_>>()
    });
    cx.editor
        .start_debugger(document, name, socket, params.as_deref())?;
    Ok(())
}

pub fn dap_launch(cx: &mut Context) {
    let doc = doc!(cx.editor);

    let config = match doc
        .language_config()
        .and_then(|config| config.debugger.as_ref())
    {
        Some(c) => c,
        None => {
            cx.editor
                .set_error(|| "No debug adapter available for language");
            return;
        }
    };

    let templates = config.templates.clone();
    let document = doc.id();

    let columns = [ui::PickerColumn::new(
        "template",
        |item: &DebugTemplate, _| item.name.as_str().into(),
    )];

    cx.push_layer(Box::new(overlaid(Picker::new(
        cx.editor.redraw().clone(),
        columns,
        0,
        templates,
        (),
        move |cx, template, _action| {
            if template.completion.is_empty() {
                if let Err(err) = start_for_document(cx, document, Some(&template.name), None, None)
                {
                    cx.editor.set_error(|| err.to_string());
                }
            } else {
                let completions = template.completion.clone();
                let name = template.name.clone();
                let callback = Box::pin(async move {
                    let call: Callback =
                        Callback::EditorCompositor(Box::new(move |_editor, compositor| {
                            let prompt =
                                debug_parameter_prompt(document, completions, name, Vec::new());
                            compositor.push(Box::new(prompt));
                        }));
                    Ok(call)
                });
                cx.jobs.callback(callback);
            }
        },
    ))));
}

pub fn dap_restart(cx: &mut Context) {
    debugger_command(cx, view::debugger::DebuggerCommand::Restart);
}

fn debug_parameter_prompt(
    document: view::DocumentId,
    completions: Vec<DebugConfigCompletion>,
    config_name: String,
    mut params: Vec<String>,
) -> Prompt {
    let completion = completions.get(params.len()).unwrap();
    let field_type = if let DebugConfigCompletion::Advanced(cfg) = completion {
        cfg.completion.as_deref().unwrap_or("")
    } else {
        ""
    };
    let name = match completion {
        DebugConfigCompletion::Advanced(cfg) => cfg.name.as_deref().unwrap_or(field_type),
        DebugConfigCompletion::Named(name) => name.as_str(),
    };
    let default_val = match completion {
        DebugConfigCompletion::Advanced(cfg) => cfg.default.as_deref().unwrap_or(""),
        _ => "",
    }
    .to_owned();

    let completer = match field_type {
        "filename" => |editor: &Editor, input: &str| {
            ui::completers::filename_with_git_ignore(editor, input, false)
        },
        "directory" => |editor: &Editor, input: &str| {
            ui::completers::directory_with_git_ignore(editor, input, false)
        },
        _ => ui::completers::none,
    };

    Prompt::new(
        format!("{}: ", name).into(),
        None,
        completer,
        move |cx, input: &str, event: PromptEvent| {
            if event != PromptEvent::Validate {
                return;
            }

            let mut value = input.to_owned();
            if value.is_empty() {
                value = default_val.clone();
            }
            params.push(value);

            if params.len() < completions.len() {
                let completions = completions.clone();
                let config_name = config_name.clone();
                let params = params.clone();
                let callback = Box::pin(async move {
                    let call: Callback =
                        Callback::EditorCompositor(Box::new(move |_editor, compositor| {
                            let prompt =
                                debug_parameter_prompt(document, completions, config_name, params);
                            compositor.push(Box::new(prompt));
                        }));
                    Ok(call)
                });
                cx.jobs.callback(callback);
            } else {
                if let Err(err) = start_for_document(
                    cx,
                    document,
                    Some(&config_name),
                    None,
                    Some(params.iter().map(|x| x.into()).collect()),
                ) {
                    cx.editor.set_error(|| err.to_string());
                }
            }
        },
    )
}

pub fn dap_toggle_breakpoints(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);

    let Some(path) = doc.path().map(ToOwned::to_owned) else {
        cx.editor
            .set_error(|| "Can't set breakpoint: document has no path");
        return;
    };

    let text = doc.text().slice(..);
    let lines: SmallVec<[usize; 1]> = doc
        .selection(view.id)
        .ranges()
        .iter()
        .map(|range| range.cursor_line(text))
        .collect();
    dap_toggle_breakpoints_impl(cx, path, &lines);
}

pub fn dap_toggle_breakpoints_impl(cx: &mut Context, path: PathBuf, lines: &[usize]) {
    if let Err(error) = cx.editor.toggle_breakpoints(path, lines) {
        cx.editor
            .set_error(|| format!("Failed to set breakpoints: {error}"));
    }
}

fn debugger_command(cx: &mut Context, command: view::debugger::DebuggerCommand) {
    if let Err(error) = cx.editor.debugger_command(command) {
        cx.editor.set_error(|| error.to_string());
    }
}

pub fn dap_continue(cx: &mut Context) {
    debugger_command(cx, view::debugger::DebuggerCommand::Continue);
}
pub fn dap_step_in(cx: &mut Context) {
    debugger_command(cx, view::debugger::DebuggerCommand::StepIn);
}
pub fn dap_step_out(cx: &mut Context) {
    debugger_command(cx, view::debugger::DebuggerCommand::StepOut);
}
pub fn dap_next(cx: &mut Context) {
    debugger_command(cx, view::debugger::DebuggerCommand::Next);
}
pub fn dap_pause(cx: &mut Context) {
    thread_picker(cx, |editor, context, thread| {
        if let Err(error) = editor.pause_debugger_thread(context, thread.id) {
            editor.set_error(|| error.to_string());
        }
    });
}
pub fn dap_terminate(cx: &mut Context) {
    debugger_command(cx, view::debugger::DebuggerCommand::Terminate);
}
pub fn dap_enable_exceptions(cx: &mut Context) {
    debugger_command(cx, view::debugger::DebuggerCommand::Exceptions(true));
}
pub fn dap_disable_exceptions(cx: &mut Context) {
    debugger_command(cx, view::debugger::DebuggerCommand::Exceptions(false));
}

pub fn dap_variables(cx: &mut Context) {
    let task = match cx.editor.debugger_variables() {
        Ok(task) => task,
        Err(error) => {
            cx.editor.set_error(|| error.to_string());
            return;
        }
    };
    debugger_query(cx.jobs, task, |editor, compositor, _context, scopes| {
        let scope_style = editor.theme.get("ui.linenr.selected");
        let type_style = editor.theme.get("ui.text");
        let text_style = editor.theme.get("ui.text.focus");
        let mut lines = Vec::new();
        for scope in scopes {
            lines.push(Line::from(tui::text::Span::styled(
                format!("▸ {}", scope.name),
                scope_style,
            )));
            for variable in scope.variables {
                let mut spans = vec![tui::text::Span::styled(variable.name, text_style)];
                if let Some(ty) = variable.ty {
                    spans.push(tui::text::Span::raw(": "));
                    spans.push(tui::text::Span::styled(ty, type_style));
                }
                spans.push(tui::text::Span::raw(" = "));
                spans.push(tui::text::Span::styled(variable.value, text_style));
                lines.push(Line::from(spans));
            }
        }
        let contents = Text::from(tui::text::Text::from(lines));
        compositor.replace_or_push("dap-variables", Popup::new("dap-variables", contents));
    });
}

pub fn dap_edit_condition(cx: &mut Context) {
    if let Some(breakpoint) = get_breakpoint_at_current_line(cx.editor) {
        let identity = breakpoint.identity();
        let Some(path) = doc!(cx.editor).path().map(ToOwned::to_owned) else {
            return;
        };
        let callback = Box::pin(async move {
            let call: Callback = Callback::EditorCompositor(Box::new(move |editor, compositor| {
                let mut prompt = Prompt::new(
                    "condition:".into(),
                    None,
                    ui::completers::none,
                    move |cx, input: &str, event: PromptEvent| {
                        if event != PromptEvent::Validate {
                            return;
                        }

                        let value = (!input.is_empty()).then(|| input.to_owned());
                        if let Err(error) = cx.editor.set_breakpoint_property(
                            &path,
                            identity,
                            BreakpointProperty::Condition,
                            value,
                        ) {
                            cx.editor
                                .set_error(|| format!("Failed to set breakpoints: {error}"));
                        }
                    },
                );
                if let Some(condition) = breakpoint.condition {
                    prompt.insert_str(&condition, editor)
                }
                compositor.push(Box::new(prompt));
            }));
            Ok(call)
        });
        cx.jobs.callback(callback);
    }
}

pub fn dap_edit_log(cx: &mut Context) {
    if let Some(breakpoint) = get_breakpoint_at_current_line(cx.editor) {
        let identity = breakpoint.identity();
        let Some(path) = doc!(cx.editor).path().map(ToOwned::to_owned) else {
            return;
        };
        let callback = Box::pin(async move {
            let call: Callback = Callback::EditorCompositor(Box::new(move |editor, compositor| {
                let mut prompt = Prompt::new(
                    "log-message:".into(),
                    None,
                    ui::completers::none,
                    move |cx, input: &str, event: PromptEvent| {
                        if event != PromptEvent::Validate {
                            return;
                        }

                        let value = (!input.is_empty()).then(|| input.to_owned());
                        if let Err(error) = cx.editor.set_breakpoint_property(
                            &path,
                            identity,
                            BreakpointProperty::LogMessage,
                            value,
                        ) {
                            cx.editor
                                .set_error(|| format!("Failed to set breakpoints: {error}"));
                        }
                    },
                );
                if let Some(log_message) = breakpoint.log_message {
                    prompt.insert_str(&log_message, editor);
                }
                compositor.push(Box::new(prompt));
            }));
            Ok(call)
        });
        cx.jobs.callback(callback);
    }
}

pub fn dap_switch_session(cx: &mut Context) {
    struct SessionItem {
        id: dap::DebugAdapterId,
        label: String,
        state: String,
        parent: String,
    }

    let debugger = cx.editor.debugger();
    let active = debugger.get_active_session().map(|session| session.id());
    let sessions: Vec<_> = debugger
        .sessions()
        .map(|session| SessionItem {
            id: session.id(),
            label: format!(
                "{} #{} {}",
                if active == Some(session.id()) {
                    "*"
                } else {
                    " "
                },
                session.number(),
                session.name()
            ),
            state: session.status().to_string(),
            parent: session
                .parent()
                .and_then(|id| debugger.get_session(id))
                .map(|parent| format!("#{} {}", parent.number(), parent.name()))
                .unwrap_or_default(),
        })
        .collect();
    if sessions.is_empty() {
        cx.editor.set_status("No debugging sessions");
        return;
    }
    let columns = [
        ui::PickerColumn::new("session", |item: &SessionItem, _| {
            item.label.as_str().into()
        }),
        ui::PickerColumn::new("state", |item: &SessionItem, _| item.state.as_str().into()),
        ui::PickerColumn::new("parent", |item: &SessionItem, _| {
            item.parent.as_str().into()
        }),
    ];
    cx.push_layer(Box::new(overlaid(Picker::new(
        cx.editor.redraw().clone(),
        columns,
        0,
        sessions,
        (),
        |cx, session, _action| {
            if let Err(error) = cx.editor.select_debugger_session(session.id) {
                cx.editor.set_error(|| error.to_string());
            }
        },
    ))));
}

pub fn dap_switch_thread(cx: &mut Context) {
    thread_picker(cx, |editor, context, thread| {
        if let Err(error) = editor.select_debugger_thread(context, thread.id) {
            editor.set_error(|| error.to_string());
        }
    });
}

pub fn dap_switch_stack_frame(cx: &mut Context) {
    let view::debugger::DebuggerFrames {
        context,
        frames,
        thread_state,
    } = match cx.editor.debugger_stack_frames() {
        Ok(frames) => frames,
        Err(error) => {
            cx.editor.set_error(|| error.to_string());
            return;
        }
    };

    let columns = [ui::PickerColumn::new(
        "frame",
        |item: &(usize, StackFrame), thread_state: &String| {
            format!("{} ({})", item.1.name, thread_state).into()
        },
    )];
    let preview_context = context.clone();
    let picker = Picker::new(
        cx.editor.redraw().clone(),
        columns,
        0,
        frames,
        thread_state,
        move |cx, (index, _frame), _action| {
            if let Err(error) = cx.editor.select_debugger_frame(&context, *index) {
                cx.editor.set_error(|| error.to_string());
            }
        },
    )
    .with_preview(move |editor, (_index, frame)| {
        if !preview_context.is_current(editor) {
            return None;
        }
        frame.source.as_ref().and_then(|source| {
            source.path.as_ref().map(|path| {
                (
                    path.as_path().into(),
                    Some((
                        frame.line.saturating_sub(1),
                        frame.end_line.unwrap_or(frame.line).saturating_sub(1),
                    )),
                )
            })
        })
    });
    cx.push_layer(Box::new(picker))
}

pub(super) mod typed {
    //! Typable dap commands.

    use crate::{commands::dap::dap_start_impl, compositor, ui::PromptEvent};
    use ::command_line::Args;

    #[cold]
    pub(in crate::commands) fn debug_eval(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let task = cx.editor.debugger_evaluate(args.join(" "))?;
        super::debugger_query(cx.jobs, task, |editor, _compositor, _context, response| {
            editor.set_status(response.result);
        });
        Ok(())
    }

    #[cold]
    pub(in crate::commands) fn debug_start(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let mut args: Vec<_> = args.into_iter().collect();
        let name = match args.len() {
            0 => None,
            _ => Some(args.remove(0)),
        };
        dap_start_impl(cx, name.as_deref(), None, Some(args))
    }

    #[cold]
    pub(in crate::commands) fn debug_remote(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let mut args: Vec<_> = args.into_iter().collect();
        let address = match args.len() {
            0 => None,
            _ => Some(args.remove(0).parse()?),
        };
        let name = match args.len() {
            0 => None,
            _ => Some(args.remove(0)),
        };
        dap_start_impl(cx, name.as_deref(), address, Some(args))
    }
}
