use super::Fixture;
use editor_core::{movement::Direction, Selection};
use lsp_client::{lsp, OffsetEncoding, UriExt as _};
use view::{
    current, current_ref,
    document::Mode,
    editor::{Action, SnippetNavigation},
    workspace_edit::ApplyEditErrorKind,
};

fn snippet(start: u32, end: u32, body: &str) -> lsp::DocumentTextEdit {
    lsp::DocumentTextEdit::Snippet(lsp::SnippetTextEdit {
        range: lsp::Range::new(lsp::Position::new(0, start), lsp::Position::new(0, end)),
        snippet: lsp::StringValue {
            kind: "snippet".into(),
            value: body.into(),
        },
        annotation_id: None,
    })
}

fn plain(start: u32, end: u32, text: &str) -> lsp::DocumentTextEdit {
    lsp::TextEdit {
        range: lsp::Range::new(lsp::Position::new(0, start), lsp::Position::new(0, end)),
        new_text: text.into(),
    }
    .into()
}

fn edit(uri: lsp::Url, edits: Vec<lsp::DocumentTextEdit>) -> lsp::WorkspaceEdit {
    lsp::WorkspaceEdit {
        document_changes: Some(lsp::DocumentChanges::Edits(vec![lsp::TextDocumentEdit {
            text_document: lsp::OptionalVersionedTextDocumentIdentifier { uri, version: None },
            edits,
        }])),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_snippets_share_original_utf16_ranges_with_plain_edits_and_undo(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("😀 foo end\n")?;
    let (view, doc) = current_ref!(f.editor);
    let focus = view.id;
    let edit = edit(
        doc.url().unwrap(),
        vec![
            snippet(3, 6, "${1:name}($0)"),
            plain(7, 10, "tail"),
            plain(0, 0, "use;\n"),
        ],
    );
    f.editor
        .apply_workspace_edit(OffsetEncoding::Utf16, &edit)?;
    let (view, doc) = current_ref!(f.editor);
    assert_eq!(view.id, focus);
    assert_eq!(doc.text(), "use;\n😀 name() tail\n");
    assert_eq!(doc.selection(view.id), &Selection::single(7, 11));
    assert!(doc.active_snippet().is_some());
    assert_eq!(f.editor.mode(), Mode::Insert);
    assert_eq!(
        f.editor.move_snippet_tabstop(Direction::Forward),
        SnippetNavigation::Finished
    );
    let (view, doc) = current!(f.editor);
    assert_eq!(doc.selection(view.id).primary().from(), 12);
    assert!(view::editing::undo(doc, view.id));
    assert_eq!(doc.text(), "😀 foo end\n");
    assert!(doc.active_snippet().is_none());
    assert!(view::editing::redo(doc, view.id));
    assert_eq!(doc.text(), "use;\n😀 name() tail\n");
    assert!(doc.active_snippet().is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn equal_position_inserts_keep_snippet_tabstops_inside_their_own_text() -> anyhow::Result<()>
{
    let mut f = Fixture::new("\n")?;
    let uri = current_ref!(f.editor).1.url().unwrap();
    f.editor.apply_workspace_edit(
        OffsetEncoding::Utf8,
        &edit(
            uri,
            vec![
                plain(0, 0, "before"),
                snippet(0, 0, "${1:one}$0"),
                plain(0, 0, "after"),
            ],
        ),
    )?;
    let (view, doc) = current_ref!(f.editor);
    assert_eq!(doc.text(), "beforeoneafter\n");
    assert_eq!(doc.selection(view.id), &Selection::single(6, 9));
    f.editor.insert_char('x')?;
    assert_eq!(current_ref!(f.editor).1.text(), "beforexafter\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_snippet_in_protocol_order_owns_the_cursor() -> anyhow::Result<()> {
    let mut f = Fixture::new("ab\n")?;
    let uri = current_ref!(f.editor).1.url().unwrap();
    f.editor.apply_workspace_edit(
        OffsetEncoding::Utf8,
        &edit(
            uri,
            vec![snippet(2, 2, "${1:last}$0"), snippet(0, 0, "${1:first}$0")],
        ),
    )?;
    let (view, doc) = current_ref!(f.editor);
    assert_eq!(doc.text(), "firstablast\n");
    assert_eq!(doc.selection(view.id), &Selection::single(7, 11));
    f.editor.insert_char('x')?;
    assert_eq!(current_ref!(f.editor).1.text(), "firstabx\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn background_workspace_snippets_expand_without_revealing_or_selecting_the_document(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("current\n")?;
    let focus = f.editor.tree.focus();
    let original = current_ref!(f.editor).1.id();
    let selection = current_ref!(f.editor).1.selection(focus).clone();
    let path = f.dir.path().join("background.words");
    std::fs::write(&path, "\n")?;
    let id = f.editor.open(&path, Action::Load)?;
    f.editor.apply_workspace_edit(
        OffsetEncoding::Utf8,
        &edit(
            lsp::Url::from_file_path(&path).unwrap(),
            vec![snippet(0, 0, "${1:item}-$TM_FILENAME-$1$0")],
        ),
    )?;
    assert_eq!(f.editor.tree.focus(), focus);
    assert_eq!(current_ref!(f.editor).1.id(), original);
    assert_eq!(current_ref!(f.editor).1.selection(focus), &selection);
    assert_eq!(f.editor.mode(), Mode::Normal);
    assert_eq!(
        f.editor.document(id).unwrap().text(),
        "item-background.words-item\n"
    );
    assert!(f.editor.document(id).unwrap().active_snippet().is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_snippet_variables_use_the_replaced_text_even_in_insert_mode(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("z\n")?;
    f.editor.set_mode(Mode::Insert);
    let uri = current_ref!(f.editor).1.url().unwrap();
    f.editor.apply_workspace_edit(
        OffsetEncoding::Utf8,
        &edit(uri, vec![snippet(0, 1, "${1:<$TM_SELECTED_TEXT>}$0")]),
    )?;
    let (view, doc) = current_ref!(f.editor);
    assert_eq!(doc.text(), "<z>\n");
    assert_eq!(doc.selection(view.id), &Selection::single(0, 3));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_snippets_reuse_choice_and_mirror_editing() -> anyhow::Result<()> {
    let mut f = Fixture::new("\n")?;
    let uri = current_ref!(f.editor).1.url().unwrap();
    f.editor.apply_workspace_edit(
        OffsetEncoding::Utf8,
        &edit(uri, vec![snippet(0, 0, "${1|red,blue|}-$1$0")]),
    )?;
    assert_eq!(current_ref!(f.editor).1.text(), "red-red\n");
    assert!(f.editor.cycle_snippet_choice(Direction::Forward)?);
    assert_eq!(current_ref!(f.editor).1.text(), "blue-blue\n");
    f.editor.insert_char('x')?;
    assert_eq!(current_ref!(f.editor).1.text(), "x-x\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_final_cursor_stop_works_without_starting_a_placeholder_session() -> anyhow::Result<()> {
    let mut f = Fixture::new("\n")?;
    let uri = current_ref!(f.editor).1.url().unwrap();
    f.editor.apply_workspace_edit(
        OffsetEncoding::Utf8,
        &edit(uri, vec![snippet(0, 0, "left$0right")]),
    )?;
    let (view, doc) = current_ref!(f.editor);
    assert_eq!(doc.text(), "leftright\n");
    assert_eq!(doc.selection(view.id).primary().from(), 4);
    assert!(doc.active_snippet().is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_workspace_snippets_or_ranges_never_apply_a_subset_of_document_edits(
) -> anyhow::Result<()> {
    for (edits, expected) in [
        (
            vec![plain(0, 0, "prefix"), snippet(3, 4, "${1:${1:recursive}}")],
            "snippet",
        ),
        (vec![snippet(1, 2, "${1:invalid}")], "range"),
        (
            vec![snippet(3, 5, "${1:one}"), plain(4, 6, "overlap")],
            "range",
        ),
        (vec![snippet(5, 3, "${1:reversed}")], "range"),
    ] {
        let mut f = Fixture::new("😀 abc\n")?;
        let uri = current_ref!(f.editor).1.url().unwrap();
        let error = f
            .editor
            .apply_workspace_edit(OffsetEncoding::Utf16, &edit(uri, edits))
            .unwrap_err();
        if expected == "snippet" {
            assert!(matches!(error.kind, ApplyEditErrorKind::InvalidSnippet(_)));
        } else {
            assert!(matches!(error.kind, ApplyEditErrorKind::InvalidTextEdit(_)));
        }
        let (view, doc) = current!(f.editor);
        assert_eq!(doc.text(), "😀 abc\n");
        assert!(doc.active_snippet().is_none());
        assert!(!view::editing::undo(doc, view.id));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_workspace_snippet_kinds_and_stale_versions_are_rejected() -> anyhow::Result<()> {
    let mut f = Fixture::new("original\n")?;
    let doc = current_ref!(f.editor).1;
    let uri = doc.url().unwrap();
    let mut bad = snippet(0, 0, "${1:wrong}");
    let lsp::DocumentTextEdit::Snippet(ref mut bad_snippet) = bad else {
        unreachable!()
    };
    bad_snippet.snippet.kind = "unknown".into();
    let error = f
        .editor
        .apply_workspace_edit(OffsetEncoding::Utf8, &edit(uri.clone(), vec![bad]))
        .unwrap_err();
    assert!(matches!(
        error.kind,
        ApplyEditErrorKind::InvalidSnippetKind(_)
    ));
    let mut stale = edit(uri, vec![snippet(0, 0, "${1:wrong}")]);
    let Some(lsp::DocumentChanges::Edits(ref mut edits)) = stale.document_changes else {
        unreachable!()
    };
    edits[0].text_document.version = Some(current_ref!(f.editor).1.version() + 1);
    assert!(matches!(
        f.editor
            .apply_workspace_edit(OffsetEncoding::Utf8, &stale)
            .unwrap_err()
            .kind,
        ApplyEditErrorKind::Document(view::editing::EditError::DocumentChanged { .. })
    ));
    assert_eq!(current_ref!(f.editor).1.text(), "original\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn multiline_workspace_snippets_keep_document_line_endings_and_navigable_ranges(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("  old\r\n")?;
    let uri = current_ref!(f.editor).1.url().unwrap();
    f.editor.apply_workspace_edit(
        OffsetEncoding::Utf8,
        &edit(uri, vec![snippet(2, 5, "${1:first}\n\t${2:second}$0")]),
    )?;
    let (view, doc) = current_ref!(f.editor);
    assert!(doc.text().to_string().starts_with("  first\r\n"));
    assert!(doc.text().to_string().ends_with("second\r\n"));
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .fragment(doc.text().slice(..)),
        "first"
    );
    assert_eq!(
        f.editor.move_snippet_tabstop(Direction::Forward),
        SnippetNavigation::Moved
    );
    let (view, doc) = current_ref!(f.editor);
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .fragment(doc.text().slice(..)),
        "second"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn resolved_variables_do_not_leave_empty_conditional_tabstops() -> anyhow::Result<()> {
    let mut f = Fixture::new("\n")?;
    let uri = current_ref!(f.editor).1.url().unwrap();
    f.editor.apply_workspace_edit(
        OffsetEncoding::Utf8,
        &edit(uri, vec![snippet(0, 0, "${TM_FILENAME:${1:unused}}$0")]),
    )?;
    let (view, doc) = current_ref!(f.editor);
    assert_eq!(doc.text(), "document.words\n");
    assert_eq!(
        doc.selection(view.id).primary().from(),
        "document.words".len()
    );
    assert!(doc.active_snippet().is_none());
    Ok(())
}
