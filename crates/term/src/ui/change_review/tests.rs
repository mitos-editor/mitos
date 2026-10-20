use super::*;

#[test]
fn source_emphasis_survives_unicode_scrolling_and_control_sanitization() {
    let normal = view::graphics::Style::default();
    let changed = normal.add_modifier(view::graphics::Modifier::BOLD);
    let range = 3..6;
    let ranges = Some(std::slice::from_ref(&range));
    let spans = source_spans("λ\tnew\u{1b}end", ranges, 1, 20, normal, changed);
    assert_eq!(
        spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>(),
        "    new�end"
    );
    assert!(spans
        .iter()
        .any(|span| span.content == "new" && span.style == changed.into()));
    let spans = source_spans("λ\tnew\u{1b}end", ranges, 3, 2, normal, changed);
    assert_eq!(
        spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>(),
        "ew"
    );
    assert!(spans.iter().all(|span| span.style == changed.into()));
}
