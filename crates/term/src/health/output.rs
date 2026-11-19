//! Shared presentation for health checks; probing stays in the callers.

use std::{
    fmt::Display,
    io::{IsTerminal, Write},
};
use termina::{
    escape::csi::{Csi, Sgr},
    style::{ColorSpec, Intensity, StyleExt as _, Stylized},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    pub summary: bool,
    pub no_color: bool,
}

#[derive(Clone, Copy)]
pub(super) enum Status {
    Ok,
    Warning,
    Error,
    Inactive,
    Unknown,
    Info,
}

pub(super) struct Output<W> {
    writer: W,
    pub options: Options,
    color: bool,
}

impl<W: Write> Output<W> {
    pub fn new(writer: W, options: Options) -> Self {
        let color = !options.no_color
            && std::io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none()
            && std::env::var_os("TERM").is_none_or(|term| term != "dumb");
        Self {
            writer,
            options,
            color,
        }
    }

    #[cfg(test)]
    pub fn into_inner(self) -> W {
        self.writer
    }

    pub fn styled(&self, text: impl Into<String>, color: ColorSpec) -> Stylized<'static> {
        let text = text.into().stylized();
        if self.color {
            text.foreground(color)
        } else {
            text
        }
    }

    pub fn bold(&self, text: &str) -> Stylized<'static> {
        if self.color {
            text.to_owned().bold()
        } else {
            text.to_owned().stylized()
        }
    }

    pub fn dim<T: Display>(&self, text: T) -> Dimmed<T> {
        Dimmed {
            text,
            enabled: self.color,
        }
    }

    pub(super) fn marker(&self, status: Status) -> Stylized<'static> {
        let (marker, color) = match status {
            Status::Ok => ("✓", ColorSpec::BRIGHT_GREEN),
            Status::Warning => ("!", ColorSpec::BRIGHT_YELLOW),
            Status::Error => ("✘", ColorSpec::BRIGHT_RED),
            Status::Inactive => ("−", ColorSpec::BRIGHT_BLACK),
            Status::Unknown => ("?", ColorSpec::BRIGHT_YELLOW),
            Status::Info => ("·", ColorSpec::BRIGHT_BLACK),
        };
        self.styled(marker, color)
    }

    pub fn heading(&mut self, title: &str) -> std::io::Result<()> {
        writeln!(self.writer, "{}", self.bold(title))
    }

    pub fn row(&mut self, status: Status, label: &str, value: impl Display) -> std::io::Result<()> {
        let value = Dimmed {
            text: value,
            enabled: self.color && matches!(status, Status::Ok | Status::Inactive | Status::Info),
        };
        writeln!(self.writer, "  {} {label:<14} {value}", self.marker(status))
    }

    pub fn detail(&mut self, value: impl Display) -> std::io::Result<()> {
        if self.options.summary {
            return Ok(());
        }
        writeln!(self.writer, "      {}", self.dim(value))
    }

    pub fn field(&mut self, label: &str, value: impl Display) -> std::io::Result<()> {
        if self.options.summary {
            return Ok(());
        }
        writeln!(
            self.writer,
            "      {} {value}",
            self.styled(label, ColorSpec::BRIGHT_BLACK)
        )
    }

    // Important explanations and remedies remain visible in summary mode.
    pub fn note(&mut self, value: impl Display) -> std::io::Result<()> {
        writeln!(self.writer, "      {value}")
    }

    pub fn remedy(&mut self, value: impl Display) -> std::io::Result<()> {
        writeln!(
            self.writer,
            "    {} {value}",
            self.styled("→", ColorSpec::BRIGHT_YELLOW)
        )
    }

    pub fn feature_list(
        &mut self,
        status: Status,
        label: &str,
        features: &[String],
        columns: usize,
    ) -> std::io::Result<()> {
        use editor_core::unicode::width::UnicodeWidthStr;
        if self.options.summary {
            return self.row(
                status,
                label,
                format_args!(
                    "{} feature{}",
                    features.len(),
                    if features.len() == 1 { "" } else { "s" }
                ),
            );
        }
        writeln!(self.writer, "  {} {label}", self.marker(status))?;
        if features.is_empty() {
            return self.detail("None");
        }
        let mut line = String::new();
        for feature in features {
            if !line.is_empty() && 6 + line.width() + 2 + feature.width() > columns {
                self.note(&line)?;
                line.clear();
            }
            if !line.is_empty() {
                line.push_str(", ");
            }
            line.push_str(feature);
        }
        self.note(line)
    }
}

impl<W: Write> Write for Output<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.writer.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

pub(super) struct Dimmed<T> {
    text: T,
    enabled: bool,
}
impl<T: Display> Display for Dimmed<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.enabled {
            write!(
                f,
                "{}{}{}",
                Csi::Sgr(Sgr::Intensity(Intensity::Dim)),
                self.text,
                Csi::Sgr(Sgr::Reset)
            )
        } else {
            self.text.fmt(f)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Options, Output, Status, Stylized};

    #[test]
    fn language_query_marks_use_individual_colors() {
        let plain = "✓ ✘ ✓";
        let colored = if Stylized::is_ansi_color_disabled() {
            plain
        } else {
            "\x1b[0;92m✓\x1b[m \x1b[0;91m✘\x1b[m \x1b[0;92m✓\x1b[m"
        };
        let rows = [
            vec!["Language".into(), "H T I N R F".into()],
            vec!["ada".into(), plain.into()],
        ];
        for (columns, prefix) in [(80, "ada       "), (12, "  H T I N R F: ")] {
            for (color, expected) in [(true, colored), (false, plain)] {
                let mut output = Output {
                    writer: Vec::new(),
                    options: Options::default(),
                    color,
                };
                super::super::write_table(&mut output, &rows, columns).unwrap();
                let output = String::from_utf8(output.into_inner()).unwrap();
                assert!(
                    output.ends_with(&format!("{prefix}{expected}\n")),
                    "{output:?}"
                );
            }
        }
    }

    #[test]
    fn capability_lists_wrap_at_display_width_without_losing_names() {
        let mut output = Output::new(Vec::new(), Options::default());
        output
            .feature_list(
                Status::Ok,
                "Supported",
                &["日本語".into(), "hover".into(), "format".into()],
                20,
            )
            .unwrap();
        let output = String::from_utf8(output.into_inner()).unwrap();
        assert!(
            output.ends_with("      日本語, hover\n      format\n"),
            "{output}"
        );
    }

    #[test]
    fn summary_preserves_problems_and_remedies_but_hides_details() {
        let mut output = Output::new(
            Vec::new(),
            Options {
                summary: true,
                no_color: true,
            },
        );
        output.detail("hidden path").unwrap();
        output.field("Arguments:", "hidden args").unwrap();
        output
            .feature_list(Status::Inactive, "Not advertised", &["hover".into()], 80)
            .unwrap();
        output
            .row(Status::Unknown, "capabilities", "initialization timed out")
            .unwrap();
        output.remedy("install the missing tool").unwrap();
        let output = String::from_utf8(output.into_inner()).unwrap();
        assert!(
            !output.contains("hidden") && !output.contains("hover"),
            "{output}"
        );
        assert!(output.contains("Not advertised 1 feature"), "{output}");
        assert!(
            output.contains("initialization timed out")
                && output.contains("install the missing tool"),
            "{output}"
        );
    }
}
