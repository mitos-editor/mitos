//! Conservative matching of local names in the selected function's parsed source.

use super::{labels::Candidate, MAX_SOURCE_LENGTH};
use editor_core::{chars::char_is_word, Rope};
use language::syntax;
use std::collections::BTreeMap;
use std::ops::Range;
use stdx::rope::RopeExt as _;

/// Stay within the innermost function identified by the language's textobject query.
/// Without a function boundary, only inspect the stopped line.
pub(super) fn candidates(
    text: &Rope,
    syntax: &language::Syntax,
    loader: &syntax::Loader,
    position: usize,
    mut lines: Range<usize>,
) -> Vec<Candidate> {
    let stopped_line = text.char_to_line_idx(position);
    let scope = (|| {
        let byte = text.char_to_byte_idx(position);
        let layer = syntax.layer_for_byte_range(byte as u32, byte as u32);
        let root = syntax
            .tree_for_byte_range(byte as u32, byte as u32)
            .root_node();
        let functions: Vec<_> = loader
            .textobject_query(syntax.layer(layer).language)?
            .capture_nodes("function.around", &root, text.slice(..))?
            .map(|node| node.byte_range())
            .collect();
        let current = functions
            .iter()
            .filter(|range| range.contains(&byte))
            .min_by_key(|range| range.len())?;
        let current = current.clone();
        let nested = functions
            .iter()
            .filter(|range| {
                range.start >= current.start && range.end <= current.end && !range.contains(&byte)
            })
            .cloned()
            .collect::<Vec<_>>();
        Some((current, nested))
    })();
    let (scope, nested) = scope.unwrap_or_else(|| {
        (
            text.line_to_byte_idx(stopped_line, stdx::rope::LINE_TYPE)
                ..text.line_to_byte_idx(
                    (stopped_line + 1).min(text.len_lines(stdx::rope::LINE_TYPE)),
                    stdx::rope::LINE_TYPE,
                ),
            Vec::new(),
        )
    });
    lines.start = lines
        .start
        .max(text.byte_to_line_idx(scope.start, stdx::rope::LINE_TYPE));
    let mut matches = BTreeMap::new();
    for line in lines {
        let content = text.line(line, stdx::rope::LINE_TYPE);
        if content.len_chars() > MAX_SOURCE_LENGTH {
            continue;
        }
        let content = content.to_string();
        let mut tokens = content.char_indices().peekable();
        while let Some((start, ch)) = tokens.next() {
            if !char_is_word(ch) || ch.is_numeric() {
                continue;
            }
            let mut end = start + ch.len_utf8();
            while let Some(&(offset, ch)) = tokens.peek() {
                if !char_is_word(ch) {
                    break;
                }
                end = offset + ch.len_utf8();
                tokens.next();
            }
            // A field/property isn't an independent local with the same name.
            if content[..start].trim_end().ends_with(['.', ':']) {
                continue;
            }
            let byte = text.line_to_byte_idx(line, stdx::rope::LINE_TYPE) + start;
            if !scope.contains(&byte) || nested.iter().any(|range| range.contains(&byte)) {
                continue;
            }
            if !is_local_name(syntax, byte as u32..(byte + end - start) as u32) {
                continue;
            }
            let name = content[start..end].to_owned();
            // Keep only the last visible occurrence of each name.
            matches.insert(name, line);
        }
    }
    matches
        .into_iter()
        .map(|(name, line)| Candidate::variable(line, name))
        .collect()
}

fn is_local_name(syntax: &language::Syntax, bytes: Range<u32>) -> bool {
    let Some(mut node) = syntax.descendant_for_byte_range(bytes.start, bytes.end) else {
        return false;
    };
    if node.byte_range() != bytes
        || matches!(
            node.kind(),
            "field_identifier" | "property_identifier" | "type_identifier"
        )
    {
        return false;
    }
    loop {
        if node.kind() == "ERROR"
            || node.kind().contains("comment")
            || node.kind().contains("string")
            || node.kind().contains("char_literal")
        {
            return false;
        }
        let Some(parent) = node.parent() else {
            return true;
        };
        node = parent;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use language::{loader::default_lang_loader, Syntax};

    fn source_candidates(language: &str, source: &str, stopped: &str) -> Vec<Candidate> {
        let loader = default_lang_loader(loader::syntax::Resources::default());
        let text = Rope::from_str(source);
        let language = loader.language_for_name(language).unwrap();
        let syntax = Syntax::new_with_timeout(
            text.slice(..),
            language,
            &loader,
            std::time::Duration::from_secs(10),
        )
        .unwrap();
        let position = text.byte_to_char_idx(source.find(stopped).unwrap());
        candidates(
            &text,
            &syntax,
            &loader,
            position,
            0..text.char_to_line_idx(position) + 1,
        )
    }

    #[test]
    fn sibling_and_nested_functions_do_not_supply_local_names() {
        let source = "fn sibling() { answer; }\nfn main() {\nfn nested() { answer; }\nobject;\n}\n";
        let values = source_candidates("rust", source, "object");
        assert!(values.contains(&Candidate::variable(3, "object".into())));
        assert!(!values
            .iter()
            .any(|value| (0..4).any(|line| *value == Candidate::variable(line, "answer".into()))));
    }

    #[test]
    fn locals_after_many_unrelated_names_are_included() {
        let declarations: String = (0..120)
            .map(|index| format!("let name{index} = 0;\n"))
            .collect();
        let source = format!("fn main() {{\n{declarations}answer;\n}}");
        assert!(source_candidates("rust", &source, "answer")
            .contains(&Candidate::variable(121, "answer".into())));
    }

    #[test]
    fn field_and_type_names_do_not_supply_local_names() {
        let source = "fn main() {\nlet value: answer = Type { answer: other };\nobject;\n}";
        let values = source_candidates("rust", source, "object");
        assert!(!values.contains(&Candidate::variable(1, "answer".into())));
    }

    #[test]
    fn functions_sharing_a_line_keep_separate_names() {
        let source = "fn before() { answer; } fn main() { object; } fn after() { answer; }";
        let values = source_candidates("rust", source, "object");
        assert!(values.contains(&Candidate::variable(0, "object".into())));
        assert!(!values.contains(&Candidate::variable(0, "answer".into())));
    }
}
