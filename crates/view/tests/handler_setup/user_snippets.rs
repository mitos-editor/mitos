use super::Fixture;
use editor_core::{Range, Selection};
use std::time::Duration;
use view::{current, current_ref, document::Mode, features::completion};

fn catalog() -> snippets::Catalog {
    snippets::Catalog::parse(
        r#"
        [[snippet]]
        name = "Greeting"
        prefix = ["hello", "hi"]
        body = '${1:world}:$TM_FILENAME:$CURSOR_NUMBER$0'
        [[snippet]]
        name = "Rust only"
        prefix = "rs"
        scope = "rust"
        body = 'fn ${1:name}() {$0}'
    "#,
    )
    .unwrap()
}

async fn request(
    f: &mut Fixture,
) -> anyhow::Result<(
    Vec<completion::CompletionItem>,
    completion::CompletionSession,
)> {
    f.editor.set_mode(Mode::Insert);
    let (view, doc) = current_ref!(f.editor);
    let cursor = doc
        .selection(view.id)
        .primary()
        .cursor(doc.text().slice(..));
    f.editor
        .handlers()
        .trigger_completions(cursor, doc.id(), view.id);
    Ok(tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let callback = f.callbacks.recv().await.unwrap();
            callback(&mut f.editor);
            while let Some(update) = completion::next_update(&mut f.editor) {
                if let Some(completion::CompletionChange::Show { items, session }) =
                    update.apply(&mut f.editor)
                {
                    return (items, session);
                }
            }
        }
    })
    .await?)
}

#[tokio::test(flavor = "multi_thread")]
async fn native_snippets_complete_preview_accept_and_undo_without_lsp() -> anyhow::Result<()> {
    let mut f = Fixture::new("he\n")?;
    f.editor.set_snippet_catalog(catalog());
    let (view, doc) = current!(f.editor);
    doc.set_selection(view.id, Selection::point(2));
    let (items, session) = request(&mut f).await?;
    assert_eq!(
        items
            .iter()
            .filter(|item| matches!(item, completion::CompletionItem::Snippet(_)))
            .count(),
        2
    );
    assert!(!items.iter().any(|item| item.filter_text() == "rs"));
    let item = items
        .iter()
        .find(|item| item.filter_text() == "hello")
        .unwrap();
    assert!(f.editor.begin_completion(&session));
    assert!(f.editor.preview_completion(&session, item)?);
    let doc = current_ref!(f.editor).1;
    assert_eq!(doc.text(), "he\n");
    assert_eq!(
        f.editor.completion_presentation(doc).text(),
        "world:document.words:1\n"
    );
    f.editor.finish_completion();
    assert_eq!(current_ref!(f.editor).1.text(), "he\n");
    let (items, session) = request(&mut f).await?;
    let item = items
        .iter()
        .find(|item| item.filter_text() == "hello")
        .unwrap();
    assert!(f.editor.accept_completion(&session, item)?);
    f.editor.finish_completion();
    f.editor.insert_char('界')?;
    assert_eq!(current_ref!(f.editor).1.text(), "界:document.words:1\n");
    let (view, doc) = current!(f.editor);
    assert!(view::editing::undo(doc, view.id));
    assert_eq!(doc.text(), "he\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn replacement_catalog_and_disabled_provider_reject_old_results() -> anyhow::Result<()> {
    let mut f = Fixture::new("he\n")?;
    f.editor.set_snippet_catalog(catalog());
    let (view, doc) = current!(f.editor);
    doc.set_selection(view.id, Selection::point(2));
    let (items, session) = request(&mut f).await?;
    let item = items
        .iter()
        .find(|item| item.filter_text() == "hello")
        .unwrap();
    f.editor.set_snippet_catalog(snippets::Catalog::default());
    assert!(!f.editor.accept_completion(&session, item)?);
    f.editor.set_snippet_catalog(catalog());
    let (items, session) = request(&mut f).await?;
    let item = items
        .iter()
        .find(|item| item.filter_text() == "hello")
        .unwrap();
    f.configure(|config| config.completion.snippets = false);
    assert!(!f.editor.preview_completion(&session, item)?);
    assert_eq!(current_ref!(f.editor).1.text(), "he\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn native_snippets_keep_multiple_cursors_and_variable_contexts() -> anyhow::Result<()> {
    let mut f = Fixture::new("he\nhe\n")?;
    f.editor.set_snippet_catalog(catalog());
    let (view, doc) = current!(f.editor);
    doc.set_selection(
        view.id,
        Selection::new(vec![Range::point(2), Range::point(5)].into(), 1),
    );
    let (items, session) = request(&mut f).await?;
    let item = items
        .iter()
        .find(|item| item.filter_text() == "hello")
        .unwrap();
    assert!(f.editor.accept_completion(&session, item)?);
    f.editor.finish_completion();
    f.editor.insert_char('x')?;
    let (view, doc) = current_ref!(f.editor);
    assert_eq!(doc.text(), "x:document.words:1\nx:document.words:2\n");
    assert_eq!(doc.selection(view.id).primary_index(), 1);
    Ok(())
}

async fn disk_fixture() -> anyhow::Result<Fixture> {
    let mut f = Fixture::new("\n")?;
    std::fs::create_dir_all(f.dir.path().join(".mitos"))?;
    std::fs::write(
        f.dir.path().join("user-snippets.toml"),
        "[[snippet]]\nname='shared'\nprefix='global'\nbody='global'",
    )?;
    std::fs::write(
        f.dir.path().join(".mitos/snippets.toml"),
        "[[snippet]]\nname='shared'\nprefix='local'\nbody='local'",
    )?;
    let path = f.dir.path().join("project.words");
    std::fs::write(&path, "\n")?;
    f.editor.open(&path, view::editor::Action::Replace)?;
    f.editor.set_snippet_resources(loader::snippets::Resources {
        user_file: f.dir.path().join("user-snippets.toml"),
    });
    f.editor.workspace_trust =
        workspace::trust::WorkspaceTrust::fully_trusted(f.dir.path().join("trust"));
    Ok(f)
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_overrides_reload_atomically_and_deletions_restore_global_definitions(
) -> anyhow::Result<()> {
    let mut f = disk_fixture().await?;
    let candidate = f.editor.snippet_candidates().pop().unwrap();
    assert_eq!(candidate.snippet.body, "local");
    std::fs::write(f.dir.path().join("user-snippets.toml"), "invalid = true")?;
    assert!(f.editor.reload_snippets().is_err());
    assert_eq!(f.editor.snippet_candidates()[0].snippet.body, "local");
    std::fs::write(
        f.dir.path().join("user-snippets.toml"),
        "[[snippet]]\nname='shared'\nprefix='global'\nbody='new global'",
    )?;
    std::fs::remove_file(f.dir.path().join(".mitos/snippets.toml"))?;
    f.editor.reload_snippets()?;
    assert!(f.editor.insert_user_snippet(&candidate).is_err());
    let candidate = f.editor.snippet_candidates().pop().unwrap();
    assert_eq!(candidate.snippet.body, "new global");
    f.editor.insert_user_snippet(&candidate)?;
    assert_eq!(current_ref!(f.editor).1.text(), "new global\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn project_definitions_require_trust_and_old_candidates_cannot_cross_trust_changes(
) -> anyhow::Result<()> {
    let mut f = disk_fixture().await?;
    let candidate = f.editor.snippet_candidates().pop().unwrap();
    let (items, session) = request(&mut f).await?;
    let item = items
        .iter()
        .find(|item| item.filter_text() == "local")
        .unwrap();
    f.editor.workspace_trust = workspace::trust::WorkspaceTrust::new(
        workspace::trust::Config::default(),
        f.dir.path().join("trust"),
    );
    assert!(!f.editor.accept_completion(&session, item)?);
    assert!(f.editor.insert_user_snippet(&candidate).is_err());
    assert_eq!(f.editor.snippet_candidates()[0].snippet.body, "global");
    let root = current_ref!(f.editor).1.workspace_root().to_path_buf();
    assert!(f.editor.workspace_trust.workspace_restricted(&root));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn picker_candidates_reject_selection_edits_and_other_editors() -> anyhow::Result<()> {
    let mut f = Fixture::new("ab\n")?;
    f.editor.set_snippet_catalog(catalog());
    let candidate = f.editor.snippet_candidates().pop().unwrap();
    let (view, doc) = current!(f.editor);
    doc.set_selection(view.id, Selection::point(1));
    assert!(f.editor.insert_user_snippet(&candidate).is_err());
    let candidate = f.editor.snippet_candidates().pop().unwrap();
    f.editor.insert_char('x')?;
    assert!(f.editor.insert_user_snippet(&candidate).is_err());
    let candidate = f.editor.snippet_candidates().pop().unwrap();
    let mut other = Fixture::new("ab\n")?;
    other.editor.set_snippet_catalog(catalog());
    assert!(other.editor.insert_user_snippet(&candidate).is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn prefixless_snippets_replace_selected_text_and_support_nested_choices() -> anyhow::Result<()>
{
    let mut f = Fixture::new("world\r\n")?;
    f.editor.set_snippet_catalog(snippets::Catalog::parse(
        "[[snippet]]\nname='Wrap'\nbody='<${1|one,two|}>$TM_SELECTED_TEXT$0'",
    )?);
    f.editor.set_mode(Mode::Select);
    let (view, doc) = current!(f.editor);
    doc.set_selection(view.id, Selection::single(0, 5));
    let candidate = f.editor.snippet_candidates().pop().unwrap();
    f.editor.insert_user_snippet(&candidate)?;
    assert_eq!(current_ref!(f.editor).1.text(), "<one>world\r\n");
    assert!(f
        .editor
        .cycle_snippet_choice(editor_core::movement::Direction::Forward)?);
    assert_eq!(current_ref!(f.editor).1.text(), "<two>world\r\n");
    f.editor
        .insert_snippet(&snippets::Snippet::parse("${1:inner}$0")?)?;
    f.editor.insert_char('X')?;
    assert_eq!(current_ref!(f.editor).1.text(), "<X>world\r\n");
    let (view, doc) = current!(f.editor);
    assert!(view::editing::undo(doc, view.id));
    assert_eq!(doc.text(), "world\r\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn variables_are_stable_across_preview_acceptance_and_repeated_occurrences(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("id\n")?;
    f.editor.set_snippet_catalog(snippets::Catalog::parse("[[snippet]]\nname='Identity'\nprefix='id'\nbody='$UUID|$UUID|$RANDOM|$RANDOM_HEX|$CURRENT_YEAR'")?);
    let (view, doc) = current!(f.editor);
    doc.set_selection(view.id, Selection::point(2));
    let (items, session) = request(&mut f).await?;
    let item = items
        .iter()
        .find(|item| item.filter_text() == "id")
        .unwrap();
    f.editor.begin_completion(&session);
    assert!(f.editor.preview_completion(&session, item)?);
    let preview = f
        .editor
        .completion_presentation(current_ref!(f.editor).1)
        .text()
        .to_string();
    let parts: Vec<_> = preview.trim_end().split('|').collect();
    assert_eq!(parts[0], parts[1]);
    assert_eq!(parts[0].len(), 36);
    assert_eq!(parts[0].as_bytes()[14], b'4');
    assert_eq!(parts[2].len(), 6);
    assert!(parts[2].chars().all(|c| c.is_ascii_digit()));
    assert_eq!(parts[3].len(), 6);
    assert_eq!(parts[4].len(), 4);
    assert!(f.editor.accept_completion(&session, item)?);
    f.editor.finish_completion();
    assert_eq!(current_ref!(f.editor).1.text().to_string(), preview);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn language_changes_invalidate_both_completion_and_picker_choices() -> anyhow::Result<()> {
    let mut f = Fixture::with_languages(
        "he\n",
        r#"
        [[language]]
        name = "first"
        scope = "text.first"
        file-types = ["words"]
        [[language]]
        name = "second"
        scope = "text.second"
        file-types = ["other"]
    "#,
    )?;
    f.editor.set_snippet_catalog(catalog());
    let candidate = f.editor.snippet_candidates().pop().unwrap();
    let (view, doc) = current!(f.editor);
    doc.set_selection(view.id, Selection::point(2));
    let (items, session) = request(&mut f).await?;
    let item = items
        .iter()
        .find(|item| item.filter_text() == "hello")
        .unwrap();
    let id = current_ref!(f.editor).1.id();
    f.editor.set_document_language(id, Some("second"))?;
    assert!(!f.editor.accept_completion(&session, item)?);
    assert!(f.editor.insert_user_snippet(&candidate).is_err());
    assert_eq!(current_ref!(f.editor).1.text(), "he\n");
    Ok(())
}

struct SnippetClipboard(std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl ::clipboard::ClipboardBackend for SnippetClipboard {
    fn name(&self, _: &::clipboard::ClipboardProvider) -> String {
        "snippet-test".into()
    }
    fn get_contents(
        &self,
        _: &::clipboard::ClipboardProvider,
        _: ::clipboard::ClipboardType,
    ) -> ::clipboard::Result<String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok("clipboard\nvalue".into())
    }
    fn set_contents(
        &self,
        _: &::clipboard::ClipboardProvider,
        _: &str,
        _: ::clipboard::ClipboardType,
    ) -> ::clipboard::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn clipboard_is_read_only_for_referenced_variables_and_only_once_per_insertion(
) -> anyhow::Result<()> {
    let mut f = Fixture::new("\n")?;
    let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    f.editor
        .registers
        .set_clipboard_backend(Box::new(SnippetClipboard(reads.clone())));
    f.editor
        .registers
        .write('+', vec!["clipboard".into(), "value".into()])?;
    f.editor
        .insert_snippet(&snippets::Snippet::parse("plain")?)?;
    assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 0);
    f.editor
        .insert_snippet(&snippets::Snippet::parse("${1:$CLIPBOARD}|$CLIPBOARD$0")?)?;
    assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert!(current_ref!(f.editor)
        .1
        .text()
        .to_string()
        .contains("clipboard\nvalue|clipboard\nvalue"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn nested_snippets_replace_even_a_single_character_placeholder() -> anyhow::Result<()> {
    let mut f = Fixture::new("\n")?;
    f.editor
        .insert_snippet(&snippets::Snippet::parse("${1:x} ${2:after}$0")?)?;
    f.editor
        .insert_snippet(&snippets::Snippet::parse("<${1:$TM_SELECTED_TEXT}>$0")?)?;
    assert_eq!(current_ref!(f.editor).1.text(), "<x> after\n");
    f.editor.insert_char('y')?;
    assert_eq!(current_ref!(f.editor).1.text(), "<y> after\n");
    f.editor
        .move_snippet_tabstop(editor_core::movement::Direction::Forward);
    f.editor
        .move_snippet_tabstop(editor_core::movement::Direction::Forward);
    f.editor.insert_char('z')?;
    assert_eq!(current_ref!(f.editor).1.text(), "<y> z\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn embedded_languages_select_their_own_snippets_and_comment_variables() -> anyhow::Result<()>
{
    let mut f = Fixture::with_languages("\n", include_str!("../../../../languages.toml"))?;
    let path = f.dir.path().join("embedded.html");
    let source = "<script>\nlet value = 1;\n</script>\n";
    std::fs::write(&path, source)?;
    f.editor.open(&path, view::editor::Action::Replace)?;
    let (view, doc) = current!(f.editor);
    doc.set_selection(view.id, Selection::point(14));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let doc = current_ref!(f.editor).1;
            if doc
                .language_config_at(14)
                .is_some_and(|language| language.language_id == "javascript")
            {
                break;
            }
            let callback = f.callbacks.recv().await.unwrap();
            callback(&mut f.editor);
        }
    })
    .await?;
    f.editor.set_snippet_catalog(snippets::Catalog::parse("[[snippet]]\nname='HTML'\nscope='html'\nbody='html'\n[[snippet]]\nname='JS'\nscope='javascript'\nbody='$LINE_COMMENT comment'")?);
    let candidates = f.editor.snippet_candidates();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].snippet.name, "JS");
    f.editor.insert_user_snippet(&candidates[0])?;
    assert!(current_ref!(f.editor)
        .1
        .text()
        .to_string()
        .contains("// comment"));
    Ok(())
}
