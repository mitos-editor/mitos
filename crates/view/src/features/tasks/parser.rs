use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use regex::Regex;

use crate::quicklist::{QuicklistEntry, QuicklistPosition, QuicklistTarget};

const MAX_DIAGNOSTIC_LINE_BYTES: usize = 64 * 1024;
const DEFAULT_PATTERN: &str = r"^\s*(?:-->\s*)?(?P<path>.+?):(?P<line>[0-9]+)(?::(?P<column>[0-9]+))?(?::\s*(?P<message>.*)|\s*)$";

pub(super) struct Parser {
    pattern: Regex,
    cwd: PathBuf,
}

impl Parser {
    pub(super) fn new(pattern: Option<&str>, cwd: PathBuf) -> Result<Self> {
        let pattern = Regex::new(pattern.unwrap_or(DEFAULT_PATTERN))
            .context("invalid task output pattern")?;
        for name in ["path", "line"] {
            if !pattern
                .capture_names()
                .flatten()
                .any(|capture| capture == name)
            {
                bail!("task output pattern requires a named '{name}' capture");
            }
        }
        Ok(Self { pattern, cwd })
    }

    fn parse(&self, line: &str) -> Option<QuicklistEntry> {
        let captures = self.pattern.captures(line)?;
        let path = captures.name("path")?.as_str().trim();
        if path.is_empty() {
            return None;
        }
        let line = captures
            .name("line")?
            .as_str()
            .parse::<usize>()
            .ok()?
            .checked_sub(1)?;
        let column = captures
            .name("column")
            .map(|column| column.as_str().parse::<usize>().ok()?.checked_sub(1))
            .unwrap_or(Some(0))?;

        let path_buf = PathBuf::from(path);
        // Recognize Windows paths when reading output on a different host as well.
        let bytes = path.as_bytes();
        let windows_absolute = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\');
        let path = if path_buf.is_absolute() || windows_absolute || path.starts_with("\\\\") {
            path_buf
        } else {
            self.cwd.join(path_buf)
        };
        Some(QuicklistEntry {
            target: QuicklistTarget::Path(path),
            position: QuicklistPosition::LineColRange {
                start_line: line,
                start_col: column,
                end_line: line,
                end_col: column,
            },
        })
    }
}

#[derive(Default)]
enum EscapeState {
    #[default]
    Text,
    Escape,
    EscapeIntermediate,
    Csi,
    Osc,
    OscEscape,
    ControlString,
    ControlStringEscape,
}

/// One decoder per output stream: UTF-8 and escape sequences can span reads.
#[derive(Default)]
pub(super) struct StreamDecoder {
    utf8_pending: Vec<u8>,
    escape: EscapeState,
    line: String,
    oversized_line: bool,
}

impl StreamDecoder {
    pub(super) fn push(
        &mut self,
        bytes: &[u8],
        finished: bool,
        parser: &Parser,
    ) -> (String, Vec<QuicklistEntry>) {
        let decoded = self.decode_utf8(bytes, finished);
        let mut output = String::with_capacity(decoded.len());
        let mut locations = Vec::new();
        for character in decoded.chars() {
            if !self.visible(character) {
                continue;
            }
            output.push(character);
            if character == '\n' {
                self.finish_line(parser, &mut locations);
            } else if !self.oversized_line {
                if self.line.len() + character.len_utf8() <= MAX_DIAGNOSTIC_LINE_BYTES {
                    self.line.push(character);
                } else {
                    // Keep all output visible, but do not retain arbitrarily long lines.
                    self.line.clear();
                    self.oversized_line = true;
                }
            }
        }
        if finished {
            self.finish_line(parser, &mut locations);
            self.escape = EscapeState::Text;
        }
        (output, locations)
    }

    fn decode_utf8(&mut self, bytes: &[u8], finished: bool) -> String {
        let mut input = std::mem::take(&mut self.utf8_pending);
        input.extend_from_slice(bytes);
        let mut output = String::with_capacity(input.len());
        let mut remaining = input.as_slice();
        loop {
            match std::str::from_utf8(remaining) {
                Ok(text) => {
                    output.push_str(text);
                    break;
                }
                Err(error) => {
                    let (valid, invalid) = remaining.split_at(error.valid_up_to());
                    output.push_str(std::str::from_utf8(valid).expect("valid UTF-8 prefix"));
                    match error.error_len() {
                        Some(length) => {
                            output.push('\u{fffd}');
                            remaining = &invalid[length..];
                        }
                        None => {
                            if finished {
                                output.push('\u{fffd}');
                            } else {
                                // An incomplete UTF-8 code point is at most three bytes.
                                self.utf8_pending.extend_from_slice(invalid);
                            }
                            break;
                        }
                    }
                }
            }
        }
        output
    }

    fn visible(&mut self, character: char) -> bool {
        use EscapeState::*;
        match self.escape {
            Text => match character {
                '\u{1b}' => self.escape = Escape,
                '\u{9b}' => self.escape = Csi,
                '\u{9d}' => self.escape = Osc,
                '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => self.escape = ControlString,
                '\n' | '\t' => return true,
                character if !character.is_control() => return true,
                _ => (),
            },
            Escape => {
                self.escape = match character {
                    '[' => Csi,
                    ']' => Osc,
                    'P' | 'X' | '^' | '_' => ControlString,
                    '\u{20}'..='\u{2f}' => EscapeIntermediate,
                    '\u{1b}' => Escape,
                    _ => Text,
                };
            }
            EscapeIntermediate => match character {
                '\u{30}'..='\u{7e}' => self.escape = Text,
                '\u{1b}' => self.escape = Escape,
                _ => (),
            },
            Csi => match character {
                '\u{40}'..='\u{7e}' => self.escape = Text,
                '\u{1b}' => self.escape = Escape,
                _ => (),
            },
            Osc => match character {
                '\u{7}' | '\u{9c}' => self.escape = Text,
                '\u{1b}' => self.escape = OscEscape,
                _ => (),
            },
            OscEscape => {
                self.escape = match character {
                    '\\' | '\u{7}' | '\u{9c}' => Text,
                    '\u{1b}' => OscEscape,
                    _ => Osc,
                };
            }
            ControlString => match character {
                '\u{9c}' => self.escape = Text,
                '\u{1b}' => self.escape = ControlStringEscape,
                _ => (),
            },
            ControlStringEscape => {
                self.escape = match character {
                    '\\' | '\u{9c}' => Text,
                    '\u{1b}' => ControlStringEscape,
                    _ => ControlString,
                };
            }
        }
        false
    }

    fn finish_line(&mut self, parser: &Parser, locations: &mut Vec<QuicklistEntry>) {
        if !self.oversized_line
            && let Some(location) = parser.parse(&self.line)
        {
            locations.push(location);
        }
        self.line.clear();
        self.oversized_line = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parser() -> Parser {
        Parser::new(None, PathBuf::from("/workspace")).unwrap()
    }

    fn location(path: &str, line: usize, column: usize) -> QuicklistEntry {
        QuicklistEntry {
            target: QuicklistTarget::Path(PathBuf::from(path)),
            position: QuicklistPosition::LineColRange {
                start_line: line,
                start_col: column,
                end_line: line,
                end_col: column,
            },
        }
    }

    #[test]
    fn streams_text_immediately_and_preserves_split_utf8() {
        let parser = parser();
        let mut stream = StreamDecoder::default();
        let (output, locations) = stream.push(b"compiling \xf0\x9f", false, &parser);
        assert_eq!(output, "compiling ");
        assert!(locations.is_empty());
        assert_eq!(stream.utf8_pending.len(), 2);
        assert_eq!(stream.push(b"\xa6\x80", false, &parser).0, "🦀");
        assert!(stream.utf8_pending.is_empty());
        assert_eq!(stream.push(b"\xff\xe2", false, &parser).0, "\u{fffd}");
        assert_eq!(stream.push(b"", true, &parser).0, "\u{fffd}");
    }

    #[test]
    fn strips_split_csi_osc_and_other_controls() {
        let parser = parser();
        let mut stream = StreamDecoder::default();
        assert_eq!(stream.push(b"\x1b[3", false, &parser).0, "");
        assert_eq!(
            stream
                .push(b"1msrc/main.rs:2:3: \x1b[0merror", false, &parser)
                .0,
            "src/main.rs:2:3: error"
        );
        assert_eq!(
            stream
                .push(b"\x1b]8;;https://example.com", false, &parser)
                .0,
            ""
        );
        assert_eq!(stream.push(b"\x1b", false, &parser).0, "");
        let (output, locations) = stream.push(b"\\link\x1b]8;;\x07\x00\r\n\t", true, &parser);
        assert_eq!(output, "link\n\t");
        assert_eq!(locations, vec![location("/workspace/src/main.rs", 1, 2)]);
    }

    #[test]
    fn parses_go_rust_unicode_and_windows_locations() {
        let parser = parser();
        let mut stream = StreamDecoder::default();
        let input = "pkg/main.go:12:7: undefined: name\r\n  --> src/é space.rs:8:4\nC:\\source dir\\main.c:9:2: error\nC:/source/main.c:3: warning\n";
        let (output, locations) = stream.push(input.as_bytes(), false, &parser);
        assert!(!output.contains('\r'));
        assert_eq!(
            locations,
            vec![
                location("/workspace/pkg/main.go", 11, 6),
                location("/workspace/src/é space.rs", 7, 3),
                location("C:\\source dir\\main.c", 8, 1),
                location("C:/source/main.c", 2, 0),
            ]
        );
    }

    #[test]
    fn parses_final_unterminated_line_once() {
        let parser = parser();
        let mut stream = StreamDecoder::default();
        assert!(stream
            .push(b"main.go:10: failure", false, &parser)
            .1
            .is_empty());
        assert_eq!(
            stream.push(b"", true, &parser).1,
            vec![location("/workspace/main.go", 9, 0)]
        );
        assert!(stream.push(b"", true, &parser).1.is_empty());
    }

    #[test]
    fn custom_pattern_handles_python_tracebacks() {
        let parser = Parser::new(
            Some(r#"^\s*File "(?P<path>.+)", line (?P<line>\d+)(?:, (?P<message>.*))?$"#),
            PathBuf::from("/workspace"),
        )
        .unwrap();
        let mut stream = StreamDecoder::default();
        let (_, locations) =
            stream.push(b"  File \"src/test.py\", line 42, in test\n", true, &parser);
        assert_eq!(locations, vec![location("/workspace/src/test.py", 41, 0)]);
    }

    #[test]
    fn validates_patterns_and_rejects_invalid_offsets() {
        for pattern in ["[", r"(?P<path>.+)", r"(?P<line>\d+)"] {
            assert!(Parser::new(Some(pattern), PathBuf::new()).is_err());
        }
        let parser = parser();
        for line in [
            "main.rs:0:1: error",
            "main.rs:1:0: error",
            "main.rs:999999999999999999999999999999:1: error",
        ] {
            assert!(parser.parse(line).is_none(), "{line}");
        }
        let custom = Parser::new(
            Some(r"(?P<path>\S+):(?P<line>\S+):(?P<column>\S+)"),
            PathBuf::new(),
        )
        .unwrap();
        assert!(custom.parse("main.rs:invalid:2").is_none());
        assert!(custom.parse("main.rs:2:invalid").is_none());
    }

    #[test]
    fn oversized_lines_remain_visible_without_unbounded_diagnostic_storage() {
        let parser = parser();
        let mut stream = StreamDecoder::default();
        let text = "x".repeat(MAX_DIAGNOSTIC_LINE_BYTES);
        assert_eq!(stream.push(text.as_bytes(), false, &parser).0, text);
        assert_eq!(stream.line.len(), MAX_DIAGNOSTIC_LINE_BYTES);
        assert_eq!(
            stream.push(b"main.rs:2:3: error", false, &parser).0,
            "main.rs:2:3: error"
        );
        assert!(stream.line.is_empty());
        assert!(stream.oversized_line);
        let (output, locations) = stream.push(b"\nmain.rs:3:4: next\n", true, &parser);
        assert_eq!(output, "\nmain.rs:3:4: next\n");
        assert_eq!(locations, vec![location("/workspace/main.rs", 2, 3)]);
    }
}
