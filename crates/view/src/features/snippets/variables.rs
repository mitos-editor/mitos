//! Editor variables are captured only for bodies that reference them.
use crate::{Document, Editor};
use editor_core::{Rope, Selection};
use snippets::{Snippet, SnippetRenderCtx};
use std::{borrow::Cow, collections::HashMap};

impl Editor {
    pub(crate) fn snippet_context(
        &self,
        doc: &Document,
        text: &Rope,
        selection: &Selection,
        snippet: &Snippet,
    ) -> SnippetRenderCtx {
        let clipboard = snippet
            .uses_variable("CLIPBOARD")
            .then(|| self.registers.clipboard_contents().ok())
            .flatten();
        let mode = if doc.snippet_placeholder_active(self.tree.focus()) {
            crate::document::Mode::Select
        } else {
            self.mode()
        };
        with_editor_variables(doc.snippet_ctx(text, selection, mode), clipboard)
    }
}

fn with_editor_variables(
    mut context: SnippetRenderCtx,
    clipboard: Option<String>,
) -> SnippetRenderCtx {
    let mut document = context.resolve_var;
    let now = jiff::Zoned::now();
    let mut random = HashMap::<(usize, String), String>::new();
    context.resolve_var = Box::new(move |name, index| {
        let value = match name {
            "CLIPBOARD" => clipboard.clone()?,
            "RANDOM" | "RANDOM_HEX" | "UUID" => random
                .entry((index, name.to_owned()))
                .or_insert_with(|| {
                    let uuid = uuid::Uuid::new_v4();
                    match name {
                        "RANDOM" => format!(
                            "{:06}",
                            u32::from_le_bytes(uuid.as_bytes()[..4].try_into().unwrap())
                                % 1_000_000
                        ),
                        "RANDOM_HEX" => uuid.simple().to_string()[..6].to_owned(),
                        _ => uuid.to_string(),
                    }
                })
                .clone(),
            _ => match time_variable(name, &now) {
                Some(value) => value,
                None => return document(name, index),
            },
        };
        Some(Cow::Owned(value))
    });
    context
}

fn time_variable(name: &str, now: &jiff::Zoned) -> Option<String> {
    let format = match name {
        "CURRENT_YEAR" => "%Y",
        "CURRENT_YEAR_SHORT" => "%y",
        "CURRENT_MONTH" => "%m",
        "CURRENT_MONTH_NAME" => "%B",
        "CURRENT_MONTH_NAME_SHORT" => "%b",
        "CURRENT_DATE" => "%d",
        "CURRENT_DAY_NAME" => "%A",
        "CURRENT_DAY_NAME_SHORT" => "%a",
        "CURRENT_HOUR" => "%H",
        "CURRENT_MINUTE" => "%M",
        "CURRENT_SECOND" => "%S",
        "CURRENT_TIMEZONE_OFFSET" => "%:z",
        "CURRENT_MILLISECOND" => return Some(format!("{:03}", now.millisecond())),
        "CURRENT_SECONDS_UNIX" => return Some(now.timestamp().as_second().to_string()),
        "CURRENT_MILLISECONDS_UNIX" => return Some(now.timestamp().as_millisecond().to_string()),
        "CURRENT_TIMEZONE_NAME" => return now.time_zone().iana_name().map(str::to_owned),
        _ => return None,
    };
    Some(now.strftime(format).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn time_variables_have_stable_formats() {
        let now: jiff::Zoned = "2024-02-03T04:05:06.078Z[UTC]".parse().unwrap();
        for (name, expected) in [
            ("CURRENT_YEAR", "2024"),
            ("CURRENT_MONTH", "02"),
            ("CURRENT_DATE", "03"),
            ("CURRENT_DAY_NAME", "Saturday"),
            ("CURRENT_MILLISECOND", "078"),
            ("CURRENT_TIMEZONE_OFFSET", "+00:00"),
        ] {
            assert_eq!(time_variable(name, &now).as_deref(), Some(expected));
        }
    }
}
