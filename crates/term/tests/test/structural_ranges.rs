//! LSP structural ranges override syntax, preserve selections, and reject stale publication.
use std::path::PathBuf;

use editor_core::{Range, Selection, Transaction};
use lsp_client::LanguageServerId;
use stdx::rope::RopeExt as _;
use term::application::TestApplication;
use tokio::sync::mpsc;
use view::{
    callbacks::EditorCallback, current, current_ref, document::FoldAction,
    editing::syntax::SelectionAction, editor::Action,
    features::structural_ranges::StructuralRangesHandler,
};

use super::helpers::{
    callbacks,
    lsp::{self, ServerConfig},
    test_config,
};

struct Fixture {
    app: TestApplication,
    callbacks: mpsc::UnboundedReceiver<(bool, EditorCallback)>,
    log: PathBuf,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new(text: &str, language: &str, args: &[&str]) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let (mut app, callbacks, gate) = lsp::feature_app(
            dir.path(),
            text,
            language,
            &["structure"],
            test_config(),
            |name| {
                let mut server = ServerConfig::feature(name, "--structural-ranges", dir.path());
                for arg in args {
                    server = server.arg(arg);
                }
                server
            },
            |handlers, sender, _| handlers.structural_ranges = StructuralRangesHandler::new(sender),
        )?;
        gate.release()?;
        lsp::initialize(&mut app, 1).await?;
        let (view, doc) = current!(app.editor);
        doc.set_selection(view.id, Selection::point(2));
        Ok(Self {
            app,
            callbacks,
            log: lsp::log_path(dir.path(), "structure"),
            _dir: dir,
        })
    }

    fn view(&self) -> view::ViewId {
        current_ref!(self.app.editor).0.id
    }
    fn server(&self) -> LanguageServerId {
        current_ref!(self.app.editor)
            .1
            .language_servers()
            .next()
            .unwrap()
            .id()
    }
    async fn response(&mut self) -> anyhow::Result<EditorCallback> {
        callbacks::response(&mut self.app, &mut self.callbacks).await
    }
    fn select(&mut self, selection: Selection) {
        let (view, doc) = current!(self.app.editor);
        doc.set_selection(view.id, selection);
    }
    fn folds(&self) -> Vec<std::ops::Range<usize>> {
        let (view, doc) = current_ref!(self.app.editor);
        doc.folded_ranges(view.id)
            .iter()
            .map(|conceal| conceal.range.clone())
            .collect()
    }
    fn requests(&self, method: &str) -> anyhow::Result<usize> {
        Ok(lsp::read_log(&self.log)?
            .iter()
            .filter(|message| message["method"] == method)
            .count())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn lsp_folds_work_without_grammar_and_cache_whole_line_ranges() -> anyhow::Result<()> {
    for ending in ["\n", "\r\n"] {
        let text = format!("😀 alpha beta{ending}middle{ending}last{ending}next{ending}");
        let mut f = Fixture::new(&text, "structure-test", &[]).await?;
        let view = f.view();
        f.app.editor.fold(view, FoldAction::CloseAll, 1);
        f.response().await?(&mut f.app.editor);
        let (_, doc) = current_ref!(f.app.editor);
        assert!(doc.syntax().is_none());
        let start = editor_core::line_ending::line_end_char_index(&doc.text().slice(..), 0);
        let end = editor_core::line_ending::line_end_char_index(&doc.text().slice(..), 2);
        assert_eq!(f.folds(), vec![start..end]);
        assert!(!doc.is_modified());
        f.app.editor.fold(view, FoldAction::OpenAll, 1);
        f.app.editor.fold(view, FoldAction::CloseAll, 1);
        assert_eq!(f.folds(), vec![start..end]);
        assert_eq!(f.requests("textDocument/foldingRange")?, 1);
        let records = lsp::read_log(&f.log)?;
        let caps = &records
            .iter()
            .find(|record| record["method"] == "initialize")
            .unwrap()["params"]["capabilities"];
        assert_eq!(caps["workspace"]["foldingRange"]["refreshSupport"], true);
        assert_eq!(
            caps["textDocument"]["foldingRange"]["lineFoldingOnly"],
            true
        );
        assert_eq!(
            caps["textDocument"]["selectionRange"]["dynamicRegistration"],
            false
        );
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn lsp_folds_override_syntax_and_empty_reports_are_authoritative() -> anyhow::Result<()> {
    for empty in [false, true] {
        let mut f = Fixture::new(
            "fn main() {\n    work();\n}\nnext();\n",
            "rust",
            if empty { &["--empty-ranges"] } else { &[] },
        )
        .await?;
        let view = f.view();
        f.app.editor.fold(view, FoldAction::CloseAll, 1);
        f.response().await?(&mut f.app.editor);
        if empty {
            assert!(f.folds().is_empty());
        } else {
            let doc = current_ref!(f.app.editor).1;
            // LSP folds the closing brace line; syntax keeps the delimiter visible.
            assert_eq!(
                f.folds()[0].end,
                editor_core::line_ending::line_end_char_index(&doc.text().slice(..), 2)
            );
        }
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unavailable_server_ranges_fall_back_to_syntax() -> anyhow::Result<()> {
    for flag in ["--null-ranges", "--reject-ranges"] {
        let mut f = Fixture::new("fn main() {\n    work();\n}\nnext();\n", "rust", &[flag]).await?;
        let view = f.view();
        f.app.editor.fold(view, FoldAction::CloseAll, 1);
        f.response().await?(&mut f.app.editor);
        let doc = current_ref!(f.app.editor).1;
        assert_eq!(f.folds().len(), 1);
        assert_eq!(f.folds()[0].end, doc.text().line_to_char_idx(2));
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn folding_refresh_recomputes_cached_ranges_and_replies() -> anyhow::Result<()> {
    let mut f = Fixture::new(
        "header\nmiddle\nlast\nnext\n",
        "structure-test",
        &["--refresh-folds"],
    )
    .await?;
    let view = f.view();
    f.app.editor.fold(view, FoldAction::CloseAll, 1);
    f.response().await?(&mut f.app.editor);
    f.app.editor.fold(view, FoldAction::OpenAll, 1);
    let server = f.server();
    f.app
        .handle_language_server_message(
            serde_json::from_value(serde_json::json!({
                "jsonrpc":"2.0","id":"refresh","method":"workspace/foldingRange/refresh"
            }))?,
            server,
        )
        .await;
    f.response().await?(&mut f.app.editor);
    f.app.editor.fold(view, FoldAction::CloseAll, 1);
    let doc = current_ref!(f.app.editor).1;
    assert_eq!(
        f.folds()[0].start,
        editor_core::line_ending::line_end_char_index(&doc.text().slice(..), 1)
    );
    assert_eq!(f.requests("textDocument/foldingRange")?, 2);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn selection_ranges_use_utf16_and_preserve_multicursor_direction_and_history(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("😀 alpha beta\nmiddle\nlast\n", "structure-test", &[]).await?;
    let original = Selection::new(vec![Range::point(2), Range::new(11, 10)].into(), 1);
    f.select(original);
    let view = f.view();
    let original = current_ref!(f.app.editor).1.selection(view).clone();
    f.app.editor.select_structure(view, SelectionAction::Expand);
    f.response().await?(&mut f.app.editor);
    let selection = current_ref!(f.app.editor).1.selection(view);
    assert_eq!(selection.ranges(), &[Range::new(2, 7), Range::new(12, 8)]);
    assert_eq!(selection.primary_index(), 1);
    let records = lsp::read_log(&f.log)?;
    let request = records
        .iter()
        .find(|message| message["method"] == "textDocument/selectionRange")
        .unwrap();
    assert_eq!(
        request["params"]["positions"],
        serde_json::json!([{"line":0,"character":3},{"line":0,"character":11}])
    );
    f.app.editor.select_structure(view, SelectionAction::Shrink);
    assert_eq!(current_ref!(f.app.editor).1.selection(view), &original);
    assert_eq!(f.requests("textDocument/selectionRange")?, 1);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_ranges_cannot_modify_moved_edited_rebound_or_detached_targets() -> anyhow::Result<()>
{
    for folding in [false, true] {
        for mutation in [
            "move", "edit", "rebind", "server", "language", "mode", "cancel",
        ] {
            let mut f =
                Fixture::new("😀 alpha beta\nmiddle\nlast\nnext\n", "structure-test", &[]).await?;
            let view = f.view();
            if folding {
                f.app.editor.fold(view, FoldAction::CloseAll, 1);
            } else {
                f.app.editor.select_structure(view, SelectionAction::Expand);
            }
            let response = f.response().await?;
            let id = current_ref!(f.app.editor).1.id();
            match mutation {
                "move" => f.select(Selection::point(8)),
                "edit" => {
                    let (view, doc) = current!(f.app.editor);
                    let edit =
                        Transaction::insert(doc.text(), &Selection::point(0), "prefix ".into());
                    view::editing::apply(doc, &edit, view.id)?;
                }
                "rebind" => {
                    let path = f._dir.path().join("other.txt");
                    std::fs::write(&path, "other\n")?;
                    f.app.editor.open(&path, Action::Replace)?;
                }
                "server" => {
                    f.app.editor.handle_language_server_exit(f.server());
                }
                "language" => {
                    f.app.editor.set_document_language(id, None)?;
                }
                "mode" => {
                    f.app.editor.set_mode(view::document::Mode::Select);
                    f.app.editor.set_mode(view::document::Mode::Normal);
                }
                "cancel" if folding => f.app.editor.fold(view, FoldAction::OpenAll, 1),
                "cancel" => {
                    f.app.editor.select_structure(view, SelectionAction::Shrink);
                }
                _ => unreachable!(),
            }
            let before = current_ref!(f.app.editor).1.selection(view).clone();
            response(&mut f.app.editor);
            assert_eq!(
                current_ref!(f.app.editor).1.selection(view),
                &before,
                "{folding} {mutation}"
            );
            assert!(
                f.app
                    .editor
                    .document(id)
                    .unwrap()
                    .folded_ranges(view)
                    .is_empty(),
                "{folding} {mutation}"
            );
            assert!(f.app.close().await.is_empty());
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_selection_ranges_preserve_syntax_fallback() -> anyhow::Result<()> {
    let mut f = Fixture::new(
        "😀 alpha beta\nmiddle\nlast\n",
        "structure-test",
        &["--invalid-ranges"],
    )
    .await?;
    let view = f.view();
    let before = current_ref!(f.app.editor).1.selection(view).clone();
    f.app.editor.select_structure(view, SelectionAction::Expand);
    f.response().await?(&mut f.app.editor);
    assert_eq!(current_ref!(f.app.editor).1.selection(view), &before);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn repeated_expansions_wait_for_each_result_and_shrink_retraces_every_step(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("😀 alpha beta\nmiddle\nlast\n", "structure-test", &[]).await?;
    let view = f.view();
    let original = current_ref!(f.app.editor).1.selection(view).clone();
    f.app.editor.select_structure(view, SelectionAction::Expand);
    let first = f.response().await?;
    // This is how repeat_last_motion invokes another expansion before publication.
    f.app.editor.select_structure(view, SelectionAction::Expand);
    first(&mut f.app.editor);
    assert_eq!(
        current_ref!(f.app.editor).1.selection(view).ranges(),
        &[Range::new(2, 7)]
    );
    f.response().await?(&mut f.app.editor);
    assert_eq!(
        current_ref!(f.app.editor).1.selection(view).ranges(),
        &[Range::new(0, 12)]
    );
    f.app.editor.select_structure(view, SelectionAction::Shrink);
    assert_eq!(
        current_ref!(f.app.editor).1.selection(view).ranges(),
        &[Range::new(2, 7)]
    );
    f.app.editor.select_structure(view, SelectionAction::Shrink);
    assert_eq!(current_ref!(f.app.editor).1.selection(view), &original);
    assert_eq!(f.requests("textDocument/selectionRange")?, 2);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_fold_commands_preserve_repeated_close_and_toggle_operations() -> anyhow::Result<()>
{
    for action in [FoldAction::Close, FoldAction::Toggle] {
        let mut f = Fixture::new(
            "outer\ninner\nend inner\nend outer\nnext\n",
            "structure-test",
            &["--nested-folds"],
        )
        .await?;
        let view = f.view();
        let inside = current_ref!(f.app.editor).1.text().line_to_char_idx(1);
        f.select(Selection::point(inside));
        f.app.editor.fold(view, action, 1);
        let response = f.response().await?;
        f.app.editor.fold(view, action, 1);
        response(&mut f.app.editor);
        if matches!(action, FoldAction::Toggle) {
            assert!(f.folds().is_empty());
        } else {
            let text = current_ref!(f.app.editor).1.text();
            assert_eq!(
                f.folds(),
                vec![
                    editor_core::line_ending::line_end_char_index(&text.slice(..), 0)
                        ..editor_core::line_ending::line_end_char_index(&text.slice(..), 3)
                ]
            );
            f.app.editor.fold(view, FoldAction::Open, 1);
            let text = current_ref!(f.app.editor).1.text();
            assert_eq!(
                f.folds(),
                vec![
                    editor_core::line_ending::line_end_char_index(&text.slice(..), 1)
                        ..editor_core::line_ending::line_end_char_index(&text.slice(..), 2)
                ]
            );
        }
        assert_eq!(f.requests("textDocument/foldingRange")?, 1);
        assert!(f.app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_fold_commands_are_independent_between_splits() -> anyhow::Result<()> {
    let mut f = Fixture::new("header\nmiddle\nlast\nnext\n", "structure-test", &[]).await?;
    let first = f.view();
    let document = current_ref!(f.app.editor).1.id();
    f.app.editor.fold(first, FoldAction::CloseAll, 1);
    let response = f.response().await?;
    f.app.editor.switch(document, Action::VerticalSplit);
    let second = f.view();
    f.app.editor.fold(second, FoldAction::CloseAll, 1);
    response(&mut f.app.editor);
    let doc = f.app.editor.document(document).unwrap();
    assert_eq!(doc.folded_ranges(first).len(), 1);
    assert_eq!(doc.folded_ranges(second), doc.folded_ranges(first));
    assert_eq!(f.requests("textDocument/foldingRange")?, 1);
    assert!(f.app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_fold_commands_survive_server_refresh() -> anyhow::Result<()> {
    let mut f = Fixture::new(
        "header\nmiddle\nlast\nnext\n",
        "structure-test",
        &["--refresh-folds"],
    )
    .await?;
    let view = f.view();
    f.app.editor.fold(view, FoldAction::CloseAll, 1);
    let obsolete = f.response().await?;
    f.app.editor.refresh_folding_ranges(f.server());
    obsolete(&mut f.app.editor);
    f.response().await?(&mut f.app.editor);
    let doc = current_ref!(f.app.editor).1;
    assert_eq!(
        f.folds(),
        vec![
            editor_core::line_ending::line_end_char_index(&doc.text().slice(..), 1)
                ..editor_core::line_ending::line_end_char_index(&doc.text().slice(..), 2)
        ]
    );
    assert!(f.app.close().await.is_empty());
    Ok(())
}
