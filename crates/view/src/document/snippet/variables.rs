//! Snapshot-based document variables for snippet rendering.

use crate::document::Mode;
use crate::Document;

impl Document {
    /// Capture variables against the text and selections used by the insertion.
    /// Completion callers must pass their saved state, rather than current typing.
    pub fn snippet_ctx(
        &self,
        text: &editor_core::Rope,
        selection: &editor_core::Selection,
        mode: Mode,
    ) -> snippets::SnippetRenderCtx {
        use std::borrow::Cow;
        use stdx::rope::RopeExt as _;
        let text = text.clone();
        let selection = selection.clone();
        let path = self.path().map(std::path::Path::to_path_buf);
        let workspace = self.workspace_root().to_path_buf();
        let comments: Vec<_> = selection
            .iter()
            .map(|range| {
                let cursor = range.cursor(text.slice(..));
                let byte = text.char_to_byte_idx(cursor).min(self.text().len());
                let language = self.language_config_at(byte);
                (
                    language
                        .and_then(|language| language.comment_tokens.as_ref()?.first().cloned()),
                    language.and_then(|language| {
                        language.block_comment_tokens.as_ref()?.first().cloned()
                    }),
                )
            })
            .collect();
        snippets::SnippetRenderCtx {
            resolve_var: Box::new(move |name, index| {
                let range = selection.ranges().get(index)?;
                let cursor = range.cursor(text.slice(..));
                let line_index = text.char_to_line_idx(cursor);
                let value = match name {
                    "TM_SELECTED_TEXT" | "SELECTION" => {
                        if range.is_empty()
                            || (mode == Mode::Insert && range.is_single_grapheme(text.slice(..)))
                        {
                            return None;
                        }
                        range.fragment(text.slice(..)).into_owned()
                    }
                    "TM_CURRENT_LINE" => editor_core::line_ending::line_without_line_ending(
                        &text.slice(..),
                        line_index,
                    )
                    .to_string(),
                    "TM_CURRENT_WORD" => {
                        let before = text
                            .slice_chars(..cursor)
                            .chars_at_char(cursor)
                            .reversed()
                            .take_while(|c| editor_core::chars::char_is_word(*c))
                            .count();
                        let after = text
                            .slice_chars(cursor..)
                            .chars()
                            .take_while(|c| editor_core::chars::char_is_word(*c))
                            .count();
                        if before + after == 0 {
                            return None;
                        }
                        text.slice_chars(cursor - before..cursor + after)
                            .to_string()
                    }
                    "TM_LINE_INDEX" => line_index.to_string(),
                    "TM_LINE_NUMBER" => (line_index + 1).to_string(),
                    "CURSOR_INDEX" => index.to_string(),
                    "CURSOR_NUMBER" => (index + 1).to_string(),
                    "TM_FILENAME" => path.as_ref()?.file_name()?.to_string_lossy().into_owned(),
                    "TM_FILENAME_BASE" => {
                        path.as_ref()?.file_stem()?.to_string_lossy().into_owned()
                    }
                    "TM_DIRECTORY" => path.as_ref()?.parent()?.to_string_lossy().into_owned(),
                    "TM_FILEPATH" => path.as_ref()?.to_string_lossy().into_owned(),
                    "RELATIVE_FILEPATH" => {
                        stdx::path::get_relative_path_from(path.as_ref()?, &workspace)
                            .to_string_lossy()
                            .into_owned()
                    }
                    "WORKSPACE_NAME" => workspace.file_name()?.to_string_lossy().into_owned(),
                    "WORKSPACE_FOLDER" => workspace.to_string_lossy().into_owned(),
                    "LINE_COMMENT" => comments.get(index)?.0.clone()?,
                    "BLOCK_COMMENT_START" => comments.get(index)?.1.as_ref()?.start.clone(),
                    "BLOCK_COMMENT_END" => comments.get(index)?.1.as_ref()?.end.clone(),
                    _ => return None,
                };
                Some(Cow::Owned(value))
            }),
            tab_width: self.tab_width(),
            indent_style: self.indent_style,
            line_ending: self.line_ending.as_str(),
        }
    }
}

#[cfg(test)]
mod variable_tests {
    use super::*;
    use crate::document::{test_document, Mode};
    use editor_core::{Range, Selection, Transaction};

    fn expand(doc: &Document, selection: &Selection, body: &str, mode: Mode) -> String {
        let snippet = snippets::Snippet::parse(body).unwrap();
        let (transaction, _, _) = snippet.render(
            doc.text(),
            selection,
            |range| (range.from(), range.to()),
            &mut doc.snippet_ctx(doc.text(), selection, mode),
        );
        let mut text = doc.text().clone();
        transaction.apply(&mut text);
        text.to_string()
    }

    #[tokio::test]
    async fn text_variables_use_each_original_unicode_selection() {
        let doc = test_document("αβ\r\n世界\r\n");
        let selection = Selection::new(vec![Range::new(0, 2), Range::new(6, 4)].into(), 1);
        assert_eq!(
            expand(
                &doc,
                &selection,
                "[$TM_SELECTED_TEXT:$TM_LINE_INDEX:$TM_LINE_NUMBER:$CURSOR_INDEX:$CURSOR_NUMBER]",
                Mode::Select
            ),
            "[αβ:0:1:0:1]\r\n[世界:1:2:1:2]\r\n"
        );
        assert_eq!(
            expand(
                &doc,
                &Selection::single(0, 1),
                "<$TM_SELECTED_TEXT>",
                Mode::Select
            ),
            "<α>β\r\n世界\r\n"
        );
        assert_eq!(
            expand(
                &doc,
                &Selection::single(0, 1),
                "<${TM_SELECTED_TEXT:empty}>",
                Mode::Insert
            ),
            "<empty>β\r\n世界\r\n"
        );
    }

    #[tokio::test]
    async fn current_line_and_word_do_not_include_line_endings() {
        let doc = test_document("hello 世界\r\n\r\n");
        assert_eq!(
            expand(
                &doc,
                &Selection::point(7),
                "$TM_CURRENT_LINE|$TM_CURRENT_WORD",
                Mode::Insert
            ),
            "hello 世hello 世界|世界界\r\n\r\n"
        );
        assert_eq!(
            expand(
                &doc,
                &Selection::point(10),
                "${TM_CURRENT_WORD:empty}|$TM_LINE_NUMBER",
                Mode::Insert
            ),
            "hello 世界\r\nempty|2\r\n"
        );
    }

    #[tokio::test]
    async fn missing_file_variables_use_defaults_and_transforms_use_empty_input() {
        let doc = test_document("\n");
        assert_eq!(expand(&doc, &Selection::point(0),
            "${TM_FILENAME:untitled}|${UNKNOWN:fallback}|${TM_FILENAME/^$/missing/}|${TM_FILENAME}", Mode::Normal),
            "untitled|fallback|missing|\n");
    }
    #[tokio::test]
    async fn saved_text_and_selection_do_not_follow_later_typing() {
        let mut doc = crate::document::test_document("old\n");
        let text = doc.text().clone();
        let selection = Selection::single(0, 3);
        let view = crate::View::new(doc.id(), Default::default(), event::Redraw::default());
        doc.ensure_view_init(view.id);
        crate::editing::apply(
            &mut doc,
            &Transaction::change(&text, [(0, 3, Some("new text".into()))].into_iter()),
            view.id,
        )
        .unwrap();
        let mut context = doc.snippet_ctx(&text, &selection, Mode::Select);
        assert_eq!((context.resolve_var)("TM_SELECTED_TEXT", 0).unwrap(), "old");
        assert_eq!((context.resolve_var)("TM_CURRENT_LINE", 0).unwrap(), "old");
    }

    #[tokio::test]
    async fn overlapping_replacements_keep_original_cursor_numbers_and_primary_selection() {
        let doc = crate::document::test_document("ab cd\n");
        let selection = Selection::new(
            vec![Range::point(1), Range::point(2), Range::point(5)].into(),
            2,
        );
        let snippet = snippets::Snippet::parse("$CURSOR_INDEX:${1:x}$0").unwrap();
        let (transaction, surviving, rendered) = snippet.render(
            doc.text(),
            &selection,
            |range| if range.head <= 2 { (0, 2) } else { (3, 5) },
            &mut doc.snippet_ctx(doc.text(), &selection, Mode::Insert),
        );
        let mut text = doc.text().clone();
        transaction.apply(&mut text);
        assert_eq!(text, "0:x 2:x\n");
        assert_eq!(surviving.primary_index(), 1);
        assert_eq!(
            rendered
                .first_selection(
                    editor_core::movement::Direction::Forward,
                    surviving.primary_index()
                )
                .primary_index(),
            1
        );
    }
}
