mod render;
use super::highlights::primary_native_cursor_kind;
use crate::repeat::InsertEvent;
use crate::{
    commands::{self, OnKeyCallback, OnKeyCallbackKind},
    compositor::{Component, Context, Cursor, Event, EventResult},
    events::OnModeSwitch,
    key,
    keymap::{KeymapResult, Keymaps},
    ui::{layout::ApplicationLayout, Completion},
};
use render::{PaneOptions, PaneRenderer};
use stdx::rope::RopeExt as _;

use editor_config::BufferLine;
use editor_core::{movement::Direction, unicode::width::UnicodeWidthStr, Range, Selection};
use std::{mem::take, num::NonZeroUsize};
use ui_core::{
    input::{KeyEvent, MouseButton, MouseEvent, MouseEventKind},
    keyboard::{KeyCode, KeyModifiers},
};
use view::{
    document::Mode,
    features::completion::CompletionItem,
    graphics::{Modifier, Rect},
    Editor,
};

use tui::{
    buffer::Buffer as Surface,
    layout::{Alignment, Constraint, Layout},
    widgets::{Paragraph, Widget},
};

pub struct EditorView {
    pub keymaps: Keymaps,
    on_next_key: Option<(OnKeyCallback, OnKeyCallbackKind)>,
    pseudo_pending: Vec<KeyEvent>,
    pub(crate) repeat: crate::repeat::RepeatController,
    pub(crate) completion: Option<Completion>,
    renderer: PaneRenderer,
    /// Tracks if the terminal window is focused by reaction to terminal focus events
    terminal_focused: bool,
}

impl EditorView {
    pub fn new(keymaps: Keymaps) -> Self {
        Self {
            keymaps,
            on_next_key: None,
            pseudo_pending: Vec::new(),
            repeat: crate::repeat::RepeatController::default(),
            completion: None,
            renderer: PaneRenderer::default(),
            terminal_focused: true,
        }
    }

    pub(crate) fn set_next_key_callback(
        &mut self,
        callback: (commands::OnKeyCallback, OnKeyCallbackKind),
    ) {
        self.on_next_key = Some(callback);
    }

    pub(crate) fn record_insert_event(&mut self, event: InsertEvent) {
        self.repeat.record(event);
    }

    /// Handle events by looking them up in `self.keymaps`. Returns None
    /// if event was handled (a command was executed or a subkeymap was
    /// activated). Only KeymapResult::{NotFound, Cancelled} is returned
    /// otherwise.
    fn handle_keymap_event(
        &mut self,
        mode: Mode,
        cxt: &mut commands::Context,
        event: KeyEvent,
    ) -> Option<KeymapResult> {
        let mut last_mode = mode;
        self.pseudo_pending.extend(self.keymaps.pending());
        let key_result = self.keymaps.get(mode, event);
        cxt.editor.autoinfo = self.keymaps.sticky().map(|node| node.infobox());

        let mut execute_command = |command: &commands::MappableCommand| {
            command.execute(cxt);
            if cxt.editor.should_close() {
                return false;
            }

            let current_mode = cxt.editor.mode();
            if current_mode != last_mode {
                event::dispatch(OnModeSwitch {
                    old_mode: last_mode,
                    new_mode: current_mode,
                    cx: cxt,
                })
                .report_to(cxt.editor.status_sender());

                // HAXX: if we just entered insert mode from normal, clear key buf
                // and record the command that got us into this mode.
                if current_mode == Mode::Insert {
                    // how we entered insert mode is important, and we should track that so
                    // we can repeat the side effect.
                    self.repeat.start_insert(command.clone());
                }
            }

            last_mode = current_mode;
            true
        };

        match &key_result {
            KeymapResult::Matched(command) => {
                execute_command(command);
            }
            KeymapResult::Pending(node) => cxt.editor.autoinfo = Some(node.infobox()),
            KeymapResult::MatchedSequence(commands) => {
                for command in commands {
                    if !execute_command(command) {
                        break;
                    }
                }
            }
            KeymapResult::NotFound | KeymapResult::Cancelled(_) => return Some(key_result),
        }
        None
    }

    pub(crate) fn insert_mode(
        &mut self,
        cx: &mut commands::Context,
        event: KeyEvent,
    ) -> Result<(), view::editing::EditError> {
        if let Some(keyresult) = self.handle_keymap_event(Mode::Insert, cx, event) {
            match keyresult {
                KeymapResult::NotFound => {
                    if !self.on_next_key(OnKeyCallbackKind::Fallback, cx, event)
                        && let Some(ch) = event.char()
                    {
                        commands::insert::try_insert_char(cx, ch)?
                    }
                }
                KeymapResult::Cancelled(pending) => {
                    for ev in pending {
                        match ev.char() {
                            Some(ch) => commands::insert::try_insert_char(cx, ch)?,
                            None => {
                                if let KeymapResult::Matched(command) =
                                    self.keymaps.get(Mode::Insert, ev)
                                {
                                    command.execute(cx);
                                }
                            }
                        }
                    }
                }
                _ => unreachable!(),
            }
        }
        Ok(())
    }

    fn command_mode(&mut self, mode: Mode, cxt: &mut commands::Context, event: KeyEvent) {
        match (event, cxt.editor.count) {
            // If the count is already started and the input is a number, always continue the count.
            (key!(i @ '0'..='9'), Some(count)) => {
                let i = i.to_digit(10).unwrap() as usize;
                let count = count.get() * 10 + i;
                if count > 100_000_000 {
                    return;
                }
                cxt.editor.count = NonZeroUsize::new(count);
            }
            // A non-zero digit will start the count if that number isn't used by a keymap.
            (key!(i @ '1'..='9'), None) if !self.keymaps.contains_key(mode, event) => {
                let i = i.to_digit(10).unwrap() as usize;
                cxt.editor.count = NonZeroUsize::new(i);
            }
            // special handling for repeat operator
            (key!('.'), _) if self.keymaps.pending().is_empty() => {
                if let Some(plan) = self.repeat.begin_replay() {
                    let count = cxt.editor.count.map_or(1, NonZeroUsize::into);
                    let result = plan.replay(cxt, count, |cx, key| self.insert_mode(cx, key));
                    self.repeat.end_replay();
                    if let Err(error) = result {
                        cxt.editor.set_error(|| error.to_string());
                    }
                }
                cxt.editor.count = None;
            }
            _ => {
                // set the count
                cxt.count = cxt.editor.count;
                // TODO: edge case: 0j -> reset to 1
                // if this fails, count was Some(0)
                // debug_assert!(cxt.count != 0);

                // set the register
                cxt.register = cxt.editor.selected_register.take();

                let res = self.handle_keymap_event(mode, cxt, event);
                if matches!(&res, Some(KeymapResult::NotFound)) {
                    self.on_next_key(OnKeyCallbackKind::Fallback, cxt, event);
                }
                if self.keymaps.pending().is_empty() {
                    cxt.editor.count = None
                } else {
                    cxt.editor.selected_register = cxt.register.take();
                }
            }
        }
    }

    pub fn set_completion(
        &mut self,
        editor: &mut Editor,
        items: Vec<CompletionItem>,
        session: view::features::completion::CompletionSession,
        size: Rect,
    ) -> Option<Rect> {
        let mut completion = Completion::new(editor, items, session.clone());

        if completion.is_empty() || !editor.begin_completion(&session) {
            // skip if we got no completion results
            return None;
        }

        let area = completion.area(size, editor);
        self.record_insert_event(InsertEvent::TriggerCompletion);

        // TODO : propagate required size on resize to completion too
        self.completion = Some(completion);
        Some(area)
    }

    pub fn clear_completion(&mut self, editor: &mut Editor) {
        self.completion = None;
        if let Some(applied) = editor.finish_completion() {
            self.record_insert_event(InsertEvent::CompletionApply(applied));
        }
    }

    pub fn handle_idle_timeout(&mut self, cx: &mut commands::Context) -> EventResult {
        view::features::inlay_hints::refresh(cx.editor);

        EventResult::Ignored(None)
    }
}

/// Whether the focused doc's workspace is in restricted mode and running `trust` would
/// change something visible at the workspace level.
fn workspace_trust_indicator_visible(editor: &Editor) -> bool {
    if editor.workspace_trust.implicit_level() == workspace::trust::ImplicitTrustLevel::Insecure {
        return false;
    }
    let (_, doc) = view::current_ref!(editor);
    editor
        .workspace_trust
        .restricted_for_doc(doc.workspace_root(), doc.servers_to_load())
}

impl EditorView {
    /// must be called whenever the editor processed input that
    /// is not a `KeyEvent`. In these cases any pending keys/on next
    /// key callbacks must be canceled.
    fn handle_non_key_input(&mut self, cxt: &mut commands::Context) {
        cxt.editor.status_msg = None;
        cxt.editor.reset_idle_timer();
        // HACKS: create a fake key event that will never trigger any actual map
        // and therefore simply acts as "dismiss"
        let null_key_event = KeyEvent {
            code: KeyCode::Null,
            modifiers: KeyModifiers::empty(),
        };
        // dismiss any pending keys
        if let Some((on_next_key, _)) = self.on_next_key.take() {
            on_next_key(cxt, null_key_event);
        }
        self.handle_keymap_event(cxt.editor.mode(), cxt, null_key_event);
        self.pseudo_pending.clear();
    }

    fn handle_mouse_event(
        &mut self,
        event: &MouseEvent,
        cxt: &mut commands::Context,
    ) -> EventResult {
        if event.kind != MouseEventKind::Moved {
            self.handle_non_key_input(cxt)
        }

        let config = cxt.editor.config();
        let MouseEvent {
            kind,
            row,
            column,
            modifiers,
            ..
        } = *event;

        let pos_and_view = |editor: &Editor, row, column, ignore_virtual_text| {
            editor.tree.views().find_map(|(view, _focus)| {
                view.pos_at_screen_coords(
                    &editor.documents[&view.document_id()],
                    row,
                    column,
                    ignore_virtual_text,
                )
                .map(|pos| (pos, view.id))
            })
        };

        let gutter_coords_and_view = |editor: &Editor, row, column| {
            editor.tree.views().find_map(|(view, _focus)| {
                view.gutter_coords_at_screen_coords(
                    &editor.documents[&view.document_id()],
                    row,
                    column,
                )
                .map(|coords| (coords, view.id))
            })
        };

        match kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let editor = &mut cxt.editor;

                if let Some((pos, view_id)) = pos_and_view(editor, row, column, true) {
                    editor.focus(view_id);

                    let prev_view_id = view!(editor).id;
                    let mode = editor.mode();
                    let doc = doc_mut!(editor, &view!(editor, view_id).document_id());

                    if modifiers == KeyModifiers::ALT {
                        let selection = doc.selection(view_id).clone();
                        doc.set_selection(view_id, selection.push(Range::point(pos)));
                    } else if mode == Mode::Select {
                        // Discards non-primary selections for consistent UX with normal mode
                        let primary = doc.selection(view_id).primary().put_cursor(
                            doc.text().slice(..),
                            pos,
                            true,
                        );
                        editor.mouse_down_range = Some(primary);
                        doc.set_selection(view_id, Selection::single(primary.anchor, primary.head));
                    } else {
                        doc.set_selection(view_id, Selection::point(pos));
                    }

                    if view_id != prev_view_id {
                        self.clear_completion(editor);
                    }

                    editor.ensure_cursor_in_view(view_id);

                    return EventResult::Consumed(None);
                }

                if let Some((coords, view_id)) = gutter_coords_and_view(editor, row, column) {
                    editor.focus(view_id);

                    let (view, doc) = current!(cxt.editor);

                    let Some(path) = doc.path().map(ToOwned::to_owned) else {
                        return EventResult::Ignored(None);
                    };

                    if let Some(char_idx) =
                        view.pos_at_visual_coords(doc, coords.row as u16, coords.col as u16, true)
                    {
                        let line = doc.text().char_to_line_idx(char_idx);
                        commands::dap_toggle_breakpoints_impl(cxt, path, &[line]);
                        return EventResult::Consumed(None);
                    }
                }

                EventResult::Ignored(None)
            }

            MouseEventKind::Drag(MouseButton::Left) => {
                let (view, doc) = current!(cxt.editor);

                let pos = match view.pos_at_screen_coords(doc, row, column, true) {
                    Some(pos) => pos,
                    None => return EventResult::Ignored(None),
                };

                let mut selection = doc.selection(view.id).clone();
                let primary = selection.primary_mut();
                *primary = primary.put_cursor(doc.text().slice(..), pos, true);
                doc.set_selection(view.id, selection);
                let view_id = view.id;
                cxt.editor.ensure_cursor_in_view(view_id);
                EventResult::Consumed(None)
            }

            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let direction = match event.kind {
                    MouseEventKind::ScrollUp => Direction::Backward,
                    MouseEventKind::ScrollDown => Direction::Forward,
                    _ => unreachable!(),
                };

                let Some((_, view_id)) = pos_and_view(cxt.editor, row, column, false) else {
                    return EventResult::Ignored(None);
                };
                let behaviour = if cxt.editor.mode() == Mode::Select {
                    editor_core::movement::Movement::Extend
                } else {
                    editor_core::movement::Movement::Move
                };
                cxt.editor.scroll_view(
                    view_id,
                    config.scroll.wheel_lines.unsigned_abs(),
                    direction,
                    view::editing::movement::ScrollCursor::KeepVisible,
                    behaviour,
                );

                EventResult::Consumed(None)
            }

            MouseEventKind::Up(MouseButton::Left) => {
                if !config.mouse.middle_click_paste {
                    return EventResult::Ignored(None);
                }

                let (view, doc) = current!(cxt.editor);

                let should_yank = match cxt.editor.mouse_down_range.take() {
                    Some(down_range) => doc.selection(view.id).primary() != down_range,
                    None => {
                        // This should not happen under normal cases. We fall back to the original
                        // behavior of yanking on non-single-char selections.
                        doc.selection(view.id)
                            .primary()
                            .slice(doc.text().slice(..))
                            .len_chars()
                            > 1
                    }
                };

                if should_yank {
                    commands::yank_main_selection_to_register(
                        cxt.editor,
                        config.mouse.yank_register,
                    );
                    EventResult::Consumed(None)
                } else {
                    EventResult::Ignored(None)
                }
            }

            MouseEventKind::Up(MouseButton::Right) => {
                if let Some((pos, view_id)) = gutter_coords_and_view(cxt.editor, row, column) {
                    cxt.editor.focus(view_id);

                    if let Some((pos, _)) = pos_and_view(cxt.editor, row, column, true) {
                        doc_mut!(cxt.editor).set_selection(view_id, Selection::point(pos));
                    } else {
                        let (view, doc) = current!(cxt.editor);

                        if let Some(pos) = view.pos_at_visual_coords(doc, pos.row as u16, 0, true) {
                            doc.set_selection(view_id, Selection::point(pos));
                            match modifiers {
                                KeyModifiers::ALT => {
                                    commands::MappableCommand::dap_edit_log.execute(cxt)
                                }
                                _ => commands::MappableCommand::dap_edit_condition.execute(cxt),
                            };
                        }
                    }

                    cxt.editor.ensure_cursor_in_view(view_id);
                    return EventResult::Consumed(None);
                }
                EventResult::Ignored(None)
            }

            MouseEventKind::Up(MouseButton::Middle) => {
                let editor = &mut cxt.editor;
                if !config.mouse.middle_click_paste {
                    return EventResult::Ignored(None);
                }

                if modifiers == KeyModifiers::ALT {
                    commands::replace_selections_with_register(
                        cxt.editor,
                        config.mouse.yank_register,
                        cxt.count(),
                    );

                    return EventResult::Consumed(None);
                }

                if let Some((pos, view_id)) = pos_and_view(editor, row, column, true) {
                    let doc = doc_mut!(editor, &view!(editor, view_id).document_id());
                    doc.set_selection(view_id, Selection::point(pos));
                    cxt.editor.focus(view_id);

                    commands::paste(
                        cxt.editor,
                        config.mouse.yank_register,
                        editor_core::transform::PastePosition::Before,
                        cxt.count(),
                    );

                    return EventResult::Consumed(None);
                }

                EventResult::Ignored(None)
            }

            _ => EventResult::Ignored(None),
        }
    }
    fn on_next_key(
        &mut self,
        kind: OnKeyCallbackKind,
        ctx: &mut commands::Context,
        event: KeyEvent,
    ) -> bool {
        match self.on_next_key.take() {
            Some((on_next_key, kind_)) => {
                if kind == kind_ {
                    on_next_key(ctx, event);
                    true
                } else {
                    self.on_next_key = Some((on_next_key, kind_));
                    false
                }
            }
            _ => false,
        }
    }
}

impl Component for EditorView {
    fn handle_event(
        &mut self,
        event: &Event,
        context: &mut crate::compositor::Context,
    ) -> EventResult {
        let mut cx = commands::Context {
            config: context.config,
            editor: context.editor,
            count: None,
            register: None,
            callback: Vec::new(),
            on_next_key_callback: None,
            jobs: context.jobs,
        };

        // Lifecycle updates may be queued while input is already ready. Never let
        // a stale menu consume input after its shared session was invalidated.
        if self.completion.is_some() && !view::features::completion::is_active(cx.editor) {
            self.clear_completion(cx.editor);
        }

        match event {
            Event::Paste(contents) => {
                self.handle_non_key_input(&mut cx);
                cx.count = cx.editor.count;
                commands::paste_bracketed_value(&mut cx, contents.clone());
                cx.editor.count = None;

                let config = cx.editor.config();
                let mode = cx.editor.mode();
                let (view, doc) = current!(cx.editor);
                view.ensure_cursor_in_view(doc, config.scroll.padding);

                // Store a history state if not in insert mode. Otherwise wait till we exit insert
                // to include any edits to the paste in the history state.
                if mode != Mode::Insert {
                    cx.editor.commit_changes();
                }

                EventResult::Consumed(None)
            }
            Event::Resize(_width, _height) => {
                // Ignore this event, we handle resizing just before rendering to screen.
                // Handling it here but not re-rendering will cause flashing
                EventResult::Consumed(None)
            }
            &Event::Key(mut key) => {
                cx.editor.reset_idle_timer();
                canonicalize_key(&mut key);
                if cx.jobs.command_expansion.is_running()
                    && matches!(key, key!(Esc) | crate::ctrl!('c'))
                {
                    cx.jobs.command_expansion.cancel();
                    cx.editor.set_status("Command cancelled");
                    return EventResult::Consumed(None);
                }

                // clear status
                cx.editor.status_msg = None;

                let mode = cx.editor.mode();

                if !self.on_next_key(OnKeyCallbackKind::PseudoPending, &mut cx, key) {
                    match mode {
                        Mode::Insert => {
                            // let completion swallow the event if necessary
                            let mut consumed = false;
                            if let Some(completion) = &mut self.completion {
                                let res = {
                                    // use a fake context here
                                    let mut cx = Context {
                                        config: cx.config,
                                        editor: cx.editor,
                                        jobs: cx.jobs,
                                        scroll: None,
                                        image_picker: None,
                                        is_cursor_owner: false,
                                    };

                                    match completion.handle_event(event, &mut cx) {
                                        EventResult::Consumed(callback) => {
                                            consumed = true;
                                            Some(callback)
                                        }
                                        _ => match completion.accept(&mut cx) {
                                            EventResult::Consumed(callback) => Some(callback),
                                            _ => None,
                                        },
                                    }
                                };

                                if let Some(callback) = res
                                    && callback.is_some()
                                {
                                    // assume close_fn
                                    self.clear_completion(cx.editor);
                                }
                            }

                            // if completion didn't take the event, we pass it onto commands
                            if !consumed {
                                match self.insert_mode(&mut cx, key) {
                                    Ok(()) => self.record_insert_event(InsertEvent::Key(key)),
                                    Err(error) => cx.editor.set_error(|| error.to_string()),
                                }
                            }
                        }
                        mode => self.command_mode(mode, &mut cx, key),
                    }
                }

                self.on_next_key = cx.on_next_key_callback.take();
                match self.on_next_key {
                    Some((_, OnKeyCallbackKind::PseudoPending)) => self.pseudo_pending.push(key),
                    _ => self.pseudo_pending.clear(),
                }

                // appease borrowck
                let callbacks = take(&mut cx.callback);

                // if the command consumed the last view, skip the render.
                // on the next loop cycle the Application will then terminate.
                if cx.editor.should_close() {
                    return EventResult::Ignored(None);
                }

                let config = cx.editor.config();
                let mode = cx.editor.mode();
                let (view, doc) = current!(cx.editor);

                view.ensure_cursor_in_view(doc, config.scroll.padding);

                // Store a history state if not in insert mode. This also takes care of
                // committing changes when leaving insert mode.
                if mode != Mode::Insert {
                    cx.editor.commit_changes();
                }
                let callback = if callbacks.is_empty() {
                    None
                } else {
                    let callback: crate::compositor::Callback = Box::new(move |compositor, cx| {
                        for callback in callbacks {
                            callback(compositor, cx)
                        }
                    });
                    Some(callback)
                };

                EventResult::Consumed(callback)
            }

            Event::Mouse(event) => self.handle_mouse_event(event, &mut cx),
            Event::IdleTimeout => self.handle_idle_timeout(&mut cx),
            Event::FocusGained => {
                self.terminal_focused = true;
                view::features::auto_reload::check_unwatched(context.editor);
                EventResult::Consumed(None)
            }
            Event::FocusLost => {
                view::features::auto_save::focus_lost(context.editor);
                self.terminal_focused = false;
                EventResult::Consumed(None)
            }
        }
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let config = cx.editor.config();
        let use_bufferline = match config.bufferline.show {
            BufferLine::Always => true,
            BufferLine::Multiple if cx.editor.documents.len() > 1 => true,
            _ => false,
        };
        let layout = ApplicationLayout::new(area, use_bufferline, config.statusline.global);
        cx.editor.resize(layout.editor);
        cx.editor.refresh_debugger_inline_values();
        self.renderer.render(
            cx.editor,
            layout,
            surface,
            cx.image_picker,
            PaneOptions {
                area,
                terminal_focused: self.terminal_focused,
                is_cursor_owner: cx.is_cursor_owner,
                true_color: cx.config.current.terminal.true_color || crate::true_color(),
            },
        );

        if config.auto_info
            && let Some(mut info) = cx.editor.autoinfo.take()
        {
            // Global statuslines are already outside layout.editor; per-view
            // statuslines occupy its bottom row.
            let info_area = layout
                .editor
                .clip_bottom(u16::from(!config.statusline.global));
            info.render(info_area, surface, cx);
            cx.editor.autoinfo = Some(info)
        }

        let key_width = 15u16; // for showing pending keys
        let status_msg_width = cx
            .editor
            .status_msg
            .as_ref()
            .map_or(0, |(status_msg, _)| status_msg.width());
        let macro_width = if cx.editor.macro_recording.is_some() {
            3
        } else {
            0
        };
        let restricted = workspace_trust_indicator_visible(cx.editor);
        let trust_width = if restricted { 3 } else { 0 };
        let show_pending = layout
            .commandline
            .width
            .saturating_sub(status_msg_width as u16)
            > key_width;
        let (status_area, pending_area, trust_area, macro_area) = if show_pending {
            let [status_area, pending_area, trust_area, macro_area] = Layout::horizontal([
                Constraint::Min(0),
                Constraint::Length(key_width),
                Constraint::Length(trust_width),
                Constraint::Length(macro_width),
            ])
            .areas(layout.commandline);
            (status_area, pending_area, trust_area, macro_area)
        } else {
            (
                layout.commandline,
                Rect::default(),
                Rect::default(),
                Rect::default(),
            )
        };

        // render status msg
        if let Some((status_msg, severity)) = &cx.editor.status_msg {
            use ::diagnostics::Severity;
            let style = if *severity == Severity::Error {
                cx.editor.theme.get("error")
            } else if *severity == Severity::Warning {
                cx.editor.theme.get("warning")
            } else {
                cx.editor.theme.get("ui.text")
            };

            Paragraph::new(status_msg.as_ref())
                .style(style)
                .render(status_area, surface);
        }

        if show_pending {
            let mut disp = String::new();
            if let Some(count) = cx.editor.count {
                disp.push_str(&count.to_string())
            }
            for key in self.keymaps.pending() {
                disp.push_str(&key.key_sequence_format());
            }
            for key in &self.pseudo_pending {
                disp.push_str(&key.key_sequence_format());
            }
            let style = cx.editor.theme.get("ui.text");
            let disp = disp
                .get(disp.len().saturating_sub(key_width as usize)..)
                .unwrap_or(&disp);
            Paragraph::new(disp)
                .style(style)
                .alignment(Alignment::Right)
                .render(pending_area, surface);

            if restricted {
                let style = style
                    .fg(view::graphics::Color::Yellow)
                    .add_modifier(Modifier::BOLD);
                Paragraph::new("[⚠]")
                    .style(style)
                    .render(trust_area, surface);
            }
            if let Some((reg, _)) = cx.editor.macro_recording {
                let disp = format!("[{}]", reg);
                let style = style
                    .fg(view::graphics::Color::Yellow)
                    .add_modifier(Modifier::BOLD);
                Paragraph::new(disp)
                    .style(style)
                    .render(macro_area, surface);
            }
        }

        if let Some(completion) = self.completion.as_mut() {
            completion.render(area, surface, cx);
        }
    }

    fn owns_cursor(&self) -> bool {
        true
    }

    fn cursor(&self, _area: Rect, editor: &Editor) -> Cursor {
        if current_ref!(editor).1.is_binary() {
            return Cursor::Hidden;
        }
        let (pos, kind) = editor.cursor();
        let Some(pos) = pos else {
            return Cursor::Hidden;
        };
        let pos = (pos.col as u16, pos.row as u16).into();
        if let Some(kind) = primary_native_cursor_kind(
            kind,
            editor.config().cursor.block_rendering,
            self.terminal_focused,
            true,
        ) {
            Cursor::Native(pos, kind)
        } else {
            Cursor::Software(pos)
        }
    }
}

fn canonicalize_key(key: &mut KeyEvent) {
    if let KeyEvent {
        code: KeyCode::Char(_),
        modifiers: _,
    } = key
    {
        key.modifiers.remove(KeyModifiers::SHIFT)
    }
}
use view::graphics::RectExt as _;
