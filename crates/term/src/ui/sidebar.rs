//! A retained file browser beside, and independent of, document splits.

use view::{graphics::Rect, Editor};

use crate::{
    compositor::{Component, Context, Cursor, DrawContext, Event, EventResult},
    keymap::component::SidebarAction,
};
use ui_core::keymap::{KeymapState, Lookup};

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
}

impl Sidebar {
    pub fn new(editor: &Editor) -> std::io::Result<Self> {
        let picker = super::file_explorer(workspace::find_workspace().0, editor)?.embedded(
            |compositor, _| {
                if let Some(sidebar) = compositor
                    .find::<EditorView>()
                    .and_then(|editor| editor.sidebar.as_mut())
                {
                    sidebar.focus_buffer();
                }
            },
        );
        Ok(Self {
            picker,
            state: State::Navigating,
            keys: KeymapState::default(),
        })
    }

    pub fn focused(&self) -> bool {
        matches!(self.state, State::Navigating | State::Filtering)
    }

    pub fn visible(&self) -> bool {
        self.state != State::Hidden
    }

    pub fn focus(&mut self) {
        self.state = State::Navigating;
        self.reset_input();
    }

    pub fn focus_buffer(&mut self) {
        if self.visible() {
            self.state = State::Visible;
        }
        self.reset_input();
    }

    pub fn hide(&mut self) {
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

    pub fn has_pending_keys(&self) -> bool {
        self.keys.is_pending() || self.picker.has_pending_keys()
    }

    pub fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
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
        let maps = &cx.config.current.component_keys;
        let map = if self.state == State::Filtering {
            &maps.sidebar_filter
        } else {
            &maps.sidebar
        };
        let actions = match self.keys.lookup(map, *key) {
            Lookup::Pending => return EventResult::Consumed(None),
            Lookup::Unbound if self.state == State::Filtering => {
                return self.picker.prompt_handle_event(event, cx)
            }
            Lookup::Unbound => return EventResult::Consumed(None),
            Lookup::Actions(actions) => actions.to_vec(),
        };
        for action in actions {
            match action {
                SidebarAction::Filter => {
                    self.state = State::Filtering;
                    self.picker.set_title("File Explorer (filter)");
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
                SidebarAction::Picker(action) => {
                    let result = self.picker.execute_actions(&[action], cx);
                    if matches!(result, EventResult::Consumed(Some(_))) {
                        return result;
                    }
                    if matches!(action, crate::keymap::component::PickerAction::Accept) {
                        self.state = State::Navigating;
                        self.reset_input();
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
