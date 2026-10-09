//! Native projections of owned plugin requests. Rendering never invokes guests.

mod keymaps;

use std::{
    cell::RefCell,
    num::NonZeroUsize,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use plugin_api::{
    ui::{
        terminal_text, BuiltinResponse, KeymapResponse, UiCancellation, UiIdentity, UiKind,
        UiOpenAction, UiOutcome, UiRequest, UiResponse, UiRow, UiValue, MAX_BUILTIN_SELECTION_WORK,
        MAX_UI_INPUT_BYTES,
    },
    ErrorCode, ServiceError,
};
use tui::{
    buffer::{Buffer as Surface, BufferExt},
    layout::{Constraint, Layout},
    widgets::{Paragraph, Widget, Wrap},
};
use ui_core::input::{KeyCode, KeyModifiers};
use view::{graphics::Rect, Editor};

use crate::{
    commands::{Context as CommandContext, MappableCommand},
    compositor::{self, Component, Compositor, Context, Cursor, Event, EventResult},
    events::CommandOrigin,
    job::Jobs,
    ui::{
        self,
        overlay::{overlaid, Overlay},
        Picker, Prompt, PromptEvent,
    },
};

const MAX_ACTIVE_UI: usize = 8;

#[cfg(feature = "integration")]
pub(crate) fn has_active_ui(compositor: &Compositor) -> bool {
    compositor.has_component(std::any::type_name::<PluginUi>())
}

/// Stored by one terminal application. Registrations never use process globals.
#[derive(Default)]
pub(crate) struct Frontend {
    keymaps: keymaps::ScopedKeymaps,
}

impl Frontend {
    /// Run after editor callbacks and before terminal input/rendering. Guest
    /// results enqueue models; this adapter only consumes their owned values.
    pub fn synchronize(&mut self, compositor: &mut Compositor, cx: &mut Context) {
        let mut active = 0;
        compositor.retain_type::<PluginUi>(|ui| {
            if cx.editor.plugin_ui_request_is_current(&ui.identity) {
                active += 1;
                true
            } else {
                let reason = if cx.editor.plugin_owner_is_current(&ui.identity.owner) {
                    UiCancellation::OriginLost
                } else {
                    UiCancellation::OwnerUnloaded
                };
                resolve(cx.editor, &ui.identity, UiOutcome::Cancelled { reason });
                false
            }
        });

        if let Some(editor) = compositor.find::<ui::EditorView>()
            && self.keymaps.retain(&mut editor.keymaps, |owner| {
                cx.editor.plugin_owner_is_current(owner)
            })
        {
            cx.editor.autoinfo = None;
        }
        for request in cx.editor.take_plugin_keymap_requests() {
            let identity = request.identity.clone();
            let result = if let Some(editor) = compositor.find::<ui::EditorView>() {
                self.keymaps.register(request, &mut editor.keymaps)
            } else {
                Err(ServiceError::new(
                    ErrorCode::Cancelled,
                    "terminal editor is unavailable",
                ))
            };
            let _ = cx.editor.resolve_plugin_keymap(KeymapResponse {
                identity,
                error: result.err(),
            });
            cx.editor.autoinfo = None;
        }
        for request in cx.editor.take_plugin_builtin_requests() {
            let mut completed = 0;
            let mut remaining_work = MAX_BUILTIN_SELECTION_WORK;
            let result = cx.editor.run_plugin_frontend_request(&request, |editor| {
                for invocation in &request.commands {
                    let (view, doc) = view::current_ref!(editor);
                    let work = invocation
                        .count
                        .unwrap_or(1)
                        .checked_mul(doc.selection(view.id).len());
                    if work.is_none_or(|work| work > remaining_work) {
                        return Err(ServiceError::new(
                            ErrorCode::ResourceExhausted,
                            "plugin builtin selection work exceeds its limit",
                        ));
                    }
                    remaining_work -= work.unwrap();
                    let before = editor.error_revision();
                    let mut command_cx = CommandContext {
                        config: cx.config,
                        register: None,
                        count: invocation.count.and_then(NonZeroUsize::new),
                        editor,
                        callback: Vec::new(),
                        on_next_key_callback: None,
                        jobs: cx.jobs,
                    };
                    // This conversion uses a closed reviewed enum, never a
                    // guest-supplied command line, macro or expansion string.
                    let command: MappableCommand =
                        invocation.command.name().parse().map_err(|error| {
                            ServiceError::new(ErrorCode::HostFailure, format!("{error}"))
                        })?;
                    command.execute_with_origin(&mut command_cx, CommandOrigin::Programmatic);
                    let callbacks = std::mem::take(&mut command_cx.callback);
                    let mut callback_cx = Context {
                        config: cx.config,
                        editor: command_cx.editor,
                        jobs: command_cx.jobs,
                        scroll: None,
                        image_picker: cx.image_picker,
                        is_cursor_owner: false,
                    };
                    for callback in callbacks {
                        callback(compositor, &mut callback_cx);
                    }
                    if callback_cx.editor.error_revision() != before && callback_cx.editor.is_err()
                    {
                        return Err(ServiceError::new(
                            ErrorCode::HostFailure,
                            callback_cx.editor.get_status().unwrap().0.to_string(),
                        ));
                    }
                    completed += 1;
                }
                Ok(())
            });
            let _ = cx.editor.resolve_plugin_builtin(BuiltinResponse {
                identity: request.identity,
                completed,
                error: result.err(),
            });
        }

        for mut request in cx.editor.take_plugin_ui_requests() {
            if !cx.editor.plugin_ui_request_is_current(&request.identity) {
                resolve(
                    cx.editor,
                    &request.identity,
                    UiOutcome::Cancelled {
                        reason: UiCancellation::OriginLost,
                    },
                );
                continue;
            }
            if active >= MAX_ACTIVE_UI {
                resolve(
                    cx.editor,
                    &request.identity,
                    UiOutcome::Failed {
                        error: ServiceError::new(
                            ErrorCode::ResourceExhausted,
                            "too many active plugin dialogs",
                        ),
                    },
                );
                continue;
            }
            match request.kind.normalize() {
                Ok(kind) => request.kind = kind,
                Err(error) => {
                    resolve(cx.editor, &request.identity, UiOutcome::Failed { error });
                    continue;
                }
            }
            let identity = request.identity.clone();
            compositor.push(Box::new(PluginUi::new(request, cx.editor, cx.jobs)));
            if let Err(error) = cx.editor.ack_plugin_ui_presented(&identity) {
                compositor.retain_type::<PluginUi>(|ui| ui.identity != identity);
                resolve(cx.editor, &identity, UiOutcome::Failed { error });
            } else {
                active += 1;
            }
        }
    }

    pub fn shutdown(&mut self, compositor: &mut Compositor, editor: &mut Editor) {
        compositor.retain_type::<PluginUi>(|ui| {
            resolve(
                editor,
                &ui.identity,
                UiOutcome::Cancelled {
                    reason: UiCancellation::Shutdown,
                },
            );
            false
        });
        if let Some(view) = compositor.find::<ui::EditorView>() {
            self.keymaps.retain(&mut view.keymaps, |_| false);
        }
    }
}

fn resolve(editor: &mut Editor, identity: &UiIdentity, outcome: UiOutcome) {
    if let Err(error) = editor.resolve_plugin_ui(UiResponse {
        identity: identity.clone(),
        outcome,
    }) {
        // Unload or a backend cancellation may have already consumed the token.
        if error.code != ErrorCode::StaleState && error.code != ErrorCode::Cancelled {
            editor.set_error(|| error.to_string());
        }
    }
}

type Outcome = Rc<RefCell<Option<UiOutcome>>>;

struct PluginUi {
    identity: UiIdentity,
    outcome: Outcome,
    content: UiContent,
    deadline: Option<tokio::time::Instant>,
    timer: Option<tokio::task::JoinHandle<()>>,
    timer_active: Arc<AtomicBool>,
}

enum UiContent {
    Prompt(Prompt),
    Picker(Box<Overlay<PluginPicker>>),
    NextKey(Prompt),
}

impl PluginUi {
    fn new(request: UiRequest, editor: &Editor, jobs: &Jobs) -> Self {
        let outcome = Rc::new(RefCell::new(None));
        let result = outcome.clone();
        let deadline = match &request.kind {
            UiKind::NextKey { timeout_ms, .. } => {
                Some(tokio::time::Instant::now() + Duration::from_millis(u64::from(*timeout_ms)))
            }
            _ => None,
        };
        let timer_active = Arc::new(AtomicBool::new(true));
        let timer = deadline.map(|deadline| {
            let active = timer_active.clone();
            let identity = request.identity.clone();
            let sender = jobs.editor_callback_sender();
            tokio::spawn(async move {
                tokio::time::sleep_until(deadline).await;
                sender
                    .send(move |editor| {
                        // A queued timeout from an earlier dialog must not cancel a
                        // new request that reuses the same guest correlation ID.
                        if active.swap(false, Ordering::Relaxed) {
                            resolve(
                                editor,
                                &identity,
                                UiOutcome::Cancelled {
                                    reason: UiCancellation::TimedOut,
                                },
                            );
                        }
                    })
                    .await;
            })
        });
        let content = match request.kind {
            UiKind::Prompt { title, initial } => {
                let prompt = Prompt::new(
                    format!("{}: {title} ", request.identity.owner.plugin).into(),
                    None,
                    ui::completers::none,
                    move |_, input: &str, event| {
                        *result.borrow_mut() = match event {
                            PromptEvent::Validate => Some(UiOutcome::Accepted {
                                value: UiValue::Prompt {
                                    text: terminal_text(input, false),
                                },
                            }),
                            PromptEvent::Abort => Some(UiOutcome::Cancelled {
                                reason: UiCancellation::User,
                            }),
                            PromptEvent::Update => None,
                        };
                    },
                )
                .with_line(initial, editor);
                UiContent::Prompt(prompt)
            }
            UiKind::Picker { title, rows } => {
                let columns = [
                    ui::PickerColumn::new("choice", |row: &UiRow, _: &()| {
                        row.label.as_str().into()
                    }),
                    ui::PickerColumn::new("detail", |row: &UiRow, _: &()| {
                        row.description.as_str().into()
                    }),
                ];
                let picker = Picker::new(columns, 0, rows, (), move |_, row, action| {
                    let action = match action {
                        view::editor::Action::HorizontalSplit => UiOpenAction::HorizontalSplit,
                        view::editor::Action::VerticalSplit => UiOpenAction::VerticalSplit,
                        _ => UiOpenAction::Replace,
                    };
                    *result.borrow_mut() = Some(UiOutcome::Accepted {
                        value: UiValue::Picker {
                            row: row.id.clone(),
                            action,
                        },
                    });
                });
                UiContent::Picker(Box::new(overlaid(PluginPicker {
                    picker,
                    title: format!("{}: {title}", request.identity.owner.plugin),
                    picker_area: Rect::default(),
                    preview_scroll: 0,
                    preview_visible: true,
                })))
            }
            UiKind::NextKey { title, .. } => UiContent::NextKey(Prompt::new(
                format!("{}: {title} ", request.identity.owner.plugin).into(),
                None,
                ui::completers::none,
                |_, _, _| {},
            )),
        };
        Self {
            identity: request.identity,
            outcome,
            content,
            deadline,
            timer,
            timer_active,
        }
    }

    fn input(&self) -> &str {
        match &self.content {
            UiContent::Prompt(prompt) | UiContent::NextKey(prompt) => prompt.line(),
            UiContent::Picker(picker) => picker.content.picker.query_input(),
        }
    }

    fn normalize_input(&mut self, editor: &Editor) {
        let input = terminal_text(self.input(), false);
        if input != self.input() {
            match &mut self.content {
                UiContent::Prompt(prompt) | UiContent::NextKey(prompt) => {
                    prompt.set_line(input, editor)
                }
                UiContent::Picker(picker) => picker.content.picker.set_query_input(input, editor),
            }
        }
    }

    fn close(&self, outcome: UiOutcome, callback: Option<compositor::Callback>) -> EventResult {
        let identity = self.identity.clone();
        EventResult::Consumed(Some(Box::new(move |compositor, cx| {
            if let Some(callback) = callback {
                callback(compositor, cx);
            }
            // A plugin dialog is a one-shot resource and must not be revived by
            // the native last-picker command after its response was delivered.
            compositor.retain_type::<PluginUi>(|ui| ui.identity != identity);
            resolve(cx.editor, &identity, outcome);
        })))
    }

    fn close_for_event(&self, event: &Event, outcome: UiOutcome) -> EventResult {
        let result = self.close(outcome, None);
        if matches!(event, Event::FocusGained | Event::FocusLost)
            && let EventResult::Consumed(callback) = result
        {
            return EventResult::Ignored(callback);
        }
        result
    }
}

impl Component for PluginUi {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        if !cx.editor.plugin_ui_request_is_current(&self.identity) {
            return self.close_for_event(
                event,
                UiOutcome::Cancelled {
                    reason: UiCancellation::OriginLost,
                },
            );
        }
        if matches!(&self.content, UiContent::NextKey(_)) {
            if self
                .deadline
                .is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
            {
                return self.close_for_event(
                    event,
                    UiOutcome::Cancelled {
                        reason: UiCancellation::TimedOut,
                    },
                );
            }
            let outcome = match event {
                Event::Key(key)
                    if key.code == KeyCode::Esc
                        || (key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL)) =>
                {
                    Some(UiOutcome::Cancelled {
                        reason: UiCancellation::User,
                    })
                }
                Event::Key(key) if key.code == KeyCode::Null => Some(UiOutcome::Cancelled {
                    reason: UiCancellation::Closed,
                }),
                Event::Key(key) => Some(UiOutcome::Accepted {
                    value: UiValue::NextKey {
                        key: key.to_string(),
                    },
                }),
                Event::Paste(_) | Event::Mouse(_) | Event::FocusLost => {
                    Some(UiOutcome::Cancelled {
                        reason: UiCancellation::Closed,
                    })
                }
                _ => None,
            };
            return outcome.map_or(EventResult::Ignored(None), |outcome| {
                self.close_for_event(event, outcome)
            });
        }
        if let Event::Key(key) = event {
            // Receiving literal user paste is allowed. Native register reads
            // and quicklist mutation are separate services, not UI authority.
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && (key.code == KeyCode::Char('r')
                    || (key.code == KeyCode::Char('q')
                        && matches!(self.content, UiContent::Picker(_))))
            {
                return EventResult::Consumed(None);
            }
        }
        let paste = match event {
            Event::Paste(text) => Some(Event::Paste(terminal_text(text, false))),
            _ => None,
        };
        let event = paste.as_ref().unwrap_or(event);
        if matches!(event, Event::Paste(text) if text.len().saturating_add(self.input().len()) > MAX_UI_INPUT_BYTES)
        {
            return self.close(
                UiOutcome::Failed {
                    error: ServiceError::new(
                        ErrorCode::ResourceExhausted,
                        "plugin input exceeds its size limit",
                    ),
                },
                None,
            );
        }
        let result = match &mut self.content {
            UiContent::Prompt(prompt) => prompt.handle_event(event, cx),
            UiContent::Picker(picker) => picker.handle_event(event, cx),
            UiContent::NextKey(_) => unreachable!(),
        };
        if self.input().len() > MAX_UI_INPUT_BYTES {
            return self.close(
                UiOutcome::Failed {
                    error: ServiceError::new(
                        ErrorCode::ResourceExhausted,
                        "plugin input exceeds its size limit",
                    ),
                },
                None,
            );
        }
        self.normalize_input(cx.editor);
        let outcome = self.outcome.borrow_mut().take();
        match (result, outcome) {
            (EventResult::Consumed(callback), Some(outcome)) => self.close(outcome, callback),
            (EventResult::Consumed(Some(callback)), None) => {
                let user = matches!(event, Event::Key(key) if key.code == KeyCode::Esc || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)));
                self.close(
                    UiOutcome::Cancelled {
                        reason: if user {
                            UiCancellation::User
                        } else {
                            UiCancellation::Closed
                        },
                    },
                    Some(callback),
                )
            }
            (result, _) => result,
        }
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        match &mut self.content {
            UiContent::Prompt(prompt) | UiContent::NextKey(prompt) => {
                prompt.render(area, surface, cx)
            }
            UiContent::Picker(picker) => picker.render(area, surface, cx),
        }
    }

    fn owns_cursor(&self) -> bool {
        true
    }

    fn cursor(&self, area: Rect, editor: &Editor) -> Cursor {
        match &self.content {
            UiContent::Prompt(prompt) | UiContent::NextKey(prompt) => prompt.cursor(area, editor),
            UiContent::Picker(picker) => picker.cursor(area, editor),
        }
    }

    fn id(&self) -> Option<&'static str> {
        Some("plugin-ui")
    }
}

impl Drop for PluginUi {
    fn drop(&mut self) {
        self.timer_active.store(false, Ordering::Relaxed);
        if let Some(timer) = self.timer.take() {
            timer.abort();
        }
    }
}

struct PluginPicker {
    picker: Picker<UiRow, ()>,
    title: String,
    picker_area: Rect,
    preview_scroll: u16,
    preview_visible: bool,
}

impl Component for PluginPicker {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        if let Event::Key(key) = event {
            if key.modifiers.contains(KeyModifiers::ALT) {
                match key.code {
                    KeyCode::Up => {
                        self.preview_scroll = self.preview_scroll.saturating_sub(1);
                        return EventResult::Consumed(None);
                    }
                    KeyCode::Down => {
                        self.preview_scroll = self.preview_scroll.saturating_add(1);
                        return EventResult::Consumed(None);
                    }
                    _ => (),
                }
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('t') {
                self.preview_visible = !self.preview_visible;
                return EventResult::Consumed(None);
            }
        }
        let before = self.picker.selection().map(|row| row.id.clone());
        let result = self.picker.handle_event(event, cx);
        if before != self.picker.selection().map(|row| row.id.clone()) {
            self.preview_scroll = 0;
        }
        result
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        surface.clear_with(area, cx.editor.theme.get("ui.background"));
        let [title, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        Paragraph::new(self.title.as_str())
            .style(cx.editor.theme.get("ui.text"))
            .render(title, surface);
        let preview = self.preview_visible
            && body.width >= 80
            && self
                .picker
                .selection()
                .is_some_and(|row| row.preview.is_some());
        let (picker_area, preview_area) = if preview {
            let [left, right] =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .areas(body);
            (left, Some(right))
        } else {
            (body, None)
        };
        self.picker_area = picker_area;
        self.picker.render(picker_area, surface, cx);
        if let Some(area) = preview_area
            && let Some(text) = self
                .picker
                .selection()
                .and_then(|row| row.preview.as_deref())
        {
            let block = ui::panel::bordered(&cx.editor.theme);
            let inner = block.inner(area);
            block.render(area, surface);
            Paragraph::new(text)
                .style(cx.editor.theme.get("ui.text"))
                .wrap(Wrap { trim: false })
                .scroll((self.preview_scroll, 0))
                .render(inner, surface);
        }
    }

    fn owns_cursor(&self) -> bool {
        true
    }

    fn cursor(&self, _area: Rect, editor: &Editor) -> Cursor {
        self.picker.cursor(self.picker_area, editor)
    }
}
