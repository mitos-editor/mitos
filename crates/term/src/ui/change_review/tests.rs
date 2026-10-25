use super::*;

#[test]
fn row_highlights_support_rgb_terminal_palettes_and_explicit_overrides() {
    use view::graphics::Color;
    let theme = theme::Theme::from(
        toml::from_str::<toml::Value>(
            r##"
        "ui.background" = { bg = "#101010" }
        "diff.plus" = "#90d090"
        "diff.minus.line" = { bg = "#402020" }
    "##,
        )
        .unwrap(),
    );
    assert_eq!(
        line_style(&theme, ReviewLineKind::Added).bg,
        Some(Color::Rgb(32, 40, 32))
    );
    assert_eq!(
        line_style(&theme, ReviewLineKind::Removed).bg,
        Some(Color::Rgb(64, 32, 32))
    );
    let theme = theme::Theme::from(
        toml::from_str::<toml::Value>(
            r##"
        "ui.selection" = { fg = "gray", modifiers = ["reversed"] }
        "diff.plus" = "green"
    "##,
        )
        .unwrap(),
    );
    assert_eq!(
        line_style(&theme, ReviewLineKind::Added).bg,
        Some(Color::DarkGray)
    );
}

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
        .any(|span| span.content == "new" && span.style == changed));
    let spans = source_spans("λ\tnew\u{1b}end", ranges, 3, 2, normal, changed);
    assert_eq!(
        spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>(),
        "ew"
    );
    assert!(spans.iter().all(|span| span.style == changed));
}
