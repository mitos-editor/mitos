//! Shared style decisions; each native backend retains its own output encoding.
use ratatui::style::Modifier;

#[derive(Clone, Copy)]
pub(super) enum ModifierChange {
    Reverse(bool),
    NormalIntensity,
    Bold,
    Dim,
    Italic(bool),
    CrossedOut(bool),
    NoBlink,
    SlowBlink,
    RapidBlink,
    Hidden(bool),
}

pub(super) fn modifier_changes(
    from: Modifier,
    to: Modifier,
) -> impl Iterator<Item = ModifierChange> {
    use ModifierChange::*;
    let removed = from - to;
    let added = to - from;
    [
        removed
            .contains(Modifier::REVERSED)
            .then_some(Reverse(false)),
        removed.contains(Modifier::BOLD).then_some(NormalIntensity),
        (removed.contains(Modifier::BOLD) && to.contains(Modifier::DIM)).then_some(Dim),
        removed.contains(Modifier::ITALIC).then_some(Italic(false)),
        removed.contains(Modifier::DIM).then_some(NormalIntensity),
        removed
            .contains(Modifier::CROSSED_OUT)
            .then_some(CrossedOut(false)),
        removed
            .intersects(Modifier::SLOW_BLINK | Modifier::RAPID_BLINK)
            .then_some(NoBlink),
        removed.contains(Modifier::HIDDEN).then_some(Hidden(false)),
        added.contains(Modifier::REVERSED).then_some(Reverse(true)),
        added.contains(Modifier::BOLD).then_some(Bold),
        added.contains(Modifier::ITALIC).then_some(Italic(true)),
        added.contains(Modifier::DIM).then_some(Dim),
        added
            .contains(Modifier::CROSSED_OUT)
            .then_some(CrossedOut(true)),
        added.contains(Modifier::SLOW_BLINK).then_some(SlowBlink),
        added.contains(Modifier::RAPID_BLINK).then_some(RapidBlink),
        added.contains(Modifier::HIDDEN).then_some(Hidden(true)),
    ]
    .into_iter()
    .flatten()
}
