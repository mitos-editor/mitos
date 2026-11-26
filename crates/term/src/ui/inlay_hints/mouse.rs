//! Mouse interaction starts only from proven displayed hint-label spans.

use super::*;
use text_layout::DisplayPosition;
use view::{features::inlay_hints::InlayHintHit, ViewId};

const LOADING: &str = "inlay-hint-hover-loading";
const DETAILS: &str = "inlay-hint-hover-details";

#[derive(Default)]
pub(crate) struct MouseHints {
    hovered: Option<InlayHintHit>,
    /// Prevent a hint click from continuing as text selection or clipboard yank.
    pub(crate) pressed: bool,
}

impl MouseHints {
    pub(crate) fn clear(&mut self) {
        self.hovered = None;
    }
    pub(crate) fn is_hovering(&self) -> bool {
        self.hovered.is_some()
    }
}

pub(crate) fn hit_at(editor: &Editor, row: u16, column: u16) -> Option<(ViewId, InlayHintHit)> {
    editor.tree.views().find_map(|(view, _)| {
        inlay_hints::hint_at_screen_coords(editor, view.id, row, column).map(|hit| (view.id, hit))
    })
}

pub(crate) fn remove_hover(compositor: &mut Compositor) -> bool {
    let loading = compositor.remove(LOADING).is_some();
    let details = compositor.remove(DETAILS).is_some();
    loading || details
}

fn has_other_layers(compositor: &mut Compositor) -> bool {
    let own = usize::from(compositor.find_id::<Popup<PendingHint>>(LOADING).is_some())
        + usize::from(compositor.find_id::<Popup<HintDetails>>(DETAILS).is_some());
    compositor.layer_count() > 1 + own
}

fn over_popup(compositor: &mut Compositor, editor: &Editor, row: u16, column: u16) -> bool {
    let area = compositor
        .find_id::<Popup<HintDetails>>(DETAILS)
        .filter(|popup| popup.contents().resolved.is_current(editor))
        .map(|popup| popup.rendered_area())
        .or_else(|| {
            compositor
                .find_id::<Popup<PendingHint>>(LOADING)
                .filter(|popup| popup.contents().target.is_current(editor))
                .map(|popup| popup.rendered_area())
        });
    area.is_some_and(|area| {
        row >= area.top() && row < area.bottom() && column >= area.left() && column < area.right()
    })
}

pub(crate) fn moved(compositor: &mut Compositor, cx: &mut Context, row: u16, column: u16) {
    if has_other_layers(compositor) {
        if let Some(view) = compositor.find::<ui::EditorView>() {
            view.inlay_mouse.clear();
        }
        if remove_hover(compositor) {
            cx.editor.redraw().request();
        }
        return;
    }
    // Keep a tooltip readable while the pointer enters it; other movement is
    // still interpreted against the exact text drawn underneath the mouse.
    if over_popup(compositor, cx.editor, row, column) {
        return;
    }
    let hit = hit_at(cx.editor, row, column).map(|(_, hit)| hit);
    let Some(view) = compositor.find::<ui::EditorView>() else {
        return;
    };
    if let (Some(old), Some(hit)) = (&view.inlay_mouse.hovered, &hit)
        && old.target.same_hint(&hit.target)
        && old.label_part == hit.label_part
    {
        return;
    }
    if view.inlay_mouse.hovered.is_none() && hit.is_none() {
        return;
    }
    view.inlay_mouse.hovered = None;
    remove_hover(compositor);
    cx.editor.redraw().request();
    // Explicit menus, prompts and completion menus own the interaction while
    // present. Passive mouse movement must not replace them.
    if compositor.layer_count() != 1 {
        return;
    }
    let Some(hit) = hit else {
        return;
    };
    compositor
        .find::<ui::EditorView>()
        .unwrap()
        .inlay_mouse
        .hovered = Some(hit.clone());
    let anchor = DisplayPosition::new(usize::from(row), usize::from(column));
    request_with(
        compositor,
        cx,
        hit.target,
        LOADING,
        Some(anchor),
        true,
        move |compositor, cx, resolved| {
            if compositor.layer_count() != 1 {
                if let Some(view) = compositor.find::<ui::EditorView>() {
                    view.inlay_mouse.clear();
                }
                return;
            }
            compositor.replace_or_push(
                DETAILS,
                Popup::new(
                    DETAILS,
                    HintDetails::new(resolved, cx.editor, hit.label_part, DETAILS),
                )
                .anchor(Some(anchor))
                .ignore_escape_key(true)
                .with_border(ui::PopupBorder::Hover)
                .auto_close(true),
            );
        },
    );
}

pub(crate) fn clicked(
    compositor: &mut Compositor,
    cx: &mut Context,
    view: ViewId,
    row: u16,
    column: u16,
    follow: bool,
) {
    if let Some(editor) = compositor.find::<ui::EditorView>() {
        editor.inlay_mouse.clear();
    }
    remove_hover(compositor);
    cx.editor.focus(view);
    // Focusing another split invalidates the passive hover's old context.
    // Capture a fresh target before any ordinary text click can move selection.
    let Some(hit) = inlay_hints::hint_at_screen_coords(cx.editor, view, row, column) else {
        return;
    };
    let anchor = DisplayPosition::new(usize::from(row), usize::from(column));
    request_with(
        compositor,
        cx,
        hit.target,
        PendingHint::ID,
        Some(anchor),
        false,
        move |compositor, cx, resolved| {
            if follow {
                let actions = resolved.actions();
                let location = actions.iter().copied().find(|action| matches!(action,InlayHintAction::LabelLocation(index) if Some(*index)==hit.label_part));
                let command = actions.iter().copied().find(|action| matches!(action,InlayHintAction::LabelCommand(index) if Some(*index)==hit.label_part));
                if let Some(action) = location.or(command) {
                    if let Err(error) = resolved.execute(cx.editor, action, Action::Replace) {
                        cx.editor
                            .set_error(|| format!("Failed to use inlay hint: {error}"));
                    }
                } else {
                    show_details_at(
                        compositor,
                        cx.editor,
                        resolved,
                        hit.label_part,
                        Some(anchor),
                    );
                }
            } else {
                present_at(compositor, cx, resolved, hit.label_part, Some(anchor));
            }
        },
    );
}
