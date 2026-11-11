//! One resize interaction for document splits and the retained file explorer.

use std::num::NonZeroUsize;

use view::{info::Info, tree, Editor};

use super::{canonicalize_key, EditorView};
use crate::{
    compositor::{Context, Event},
    key,
};

pub(super) struct Resize {
    pub sidebar: bool,
    sizes: tree::SplitSizes,
    sidebar_width: Option<u16>,
    count: Option<NonZeroUsize>,
}

impl Resize {
    pub(super) fn keymap_context(&self) -> crate::keymap::inspect::Context {
        use crate::keymap::inspect::{Context, Definition, Fallback};
        let mut bindings = Vec::new();
        for (key, action) in [
            ("h", "resize left"),
            ("left", "resize left"),
            ("l", "resize right"),
            ("right", "resize right"),
            ("j", "resize down"),
            ("down", "resize down"),
            ("k", "resize up"),
            ("up", "resize up"),
            ("=", "reset sizes"),
            ("ret", "confirm resize"),
            ("esc", "cancel resize"),
            ("C-c", "cancel resize"),
        ] {
            if self.sidebar && matches!(key, "j" | "down" | "k" | "up") {
                continue;
            }
            bindings.push(Definition {
                table: "resize".into(),
                keys: vec![key.parse().unwrap()],
                action: action.into(),
                description: action.into(),
                prefix: false,
            });
        }
        for digit in '0'..='9' {
            if self.count.is_some() || digit != '0' {
                bindings.push(Definition {
                    table: "resize".into(),
                    keys: vec![ui_core::input::KeyEvent {
                        code: ui_core::keyboard::KeyCode::Char(digit),
                        modifiers: ui_core::keyboard::KeyModifiers::NONE,
                    }],
                    action: "resize count".into(),
                    description: "Set resize distance".into(),
                    prefix: false,
                });
            }
        }
        let mut context = Context::new("Resize", bindings);
        context.fallback = Fallback::Consume;
        context
    }
}

impl EditorView {
    pub(crate) fn start_resize(&mut self, editor: &mut Editor, count: Option<NonZeroUsize>) {
        self.resizing = Some(Resize {
            sidebar: self.sidebar_focused(),
            sizes: editor.tree.split_sizes(),
            sidebar_width: self.sidebar.as_ref().and_then(|sidebar| sidebar.width),
            count,
        });
        let mut hints = vec![
            ("h, left", "Resize window left"),
            ("l, right", "Resize window right"),
        ];
        if !self.sidebar_focused() {
            hints.extend([
                ("j, down", "Resize window down"),
                ("k, up", "Resize window up"),
            ]);
        }
        hints.extend([
            ("=", "Reset window sizes"),
            ("ret", "Confirm resizing"),
            ("esc", "Cancel resizing"),
        ]);
        editor.autoinfo = Some(Info::new("Resize window", &hints));
    }

    pub(super) fn finish_resize(&mut self, editor: &mut Editor, cancel: bool) {
        let Some(resize) = self.resizing.take() else {
            return;
        };
        if cancel {
            editor.restore_split_sizes(resize.sizes);
            if let Some(sidebar) = &mut self.sidebar {
                sidebar.width = resize.sidebar_width;
            }
        }
        editor.autoinfo = None;
    }

    pub(super) fn handle_resize_event(&mut self, event: &Event, cx: &mut Context) -> bool {
        let Some(resize) = &mut self.resizing else {
            return false;
        };
        let Event::Key(mut key) = *event else {
            if matches!(event, Event::Mouse(_)) && !cx.editor.config().mouse.enable {
                return false;
            }
            // Resize/focus notifications and mouse motion do not end the interaction.
            if matches!(event, Event::Paste(_))
                || matches!(event, Event::Mouse(mouse) if mouse.kind != ui_core::input::MouseEventKind::Moved)
            {
                self.finish_resize(cx.editor, true);
                cx.interaction.count = None;
            }
            return false;
        };
        canonicalize_key(&mut key);
        cx.editor.reset_idle_timer();
        cx.editor.status_msg = None;
        match key {
            key!(Enter) | key!(Esc) | key!(Null) | crate::ctrl!('c') => {
                self.finish_resize(cx.editor, key != key!(Enter));
                cx.interaction.count = None;
                return true;
            }
            key!(digit @ '0'..='9') if resize.count.is_some() || digit != '0' => {
                let next = resize.count.map_or(0, NonZeroUsize::get) * 10
                    + digit.to_digit(10).unwrap() as usize;
                if next <= 100_000_000 {
                    resize.count = NonZeroUsize::new(next);
                }
                cx.interaction.count = resize.count;
                return true;
            }
            _ => {}
        }
        let count = resize.count.take();
        cx.interaction.count = None;
        let direction = match key {
            key!('h') | key!(Left) => Some(tree::Direction::Left),
            key!('l') | key!(Right) => Some(tree::Direction::Right),
            key!('j') | key!(Down) => Some(tree::Direction::Down),
            key!('k') | key!(Up) => Some(tree::Direction::Up),
            _ => None,
        };
        if let Some(direction) = direction {
            let default = match direction {
                tree::Direction::Left | tree::Direction::Right => 5,
                tree::Direction::Up | tree::Direction::Down => 2,
            };
            self.resize_direction(
                cx.editor,
                direction,
                count.map_or(default, NonZeroUsize::get),
            );
        } else if key == key!('=') {
            cx.editor.equalize_views();
            if self.sidebar_area.is_some()
                && let Some(sidebar) = &mut self.sidebar
            {
                sidebar.width = None;
            }
        }
        true
    }

    pub(crate) fn resize_direction(
        &mut self,
        editor: &mut Editor,
        direction: tree::Direction,
        cells: usize,
    ) {
        use tree::Direction::{Left, Right};
        let sidebar_focused = self.sidebar_focused();
        if !matches!(direction, Left | Right) {
            if !sidebar_focused {
                editor.resize_view_direction(direction, cells);
            }
            return;
        }
        let pane = view::view!(editor).area;
        let documents = editor.tree.area();
        // A left-edge document has the sidebar as its immediate left neighbor.
        // With no vertical split ancestor, both horizontal directions move that
        // boundary. A constrained buffer boundary never falls through to it.
        let use_sidebar = self.sidebar_area.is_some()
            && (sidebar_focused
                || (matches!(direction, Left) && pane.x == documents.x)
                || pane.width == documents.width);
        if use_sidebar {
            let area = self.sidebar_area.unwrap();
            let total = area.width.saturating_add(documents.width);
            let grow = matches!(direction, Right);
            let delta = isize::try_from(cells).unwrap_or(isize::MAX);
            self.sidebar
                .as_mut()
                .unwrap()
                .resize_width(total, if grow { delta } else { -delta });
        } else {
            editor.resize_view_direction(direction, cells);
        }
    }
}
