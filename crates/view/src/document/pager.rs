//! Decode finite ANSI output into plain text and immutable style changes.

use super::{Document, DocumentSource};
use arc_swap::access::DynAccess;
use editor_config::Config;
use editor_core::Rope;
use language::syntax;
use std::sync::Arc;
use theme::{Color, Modifier, Style};

impl Document {
    pub(crate) fn pager(
        input: &str,
        config: Arc<dyn DynAccess<Config>>,
        loader: Arc<syntax::Loader>,
    ) -> Self {
        let mut output = Output::default();
        vte::Parser::new().advance(&mut output, input.as_bytes());
        let mut doc = Self::from(Rope::from(output.text), None, config, loader);
        doc.source = DocumentSource::Pager(output.styles);
        doc
    }
}

#[derive(Default)]
struct Output {
    text: String,
    styles: Vec<(usize, Style)>,
    position: usize,
    style: Style,
    overstrike: Option<(char, Style)>,
}

impl Output {
    fn push(&mut self, ch: char, style: Style) {
        if self.styles.last().is_none_or(|(_, last)| *last != style) {
            self.styles.push((self.position, style));
        }
        self.text.push(ch);
        self.position += 1;
    }

    fn backspace(&mut self) {
        // Overstrikes in man output operate on the preceding character, never
        // across a line boundary. Keep the original style for bold/underline.
        if let Some(ch) = self.text.chars().next_back().filter(|ch| *ch != '\n') {
            let style = self.styles.last().unwrap().1;
            self.text.pop();
            self.position -= 1;
            while self
                .styles
                .last()
                .is_some_and(|(pos, _)| *pos >= self.position)
            {
                self.styles.pop();
            }
            self.overstrike = Some((ch, style));
        }
    }
}

impl vte::Perform for Output {
    fn print(&mut self, mut ch: char) {
        if ch.is_control() {
            return;
        }
        let mut style = self.style;
        if let Some((previous, previous_style)) = self.overstrike.take() {
            if previous == ch {
                style = previous_style.patch(style).add_modifier(Modifier::BOLD);
            } else if previous == '_' || ch == '_' {
                if ch == '_' {
                    ch = previous;
                }
                style = previous_style
                    .patch(style)
                    .add_modifier(Modifier::UNDERLINED);
            }
        }
        self.push(ch, style);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | b'\t' => {
                self.overstrike = None;
                self.push(byte as char, self.style);
            }
            8 => self.backspace(),
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        if action != 'm' || ignore || !intermediates.is_empty() {
            return;
        }
        let mut params = params.iter();
        while let Some(param) = params.next() {
            let code = param[0];
            if matches!(code, 38 | 48 | 58) {
                let color = if param.len() > 1 {
                    extended_color(&param[1..])
                } else {
                    // Consume the whole color specification, even if a channel
                    // is invalid, so its values cannot become unrelated SGRs.
                    match params.next() {
                        Some([5]) => params.next().and_then(extended_index),
                        Some([2]) => {
                            let red = params.next();
                            let green = params.next();
                            let blue = params.next();
                            match (red, green, blue) {
                                (Some([r]), Some([g]), Some([b])) => rgb(*r, *g, *b),
                                _ => None,
                            }
                        }
                        _ => None,
                    }
                };
                if let Some(color) = color {
                    if code == 38 {
                        self.style.fg = Some(color);
                    } else if code == 48 {
                        self.style.bg = Some(color);
                    }
                }
                continue;
            }
            if param.len() != 1 {
                continue;
            }
            self.style = match code {
                0 => Style::default(),
                1 => self.style.add_modifier(Modifier::BOLD),
                2 => self.style.add_modifier(Modifier::DIM),
                3 => self.style.add_modifier(Modifier::ITALIC),
                4 => self.style.add_modifier(Modifier::UNDERLINED),
                7 => self.style.add_modifier(Modifier::REVERSED),
                9 => self.style.add_modifier(Modifier::CROSSED_OUT),
                22 => self.style.remove_modifier(Modifier::BOLD | Modifier::DIM),
                23 => self.style.remove_modifier(Modifier::ITALIC),
                24 => self.style.remove_modifier(Modifier::UNDERLINED),
                27 => self.style.remove_modifier(Modifier::REVERSED),
                29 => self.style.remove_modifier(Modifier::CROSSED_OUT),
                30..=37 => self.style.fg(Color::Indexed((code - 30) as u8)),
                40..=47 => self.style.bg(Color::Indexed((code - 40) as u8)),
                90..=97 => self.style.fg(Color::Indexed((code - 90 + 8) as u8)),
                100..=107 => self.style.bg(Color::Indexed((code - 100 + 8) as u8)),
                39 => Style {
                    fg: None,
                    ..self.style
                },
                49 => Style {
                    bg: None,
                    ..self.style
                },
                _ => self.style,
            };
        }
    }
}

fn extended_index(params: &[u16]) -> Option<Color> {
    match params {
        [index] => Some(Color::Indexed((*index).try_into().ok()?)),
        _ => None,
    }
}

fn extended_color(params: &[u16]) -> Option<Color> {
    match params {
        [5, index] => extended_index(&[*index]),
        [2, r, g, b] | [2, 0, r, g, b] => rgb(*r, *g, *b),
        _ => None,
    }
}

fn rgb(red: u16, green: u16, blue: u16) -> Option<Color> {
    Some(Color::Rgb(
        red.try_into().ok()?,
        green.try_into().ok()?,
        blue.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Output {
        let mut output = Output::default();
        vte::Parser::new().advance(&mut output, input.as_bytes());
        output
    }

    #[test]
    fn colors_and_resets_use_decoded_character_offsets() {
        let output = parse("é\x1b[31;1m赤\x1b[38;5;200;48;2;1;2;3mX\x1b[22;39;49mY\x1b[mZ");
        assert_eq!(output.text, "é赤XYZ");
        assert_eq!(output.styles[0], (0, Style::default()));
        assert_eq!(
            output.styles[1],
            (
                1,
                Style::default()
                    .fg(Color::Indexed(1))
                    .add_modifier(Modifier::BOLD)
            )
        );
        assert_eq!(output.styles[2].0, 2);
        assert_eq!(output.styles[2].1.fg, Some(Color::Indexed(200)));
        assert_eq!(output.styles[2].1.bg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(output.styles[3].1.fg, None);
        assert_eq!(output.styles[3].1.bg, None);
        assert!(!output.styles[3].1.add_modifier.contains(Modifier::BOLD));
        assert_eq!(output.styles[4], (4, Style::default()));
    }

    #[test]
    fn colon_colors_modifiers_and_invalid_channels() {
        let output =
            parse("\x1b[38:2::12:34:56;48:5:42;2;3;4;7;9mA\x1b[23;24;27;29mB\x1b[38;2;999;1;2mC");
        assert_eq!(output.text, "ABC");
        let style = output.styles[0].1;
        assert_eq!(style.fg, Some(Color::Rgb(12, 34, 56)));
        assert_eq!(style.bg, Some(Color::Indexed(42)));
        assert!(style.add_modifier.contains(
            Modifier::DIM
                | Modifier::ITALIC
                | Modifier::UNDERLINED
                | Modifier::REVERSED
                | Modifier::CROSSED_OUT
        ));
        assert_eq!(output.styles.len(), 2);
        assert_eq!(output.styles[1].1.add_modifier, Modifier::DIM);
        assert_eq!(
            parse("\x1b[58;2;1;2;3mX").styles,
            vec![(0, Style::default())]
        );
    }

    #[test]
    fn control_sequences_are_consumed_without_side_effects() {
        let output = parse("a\x1b]0;title\x07\x1b]52;c;clipboard\x1b\\\x1bPignored\x1b\\\x1b[2J\x1b[?31mb\r\n\t\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\\x1b[31");
        assert_eq!(output.text, "ab\n\tlink");
        assert_eq!(output.styles, vec![(0, Style::default())]);
    }

    #[test]
    fn man_overstrikes_preserve_styles_and_do_not_cross_lines() {
        let output = parse("A\x08A _\x08é z\x08_\n\x08\x1b[31mB\x08B\x1b[0m!");
        assert_eq!(output.text, "A é z\nB!");
        assert!(output.styles[0].1.add_modifier.contains(Modifier::BOLD));
        assert_eq!(output.styles[2].0, 2);
        assert!(output.styles[2]
            .1
            .add_modifier
            .contains(Modifier::UNDERLINED));
        let (_, style) = output.styles.iter().find(|(pos, _)| *pos == 6).unwrap();
        assert_eq!(style.fg, Some(Color::Indexed(1)));
        assert!(style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(output.styles.last(), Some(&(7, Style::default())));
    }

    #[tokio::test]
    async fn pager_origin_prevents_edits_history_and_forced_saves() {
        let (template, view) = crate::editing::tests::document_and_view("#[|]#\n");
        let mut doc = Document::pager(
            "\x1b[31mred\x1b[m",
            template.config.clone(),
            template.syntax_loader.clone(),
        );
        doc.ensure_view_init(view.id);
        assert_eq!(doc.text().to_string(), "red");
        assert!(doc.is_content_readonly() && doc.is_readonly());
        assert!(!doc.is_modified());
        assert!(doc.path().is_none() && doc.uri().is_none());
        let transaction =
            editor_core::Transaction::insert(doc.text(), doc.selection(view.id), "changed".into());
        assert!(crate::editing::apply(&mut doc, &transaction, view.id).is_err());
        assert!(!doc.undo());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("output.txt");
        assert!(doc.save(Some(path.clone()), true).is_err());
        assert!(!path.exists());
    }
}
