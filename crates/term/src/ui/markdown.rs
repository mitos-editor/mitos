use crate::{
    compositor::{Component, Context},
    ui::panel,
};
use arc_swap::ArcSwap;
use tui::{
    buffer::Buffer as Surface,
    text::{Line, Span, Text},
};

use std::sync::Arc;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use editor_core::{highlight::OverlayHighlights, RopeSlice};
use language::{syntax, Syntax};
use theme::Modifier;
use theme::Theme;
use view::graphics::{Rect, Style};

fn styled_multiline_text<'a>(text: &str, style: Style) -> Text<'a> {
    let spans: Vec<_> = text
        .lines()
        .map(|line| Span::styled(line.to_string(), style))
        .map(Line::from)
        .collect();
    Text::from(spans)
}

pub fn highlighted_code_block<'a>(
    text: &str,
    language: &str,
    theme: Option<&Theme>,
    loader: &syntax::Loader,
    // Optional overlay highlights to mix in with the syntax highlights.
    //
    // Note that `OverlayHighlights` is typically used with char indexing but the only caller
    // which passes this parameter currently passes **byte indices** instead.
    additional_highlight_spans: Option<OverlayHighlights>,
) -> Text<'a> {
    let mut spans = Vec::new();
    let mut lines = Vec::new();

    let get_theme = |key: &str| -> Style { theme.map(|t| t.get(key)).unwrap_or_default() };
    let text_style = get_theme(Markdown::TEXT_STYLE);
    let code_style = get_theme(Markdown::BLOCK_STYLE);

    let theme = match theme {
        Some(t) => t,
        None => return styled_multiline_text(text, code_style),
    };

    let ropeslice = RopeSlice::from(text);
    let Some(syntax) = loader
        .language_for_match(RopeSlice::from(language))
        .and_then(|lang| Syntax::new(ropeslice, lang, loader).ok())
    else {
        return styled_multiline_text(text, code_style);
    };

    for (range, style) in super::highlights::styled_spans(
        Some(syntax.highlighter(ropeslice, loader, ..)),
        additional_highlight_spans,
        theme,
        text_style,
        text.len(),
    ) {
        for fragment in text[range].split_inclusive('\n') {
            let line = fragment.strip_suffix('\n');
            spans.push(Span::styled(
                line.unwrap_or(fragment).replace('\t', "    "),
                style,
            ));
            if line.is_some() {
                lines.push(Line::from(std::mem::take(&mut spans)));
            }
        }
    }

    if !spans.is_empty() {
        let spans = std::mem::take(&mut spans);
        lines.push(Line::from(spans));
    }

    Text::from(lines)
}

pub struct Markdown {
    contents: String,

    config_loader: Arc<ArcSwap<syntax::Loader>>,
}

// TODO: pre-render and self reference via Pin
// better yet, just use Tendril + subtendril for references

impl Markdown {
    const TEXT_STYLE: &'static str = "ui.text";
    const BLOCK_STYLE: &'static str = "markup.raw.inline";
    const RULE_STYLE: &'static str = "punctuation.special";
    const UNNUMBERED_LIST_STYLE: &'static str = "markup.list.unnumbered";
    const NUMBERED_LIST_STYLE: &'static str = "markup.list.numbered";
    const HEADING_STYLES: [&'static str; 6] = [
        "markup.heading.1",
        "markup.heading.2",
        "markup.heading.3",
        "markup.heading.4",
        "markup.heading.5",
        "markup.heading.6",
    ];
    const INDENT: &'static str = "  ";

    pub fn new(contents: String, config_loader: Arc<ArcSwap<syntax::Loader>>) -> Self {
        Self {
            contents,
            config_loader,
        }
    }

    pub fn parse(&self, theme: Option<&Theme>) -> tui::text::Text<'_> {
        fn push_line<'a>(spans: &mut Vec<Span<'a>>, lines: &mut Vec<Line<'a>>) {
            let spans = std::mem::take(spans);
            if !spans.is_empty() {
                lines.push(Line::from(spans));
            }
        }

        let mut options = Options::empty();
        options.insert(Options::ENABLE_STRIKETHROUGH);
        let parser = Parser::new_ext(&self.contents, options);

        // TODO: if possible, render links as terminal hyperlinks: https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda
        let mut tags = Vec::new();
        let mut spans = Vec::new();
        let mut lines = Vec::new();
        let mut list_stack = Vec::new();

        let get_indent = |level: usize| {
            if level < 1 {
                String::new()
            } else {
                Self::INDENT.repeat(level - 1)
            }
        };

        let get_theme = |key: &str| -> Style { theme.map(|t| t.get(key)).unwrap_or_default() };
        let text_style = get_theme(Self::TEXT_STYLE);
        let code_style = get_theme(Self::BLOCK_STYLE);
        let numbered_list_style = get_theme(Self::NUMBERED_LIST_STYLE);
        let unnumbered_list_style = get_theme(Self::UNNUMBERED_LIST_STYLE);
        let rule_style = get_theme(Self::RULE_STYLE);
        let heading_styles: Vec<Style> = Self::HEADING_STYLES
            .iter()
            .map(|key| get_theme(key))
            .collect();

        // Transform text in `<code>` blocks into `Event::Code`
        let mut in_code = false;
        let parser = parser.filter_map(|event| match event {
            Event::Html(tag)
                if tag.starts_with("<code") && matches!(tag.chars().nth(5), Some(' ' | '>')) =>
            {
                in_code = true;
                None
            }
            Event::Html(tag) if *tag == *"</code>" => {
                in_code = false;
                None
            }
            Event::Text(text) if in_code => Some(Event::Code(text)),
            _ => Some(event),
        });

        for event in parser {
            match event {
                Event::Start(Tag::List(list)) => {
                    // if the list stack is not empty this is a sub list, in that
                    // case we need to push the current line before proceeding
                    if !list_stack.is_empty() {
                        push_line(&mut spans, &mut lines);
                    }

                    list_stack.push(list);
                }
                Event::End(TagEnd::List(_)) => {
                    list_stack.pop();

                    // whenever top-level list closes, empty line
                    if list_stack.is_empty() {
                        lines.push(Line::default());
                    }
                }
                Event::Start(Tag::Item) => {
                    if list_stack.is_empty() {
                        log::warn!("markdown parsing error, list item without list");
                    }

                    tags.push(Tag::Item);

                    // get the appropriate bullet for the current list
                    let (bullet, bullet_style) = list_stack
                        .last()
                        .unwrap_or(&None) // use the '- ' bullet in case the list stack would be empty
                        .map_or((String::from("• "), unnumbered_list_style), |number| {
                            (format!("{}. ", number), numbered_list_style)
                        });

                    // increment the current list number if there is one
                    if let Some(v) = list_stack.last_mut().unwrap_or(&mut None).as_mut() {
                        *v += 1;
                    }

                    let prefix = get_indent(list_stack.len()) + bullet.as_str();
                    spans.push(Span::styled(prefix, bullet_style));
                }
                Event::Start(tag) => {
                    tags.push(tag);
                    if spans.is_empty() && !list_stack.is_empty() {
                        // TODO: could push indent + 2 or 3 spaces to align with
                        // the rest of the list.
                        spans.push(Span::from(get_indent(list_stack.len())));
                    }
                }
                Event::End(tag) => {
                    tags.pop();
                    match tag {
                        TagEnd::Heading(_)
                        | TagEnd::Paragraph
                        | TagEnd::CodeBlock
                        | TagEnd::Item => {
                            push_line(&mut spans, &mut lines);
                        }
                        _ => (),
                    }

                    // whenever heading, code block or paragraph closes, empty line
                    match tag {
                        TagEnd::Heading(_) | TagEnd::Paragraph | TagEnd::CodeBlock => {
                            lines.push(Line::default());
                        }
                        _ => (),
                    }
                }
                Event::Text(text) => {
                    if let Some(Tag::CodeBlock(kind)) = tags.last() {
                        let language = match kind {
                            CodeBlockKind::Fenced(language) => language,
                            CodeBlockKind::Indented => "",
                        };
                        let tui_text = highlighted_code_block(
                            &text,
                            language,
                            theme,
                            &self.config_loader.load(),
                            None,
                        );
                        lines.extend(tui_text.lines);
                    } else {
                        let style = match tags.last() {
                            Some(Tag::Heading { level, .. }) => match level {
                                HeadingLevel::H1 => heading_styles[0],
                                HeadingLevel::H2 => heading_styles[1],
                                HeadingLevel::H3 => heading_styles[2],
                                HeadingLevel::H4 => heading_styles[3],
                                HeadingLevel::H5 => heading_styles[4],
                                HeadingLevel::H6 => heading_styles[5],
                            },
                            Some(Tag::Emphasis) => text_style.add_modifier(Modifier::ITALIC),
                            Some(Tag::Strong) => text_style.add_modifier(Modifier::BOLD),
                            Some(Tag::Strikethrough) => {
                                text_style.add_modifier(Modifier::CROSSED_OUT)
                            }
                            _ => text_style,
                        };
                        spans.push(Span::styled(text, style));
                    }
                }
                Event::Code(text) | Event::Html(text) => {
                    spans.push(Span::styled(text, code_style));
                }
                Event::SoftBreak | Event::HardBreak => {
                    push_line(&mut spans, &mut lines);
                    if !list_stack.is_empty() {
                        // TODO: could push indent + 2 or 3 spaces to align with
                        // the rest of the list.
                        spans.push(Span::from(get_indent(list_stack.len())));
                    }
                }
                Event::Rule => {
                    lines.push(Line::from(Span::styled("───", rule_style)));
                    lines.push(Line::default());
                }
                // TaskListMarker(bool) true if checked
                _ => {
                    log::warn!("unhandled markdown event {:?}", event);
                }
            }
            // build up a vec of Paragraph tui widgets
        }

        if !spans.is_empty() {
            lines.push(Line::from(spans));
        }

        // if last line is empty, remove it
        if let Some(line) = lines.last()
            && line.spans.is_empty()
        {
            lines.pop();
        }

        Text::from(lines)
    }
}

impl Component for Markdown {
    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        use tui::widgets::Widget;

        let text = self.parse(Some(&cx.editor.theme));

        let par =
            crate::ui::text::paragraph(text).scroll((cx.scroll.unwrap_or_default() as u16, 0));

        par.render(panel::content_area(area), surface);
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        let contents = crate::ui::text::paragraph(self.parse(None));

        // TODO: account for tab width
        let max_text_width = viewport.0.saturating_sub(panel::HORIZONTAL_INSET).min(120);
        let (width, height) = crate::ui::text::required_size(&contents, max_text_width);

        Some((
            width + panel::HORIZONTAL_INSET,
            height + panel::VERTICAL_INSET,
        ))
    }
}

#[cfg(all(test, feature = "integration"))]
mod tests {
    use super::*;

    #[test]
    fn code_spans_preserve_unicode_overlays_tabs_and_line_endings() {
        let loader = syntax::Loader::new(
            toml::from_str(
                r#"[[language]]
name = "json"
scope = "source.json"
file-types = ["json"]
"#,
            )
            .unwrap(),
            loader::syntax::Resources::default(),
        )
        .unwrap();
        let theme = Theme::from(toml::Value::Table(toml::toml! {
            "ui.text" = "white"
            "ui.selection" = { bg = "blue" }
            "string" = "red"
        }));
        loader.set_scopes(theme.scopes().to_vec());
        let selected = theme.find_highlight_exact("ui.selection").unwrap();
        let output = highlighted_code_block(
            "{\n\t\"é\": 1\n}\n",
            "json",
            Some(&theme),
            &loader,
            Some(OverlayHighlights::single(selected, 4..6)),
        );
        let lines: Vec<String> = output
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        assert_eq!(lines, ["{", "    \"é\": 1", "}"]);
        let span = output.lines[1]
            .spans
            .iter()
            .find(|span| span.content == "é")
            .unwrap();
        assert_eq!(span.style.bg, Some(tui::style::Color::Blue));
    }
}
