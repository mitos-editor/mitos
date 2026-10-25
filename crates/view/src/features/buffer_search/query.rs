use std::sync::Arc;
use stdx::rope;

/// A compiled buffer query and the resolved options that give it its identity.
/// Independently compiled queries can reuse statistics without pointer identity.
#[derive(Clone, Debug)]
pub struct Query {
    pattern: Arc<str>,
    case_insensitive: bool,
    crlf: bool,
    regex: rope::Regex,
}

impl Query {
    pub fn new(pattern: &str, smart_case: bool, crlf: bool) -> anyhow::Result<Self> {
        let case_insensitive = smart_case && !pattern.chars().any(char::is_uppercase);
        let regex = rope::RegexBuilder::new()
            .syntax(
                rope::Config::new()
                    .case_insensitive(case_insensitive)
                    .multi_line(true)
                    .crlf(crlf),
            )
            .build(pattern)?;
        Ok(Self {
            pattern: pattern.into(),
            case_insensitive,
            crlf,
            regex,
        })
    }

    pub fn regex(&self) -> &rope::Regex {
        &self.regex
    }

    pub fn into_regex(self) -> rope::Regex {
        self.regex
    }

    pub(super) fn same_pattern(&self, other: &Self) -> bool {
        self.pattern == other.pattern
            && self.case_insensitive == other.case_insensitive
            && self.crlf == other.crlf
    }
}
