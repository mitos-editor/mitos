//! A retained file browser beside, and independent of, document splits.

use std::collections::{BTreeMap, BTreeSet};

use view::{graphics::Rect, info::Info, Editor};

use crate::{
    compositor::{Component, Context, Cursor, DrawContext, Event, EventResult},
    keymap::component::{ComponentKeymaps, PickerAction, SidebarAction},
};
use ui_core::keymap::{Binding, Keymap, KeymapState, Lookup};

use super::{editor::EditorView, layout::ApplicationLayout, FileExplorer};

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Hidden,
    Visible,
    Navigating,
    Filtering,
}

pub(super) struct Sidebar {
    pub picker: FileExplorer,
    state: State,
    keys: KeymapState,
    search: event::TaskController,
    search_pending: bool,
    pending_action: Option<PickerAction>,
    action_scheduled: bool,
    pending_input: Vec<(Event, Vec<char>)>,
}

impl Sidebar {
    pub fn new(editor: &Editor) -> std::io::Result<Self> {
        let picker = super::file_explorer::picker(workspace::find_workspace().0, editor, true)?
            .embedded(|compositor, _| {
                if let Some(sidebar) = compositor
                    .find::<EditorView>()
                    .and_then(|editor| editor.sidebar.as_mut())
                {
                    sidebar.focus_buffer();
                }
            });
        Ok(Self {
            picker,
            state: State::Navigating,
            keys: KeymapState::default(),
            search: event::TaskController::new(),
            search_pending: false,
            pending_action: None,
            action_scheduled: false,
            pending_input: Vec::new(),
        })
    }

    pub fn focused(&self) -> bool {
        matches!(self.state, State::Navigating | State::Filtering)
    }

    pub fn visible(&self) -> bool {
        self.state != State::Hidden
    }

    pub fn focus(&mut self) {
        self.clear_pending_input();
        self.state = State::Navigating;
        self.reset_input();
    }

    pub fn focus_buffer(&mut self) {
        self.clear_pending_input();
        if self.visible() {
            self.state = State::Visible;
        }
        self.reset_input();
    }

    pub fn hide(&mut self) {
        self.clear_pending_input();
        self.state = State::Hidden;
        self.reset_input();
        self.picker.suspend();
    }

    fn reset_input(&mut self) {
        self.keys.clear();
        self.picker.cancel_input();
        self.picker.set_title("File Explorer");
    }

    pub fn cursor(&self, area: Rect, cx: &DrawContext) -> Cursor {
        if self.state == State::Filtering {
            self.picker.cursor(area, cx)
        } else {
            Cursor::Hidden
        }
    }

    /// Reserve a bounded width while leaving at least 24 columns for documents.
    /// Small terminals temporarily hide the sidebar without forgetting its state.
    pub fn layout(&self, layout: &mut ApplicationLayout) -> Option<Rect> {
        if !self.visible() {
            return None;
        }
        Self::reserve_space(layout)
    }

    fn reserve_space(layout: &mut ApplicationLayout) -> Option<Rect> {
        if layout.editor.width < 45 || layout.editor.height < 4 {
            return None;
        }
        let width = (layout.editor.width / 3).clamp(20, 36);
        let top = layout.bufferline.map_or(layout.editor.y, |rect| rect.y);
        let area = Rect::new(layout.editor.x, top, width, layout.editor.bottom() - top);
        layout.editor.x += width;
        layout.editor.width -= width;
        if let Some(bufferline) = &mut layout.bufferline {
            bufferline.x += width;
            bufferline.width -= width;
        }
        Some(area)
    }

    fn keymap<'a>(&self, maps: &'a ComponentKeymaps) -> &'a Keymap<SidebarAction> {
        if self.state == State::Filtering {
            &maps.sidebar_filter
        } else {
            &maps.sidebar
        }
    }

    pub fn infobox(&self, maps: &ComponentKeymaps) -> Option<Info> {
        let keys = self.keys.pending_map(self.keymap(maps))?;
        let mut body = BTreeMap::<String, BTreeSet<_>>::new();
        for (&key, binding) in &keys.0 {
            let description = match binding {
                Binding::Actions(actions) if !actions.is_empty() => actions
                    .iter()
                    .map(|action| action.description())
                    .collect::<Vec<_>>()
                    .join("; "),
                Binding::Keys(_) => "More keys".to_owned(),
                _ => continue,
            };
            body.entry(description).or_default().insert(key);
        }
        let body: Vec<_> = body
            .into_iter()
            .map(|(description, keys)| {
                let keys = keys.iter().map(ToString::to_string).collect::<Vec<_>>();
                (keys.join(", "), description)
            })
            .collect();
        (!body.is_empty()).then(|| Info::new("File Explorer", &body))
    }

    pub fn has_pending_keys(&self) -> bool {
        self.keys.is_pending() || self.picker.has_pending_keys()
    }

    pub fn has_pending_render(&self) -> bool {
        self.search_pending || self.picker.has_pending_render()
    }

    fn cancel_search(&mut self) {
        self.search.cancel();
        self.search_pending = false;
        self.clear_pending_input();
    }

    fn clear_pending_input(&mut self) {
        self.pending_action = None;
        self.action_scheduled = false;
        self.pending_input.clear();
    }

    fn start_search(&mut self, cx: &mut Context) {
        let data = self.picker.editor_data();
        let root = data.root.clone();
        let index = data.tree.as_ref().unwrap().index.clone();
        let config = cx.editor.config().file_explorer.clone();
        let revision = cx.editor.file_operation_revision();
        let load = self.search.restart();
        let worker = load.clone();
        self.search_pending = true;
        cx.jobs.callback(async move {
            let rows = tokio::task::spawn_blocking(move || {
                super::file_explorer::search_index(&root, &config, || worker.is_canceled())
            })
            .await
            .unwrap_or_else(|error| Err(std::io::Error::other(error)));
            Ok(crate::job::Callback::ui(move |compositor, cx| {
                if load.is_canceled() {
                    return;
                }
                let Some(sidebar) = compositor
                    .find::<EditorView>()
                    .and_then(|view| view.sidebar.as_mut())
                else {
                    return;
                };
                if revision != cx.editor.file_operation_revision() {
                    sidebar.start_search(cx);
                    return;
                }
                sidebar.search_pending = false;
                match rows {
                    Ok(rows) => *index.lock().unwrap() = Some(rows),
                    Err(error) => {
                        sidebar.clear_pending_input();
                        cx.editor.set_error(|| error.to_string());
                        return;
                    }
                }
                sidebar.picker.refresh_query_options(cx.editor, true);
            }))
        });
    }

    pub fn update(&mut self, area: Rect, cx: &mut Context) {
        self.picker.update(area, cx);
        if self.search_pending
            || self.picker.has_pending_render()
            || (self.pending_action.is_none() && self.pending_input.is_empty())
            || self.action_scheduled
        {
            return;
        }
        // Keep collecting input until the callback has applied the action and
        // established its target, which may be a file-operation prompt.
        self.action_scheduled = true;
        cx.jobs
            .sender()
            .enqueue(crate::job::Callback::ui(move |compositor, cx| {
                let Some(sidebar) = compositor
                    .find::<EditorView>()
                    .and_then(|view| view.sidebar.as_mut())
                else {
                    return;
                };
                sidebar.action_scheduled = false;
                let action = sidebar.pending_action.take();
                let input = std::mem::take(&mut sidebar.pending_input);
                let result = action.map(|action| sidebar.picker_action(action, cx));
                if let Some(EventResult::Consumed(Some(callback))) = result {
                    callback(compositor, cx);
                }
                for (event, macro_playback) in input {
                    if cx.editor.should_close() {
                        break;
                    }
                    if macro_playback.is_empty() {
                        compositor.handle_event(&event, cx);
                    } else {
                        let previous =
                            std::mem::replace(&mut cx.interaction.macro_replaying, macro_playback);
                        compositor.replay_event(&event, cx);
                        cx.interaction.macro_replaying = previous;
                    }
                }
            }));
    }

    fn picker_action(&mut self, action: PickerAction, cx: &mut Context) -> EventResult {
        let result = self.picker.execute_actions(&[action], cx);
        if !matches!(result, EventResult::Consumed(Some(_))) && action == PickerAction::Accept {
            self.state = State::Navigating;
            self.reset_input();
        }
        result
    }

    pub fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        if self.pending_action.is_some()
            || !self.pending_input.is_empty()
            || (self.state == State::Navigating && self.picker.has_pending_render())
        {
            let macro_playback = cx.interaction.macro_replaying.clone();
            // Deferred keys are recorded when dispatched, after the accepted
            // file or prompt has become their input target.
            if macro_playback.is_empty()
                && matches!(event, Event::Key(_))
                && let Some((_, keys)) = &mut cx.interaction.macro_recording
            {
                keys.pop();
            }
            self.pending_input.push((event.clone(), macro_playback));
            return EventResult::Consumed(None);
        }
        let Event::Key(key) = event else {
            self.keys.clear();
            return if self.state == State::Filtering || matches!(event, Event::Mouse(_)) {
                self.picker.handle_event(event, cx)
            } else {
                EventResult::Consumed(None)
            };
        };
        cx.editor.reset_idle_timer();
        cx.editor.status_msg = None;
        if self.picker.has_pending_keys() {
            return self.picker.handle_event(event, cx);
        }
        let map = self.keymap(&cx.config.current.component_keys);
        let actions = match self.keys.lookup(map, *key) {
            Lookup::Pending => return EventResult::Consumed(None),
            Lookup::Unbound if self.state == State::Filtering => {
                self.pending_action = None;
                return self.picker.prompt_handle_event(event, cx);
            }
            Lookup::Unbound => return EventResult::Consumed(None),
            Lookup::Actions(actions) => actions.to_vec(),
        };
        for action in actions {
            match action {
                SidebarAction::Filter => {
                    self.picker.clear_filter(cx.editor);
                    self.state = State::Filtering;
                    self.picker.set_title("File Explorer (filter)");
                    self.start_search(cx);
                    return EventResult::Consumed(None);
                }
                SidebarAction::FocusEditor => self.focus_buffer(),
                SidebarAction::Hide => self.hide(),
                SidebarAction::NextView | SidebarAction::PreviousView => {
                    let view = if action == SidebarAction::NextView {
                        cx.editor.tree.views().next().map(|(view, _)| view.id)
                    } else {
                        cx.editor.tree.views().last().map(|(view, _)| view.id)
                    };
                    if let Some(view) = view {
                        cx.editor.focus(view);
                    }
                    self.focus_buffer();
                }
                SidebarAction::Picker(PickerAction::Cancel)
                    if self.state == State::Filtering
                        || self.picker.editor_data().tree.as_ref().unwrap().searching =>
                {
                    self.cancel_search();
                    self.picker.clear_filter(cx.editor);
                    self.picker
                        .editor_data()
                        .tree
                        .as_ref()
                        .unwrap()
                        .clear_index();
                    self.focus();
                }
                SidebarAction::Picker(PickerAction::Accept) if self.state == State::Filtering => {
                    self.state = State::Navigating;
                    self.reset_input();
                }
                SidebarAction::Picker(action) => {
                    if (self.search_pending || self.picker.has_pending_render())
                        && matches!(
                            action,
                            PickerAction::Accept
                                | PickerAction::AcceptKeepOpen
                                | PickerAction::AcceptHorizontalSplit
                                | PickerAction::AcceptVerticalSplit
                                | PickerAction::Item(_)
                                | PickerAction::PopulateQuicklist
                        )
                    {
                        self.pending_action = Some(action);
                        return EventResult::Consumed(None);
                    }
                    let result = self.picker_action(action, cx);
                    if matches!(result, EventResult::Consumed(Some(_)))
                        || action == PickerAction::Accept
                    {
                        return result;
                    }
                    continue;
                }
            }
            return EventResult::Consumed(None);
        }
        EventResult::Consumed(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shrinking_returns_focus_without_losing_the_visible_sidebar() -> anyhow::Result<()> {
        use crate::{application::TestApplication, config::Config, job::Jobs, keymap::Keymaps};
        let mut config = Config::default();
        config.editor.files.watch.enable = false;
        config.editor.files.watch.watch_vcs = false;
        let mut app = TestApplication::new(
            Default::default(),
            config.clone(),
            language::syntax::Loader::default(),
            workspace::trust::WorkspaceTrust::fully_trusted(loader::workspace_trust_dir()),
        )?;
        let mut view = EditorView::new(Keymaps::new(Box::new(arc_swap::access::Constant(
            crate::keymap::default(),
        ))));
        view.focus_sidebar(&mut app.editor);
        let mut jobs = Jobs::new();
        let updates = tokio::sync::mpsc::unbounded_channel().0;
        let mut cx = Context {
            config: crate::config::Context {
                current: &config,
                updates: &updates,
            },
            editor: &mut app.editor,
            interaction: &mut app.interaction,
            scroll: None,
            jobs: &mut jobs,
            image_picker: None,
            is_cursor_owner: true,
        };
        view.update(Rect::new(0, 0, 80, 24), &mut cx);
        assert!(view.sidebar.as_ref().unwrap().focused());
        assert!(cx.editor.tree.area().x > 0);
        for (width, height) in [(20, 24), (80, 3), (0, 0)] {
            let area = Rect::new(0, 0, width, height);
            view.update_and_render(area, &mut tui::buffer::Buffer::empty(area), &mut cx);
            assert!(!view.sidebar.as_ref().unwrap().focused());
            assert!(view.sidebar.as_ref().unwrap().visible());
            assert_eq!(cx.editor.tree.area().x, 0);
            assert!(!view.has_pending_render());
        }
        view.update(Rect::new(0, 0, 80, 24), &mut cx);
        assert!(cx.editor.tree.area().x > 0);
        assert!(!view.sidebar.as_ref().unwrap().focused());
        Ok(())
    }

    #[test]
    fn dock_respects_application_chrome_and_nonzero_origins() {
        for bufferline in [false, true] {
            for global in [false, true] {
                let mut layout = ApplicationLayout::new(
                    Rect::new(3, 5, 120, 30),
                    bufferline,
                    global,
                    Default::default(),
                );
                let original = layout;
                let sidebar = Sidebar::reserve_space(&mut layout).unwrap();
                assert_eq!(sidebar, Rect::new(3, 5, 36, original.editor.bottom() - 5));
                assert_eq!(sidebar.right(), layout.editor.x);
                assert_eq!(layout.editor.right(), original.editor.right());
                assert_eq!(layout.commandline, original.commandline);
                assert_eq!(layout.statusline, original.statusline);
                if let Some(bufferline) = layout.bufferline {
                    assert_eq!(bufferline.x, layout.editor.x);
                    assert_eq!(bufferline.width, layout.editor.width);
                }
            }
        }
    }

    #[test]
    fn tiny_terminals_keep_all_available_space_for_documents() {
        for width in [0, 1, 20, 44, 45, 80] {
            for height in [0, 1, 3, 4, 24] {
                let mut layout = ApplicationLayout::new(
                    Rect::new(3, 5, width, height),
                    false,
                    true,
                    Default::default(),
                );
                let original = layout;
                match Sidebar::reserve_space(&mut layout) {
                    Some(sidebar) => {
                        assert!(layout.editor.width >= 24);
                        assert_eq!(sidebar.width + layout.editor.width, width);
                        assert_eq!(sidebar.right(), layout.editor.x);
                    }
                    None => assert_eq!(layout, original),
                }
            }
        }
    }
}
