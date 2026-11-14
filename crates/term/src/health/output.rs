//! Shared presentation for health checks; probing stays in the callers.

use std::{fmt::Display, io::Write};

use termina::style::{ColorSpec, StyleExt as _};

pub(super) enum Status {
    Ok,
    Warning,
    Error,
    Inactive,
    Unknown,
    Info,
}

impl Status {
    fn marker(&self) -> termina::style::Stylized<'static> {
        let (marker, color) = match self {
            Self::Ok => ("✓", ColorSpec::BRIGHT_GREEN),
            Self::Warning => ("!", ColorSpec::BRIGHT_YELLOW),
            Self::Error => ("✘", ColorSpec::BRIGHT_RED),
            Self::Inactive => ("−", ColorSpec::BRIGHT_BLACK),
            Self::Unknown => ("?", ColorSpec::BRIGHT_YELLOW),
            Self::Info => ("·", ColorSpec::BRIGHT_BLACK),
        };
        super::styled(marker, color)
    }
}

pub(super) fn heading(output: &mut impl Write, title: &str) -> std::io::Result<()> {
    let title = if super::use_color() {
        title.to_owned().bold()
    } else {
        title.to_owned().stylized()
    };
    writeln!(output, "{title}")
}

pub(super) fn row(
    output: &mut impl Write,
    status: Status,
    label: &str,
    value: impl Display,
) -> std::io::Result<()> {
    // Keep paths, commands, and descriptions uncolored and easy to copy.
    writeln!(output, "  {} {label:<14} {value}", status.marker())
}

pub(super) fn detail(output: &mut impl Write, value: impl Display) -> std::io::Result<()> {
    writeln!(output, "      {value}")
}

pub(super) fn feature_list(
    output: &mut impl Write,
    status: Status,
    label: &str,
    features: &[String],
    columns: usize,
) -> std::io::Result<()> {
    use editor_core::unicode::width::UnicodeWidthStr;

    writeln!(output, "  {} {label}", status.marker())?;
    if features.is_empty() {
        return detail(output, "None");
    }
    let mut line = String::new();
    for feature in features {
        if !line.is_empty() && 6 + line.width() + 2 + feature.width() > columns {
            detail(output, &line)?;
            line.clear();
        }
        if !line.is_empty() {
            line.push_str(", ");
        }
        line.push_str(feature);
    }
    detail(output, line)
}

#[cfg(test)]
mod tests {
    #[test]
    fn capability_lists_wrap_at_display_width_without_losing_names() {
        let mut output = Vec::new();
        super::feature_list(
            &mut output,
            super::Status::Ok,
            "Supported",
            &["日本語".into(), "hover".into(), "format".into()],
            20,
        )
        .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(
            output.ends_with("      日本語, hover\n      format\n"),
            "{output}"
        );
    }
}
