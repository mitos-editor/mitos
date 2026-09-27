//! Shared services run through the public view API without a terminal or compositor.
#[path = "handler_setup/workspace_trust.rs"]
mod workspace_trust;
use std::{sync::Arc, time::Duration};

use anyhow::Context as _;
use arc_swap::ArcSwap;
use editor_core::{syntax, Selection, Transaction};
use tokio::sync::mpsc;
use view::{
    callbacks::{EditorCallback, EditorCallbackSender},
    config::Config,
    current, current_ref,
    document::Mode,
    editor::Action,
    graphics::Rect,
    handlers::{completion, Handlers},
    theme, Editor,
};

struct Fixture {
    editor: Editor,
    config: Arc<ArcSwap<Config>>,
    callbacks: mpsc::UnboundedReceiver<EditorCallback>,
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new(text: &str) -> anyhow::Result<Self> {
        Self::with_languages(text, "language = []")
    }

    fn with_languages(text: &str, languages: &str) -> anyhow::Result<Self> {
        Self::with_resources(text, languages, loader::syntax::Resources::default())
    }

    fn with_resources(
        text: &str,
        languages: &str,
        resources: loader::syntax::Resources,
    ) -> anyhow::Result<Self> {
        let mut config = Config::default();
        config.file_watcher.enable = false;
        config.auto_reload.enable = false;
        config.auto_reload.poll.enable = false;
        config.lsp.enable = false;
        config.auto_completion = false;
        config.word_completion.enable = true;
        config.path_completion = false;
        let config = Arc::new(ArcSwap::from_pointee(config));
        let (tx, callbacks) = mpsc::unbounded_channel();
        let blocking = tx.clone();
        let callbacks_tx = EditorCallbackSender::new(
            move |callback| {
                let tx = tx.clone();
                async move {
                    let _ = tx.send(callback);
                }
            },
            move |callback| {
                let _ = blocking.send(callback);
            },
        );
        let handlers = Handlers::new(&config.load(), callbacks_tx);
        let mut editor = Editor::new(
            Rect::new(0, 0, 80, 24),
            Arc::new(theme::Loader::new(loader::theme::Resources::new(vec![]))),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::new(
                toml::from_str(languages)?,
                resources,
            )?)),
            config.clone(),
            handlers,
            loader::workspace_trust::WorkspaceTrust::fully_trusted(),
        );
        editor.new_file(Action::VerticalSplit);
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("document.words");
        std::fs::write(&path, text)?;
        editor.open(&path, Action::Replace)?;
        Ok(Self {
            editor,
            config,
            callbacks,
            dir,
        })
    }

    fn replace(&mut self, text: &str) {
        let (view, doc) = current!(self.editor);
        let transaction = Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some(text.into()))].into_iter(),
        )
        .with_selection(Selection::point(0));
        assert!(doc.apply(&transaction, view.id));
    }

    fn words(&self) -> Vec<String> {
        let mut words = self.editor.handlers.word_index().matches("");
        words.sort();
        words
    }

    async fn expect_words(&self, words: &[&str]) -> anyhow::Result<()> {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.words() != words {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .with_context(|| format!("expected {words:?}, got {:?}", self.words()))
    }

    fn word_completion(&mut self, enabled: bool) {
        let old = self.config.load_full();
        let mut new = (*old).clone();
        new.word_completion.enable = enabled;
        self.config.store(Arc::new(new));
        self.editor.refresh_config(&old);
    }

    fn close_current(&mut self) -> anyhow::Result<()> {
        let doc = current_ref!(self.editor).1.id();
        self.editor.new_file(Action::Replace);
        assert!(self.editor.close_document(doc, true).is_ok());
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn headless_setup_runs_completion_through_its_callback_destination() -> anyhow::Result<()> {
    let mut f = Fixture::new("alpha alphabet al\n")?;
    f.expect_words(&["alpha", "alphabet"]).await?;
    let (view, doc) = current!(f.editor);
    let cursor = doc.text().len_chars() - 1;
    doc.set_selection(view.id, Selection::point(cursor));
    f.editor.mode = Mode::Insert;
    let (view, doc) = current_ref!(f.editor);
    f.editor
        .handlers
        .trigger_completions(cursor, doc.id(), view.id);
    let items = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let callback = f.callbacks.recv().await.expect("callback destination open");
            callback(&mut f.editor);
            while let Some(update) = completion::next_update(&mut f.editor) {
                if let Some(completion::CompletionChange::Show { items, .. }) =
                    update.apply(&mut f.editor)
                {
                    return items;
                }
            }
        }
    })
    .await?;
    assert!(items.iter().any(|item| item.filter_text() == "alpha"));
    assert!(items.iter().any(|item| item.filter_text() == "alphabet"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn multiple_editors_index_only_their_own_documents_and_edits() -> anyhow::Result<()> {
    let mut first = Fixture::new("firstword\n")?;
    let mut second = Fixture::new("secondword\n")?;
    assert_eq!(
        current_ref!(first.editor).1.id(),
        current_ref!(second.editor).1.id()
    );
    first.expect_words(&["firstword"]).await?;
    second.expect_words(&["secondword"]).await?;
    first.replace("firstchanged\n");
    second.replace("secondchanged\n");
    first.expect_words(&["firstchanged"]).await?;
    second.expect_words(&["secondchanged"]).await?;
    // Closing must remove the word completely: duplicate hook registration leaks counts.
    first.close_current()?;
    first.expect_words(&[]).await?;
    second.expect_words(&["secondchanged"]).await?;
    drop(first);
    let mut replacement = Fixture::new("replacementword\n")?;
    replacement.expect_words(&["replacementword"]).await?;
    second.close_current()?;
    second.expect_words(&[]).await?;
    replacement.close_current()?;
    replacement.expect_words(&[]).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn configuration_reset_discards_debounced_edits_before_reindexing() -> anyhow::Result<()> {
    let mut f = Fixture::new("seedword\n")?;
    f.expect_words(&["seedword"]).await?;
    f.replace("pendingword\n");
    f.word_completion(false);
    f.expect_words(&[]).await?;
    // The index must remain empty after the old edit's one-second debounce deadline.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(f.words().is_empty());
    f.word_completion(true);
    f.expect_words(&["pendingword"]).await?;
    f.replace("finalword\n");
    f.close_current()?;
    f.expect_words(&[]).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn language_override_survives_global_word_completion_changes() -> anyhow::Result<()> {
    let mut f = Fixture::with_languages(
        "languageword\n",
        r#"
        [[language]]
        name = "words"
        scope = "source.words"
        file-types = ["words"]
        roots = []
        word-completion = { enable = true }
    "#,
    )?;
    let language_doc = current_ref!(f.editor).1.id();
    let other = f.dir.path().join("other.txt");
    std::fs::write(&other, "globalword\n")?;
    f.editor.open(&other, Action::Replace)?;
    f.expect_words(&["globalword", "languageword"]).await?;
    f.word_completion(false);
    f.expect_words(&["languageword"]).await?;
    f.word_completion(true);
    f.expect_words(&["globalword", "languageword"]).await?;
    f.close_current()?;
    f.expect_words(&["languageword"]).await?;
    assert!(f.editor.close_document(language_doc, true).is_ok());
    f.expect_words(&[]).await?;
    Ok(())
}

fn insert_snippet(editor: &mut Editor) -> anyhow::Result<()> {
    let snippet = snippets::Snippet::parse("${1:one} ${2:two}$0")?;
    let mut ctx = snippets::SnippetRenderCtx {
        resolve_var: Box::new(|_| None),
        tab_width: 4,
        indent_style: editor_core::indent::IndentStyle::Spaces(4),
        line_ending: "\n",
    };
    let (view, doc) = current!(editor);
    let (transaction, selection, rendered) = snippet.render(
        doc.text(),
        &Selection::point(0),
        |range| (range.from(), range.to()),
        &mut ctx,
    );
    assert!(doc.apply(&transaction, view.id));
    doc.set_selection(view.id, selection);
    doc.active_snippet = snippets::ActiveSnippet::new(rendered);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn snippet_lifecycle_is_installed_once_without_terminal_hooks() -> anyhow::Result<()> {
    let mut first = Fixture::new("\n\n")?;
    let _second = Fixture::new("\n")?;
    insert_snippet(&mut first.editor)?;
    let (view, doc) = current!(first.editor);
    let transaction = Transaction::change(doc.text(), [(0, 0, Some("xx".into()))].into_iter())
        .with_selection(Selection::point(4));
    assert!(doc.apply(&transaction, view.id));
    assert_eq!(
        doc.active_snippet
            .as_ref()
            .unwrap()
            .tabstops()
            .next()
            .unwrap()
            .ranges[0]
            .start,
        2
    );
    doc.set_selection(view.id, Selection::point(doc.text().len_chars() - 1));
    assert!(doc.active_snippet.is_none());
    insert_snippet(&mut first.editor)?;
    let (view, doc) = current!(first.editor);
    let transaction =
        Transaction::delete(doc.text(), [(0, doc.text().len_chars() - 1)].into_iter());
    assert!(doc.apply(&transaction, view.id));
    assert!(doc.active_snippet.is_none());
    insert_snippet(&mut first.editor)?;
    let id = current_ref!(first.editor).1.id();
    first.editor.switch(id, Action::VerticalSplit);
    assert!(current_ref!(first.editor).1.active_snippet.is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn batched_configuration_rebuild_keeps_all_documents() -> anyhow::Result<()> {
    let mut f = Fixture::new("seedword\n")?;
    // More documents than the old debounce channel's capacity, created without yielding.
    let mut expected = vec!["seedword".to_owned()];
    for i in 0..140 {
        f.editor.new_file(Action::Replace);
        let word = format!("batchword{i:03}");
        f.replace(&format!("{word}\n"));
        expected.push(word);
    }
    f.word_completion(false);
    f.word_completion(true);
    expected.sort();
    let expected: Vec<_> = expected.iter().map(String::as_str).collect();
    f.expect_words(&expected).await?;
    let documents: Vec<_> = f.editor.documents().map(|doc| doc.id()).collect();
    f.editor.new_file(Action::Replace);
    for document in documents {
        assert!(f.editor.close_document(document, true).is_ok());
    }
    f.expect_words(&[]).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn native_file_watching_reloads_without_terminal_setup() -> anyhow::Result<()> {
    let mut f = Fixture::new("before\n")?;
    let old = f.config.load_full();
    let mut config = (*old).clone();
    config.auto_reload.enable = true;
    config.file_watcher.enable = true;
    f.config.store(Arc::new(config));
    f.editor.refresh_config(&old);
    f.editor.file_watcher.add_root(f.dir.path());
    let path = f.dir.path().join("document.words");
    tokio::time::timeout(Duration::from_secs(10), async {
        while !f.editor.file_watcher.is_watching(&path) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .context("native root did not become ready")?;
    let replacement = f.dir.path().join("replacement");
    std::fs::write(&replacement, "after\n")?;
    std::fs::rename(replacement, &path)?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while current_ref!(f.editor).1.text() != "after\n" {
            let callback = f
                .callbacks
                .recv()
                .await
                .context("editor callback queue closed")?;
            callback(&mut f.editor);
        }
        anyhow::Ok(())
    })
    .await
    .context("native change did not reach its headless editor")??;
    assert!(!current_ref!(f.editor).1.is_modified());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn language_reload_retains_each_editors_selected_runtime() -> anyhow::Result<()> {
    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    for (dir, content) in [(&first, "; first source"), (&second, "; second source")] {
        let path = dir.path().join("queries/resource-test");
        std::fs::create_dir_all(&path)?;
        std::fs::write(path.join("highlights.scm"), content)?;
    }
    let mut a = Fixture::with_resources(
        "first\n",
        "language = []",
        loader::syntax::Resources::new(vec![first.path().into()]),
    )?;
    let mut b = Fixture::with_resources(
        "second\n",
        "language = []",
        loader::syntax::Resources::new(vec![second.path().into()]),
    )?;
    for (fixture, expected) in [(&mut a, "; first source"), (&mut b, "; second source")] {
        let config = (*fixture.editor.config()).clone();
        let language_loader = fixture.editor.load_language_config(&config)?;
        assert_eq!(
            language_loader
                .resources()
                .query("resource-test", "highlights.scm"),
            expected
        );
        fixture.editor.apply_language_config(
            language_loader,
            theme::Loader::new(loader::theme::Resources::new(vec![])).default_theme(),
        )?;
        let installed = fixture.editor.syn_loader.load();
        assert_eq!(
            installed
                .resources()
                .query("resource-test", "highlights.scm"),
            expected
        );
        // These isolated roots deliberately exclude the process-default native grammars.
        assert!(installed.resources().grammar("json")?.is_none());
    }
    Ok(())
}

#[derive(Default)]
struct ClipboardState {
    contents: [String; 2],
    writes: Vec<(
        view::clipboard::ClipboardProvider,
        view::clipboard::ClipboardType,
        String,
    )>,
    unreadable: bool,
    fail_writes: bool,
}

struct MemoryClipboard(Arc<std::sync::Mutex<ClipboardState>>);

impl view::clipboard::ClipboardBackend for MemoryClipboard {
    fn name(&self, provider: &view::clipboard::ClipboardProvider) -> String {
        format!("memory: {provider:?}")
    }

    fn get_contents(
        &self,
        _: &view::clipboard::ClipboardProvider,
        kind: view::clipboard::ClipboardType,
    ) -> view::clipboard::Result<String> {
        let state = self.0.lock().unwrap();
        if state.unreadable {
            Err(view::clipboard::ClipboardError::ReadingNotSupported)
        } else {
            Ok(
                state.contents[usize::from(kind == view::clipboard::ClipboardType::Selection)]
                    .clone(),
            )
        }
    }

    fn set_contents(
        &self,
        provider: &view::clipboard::ClipboardProvider,
        content: &str,
        kind: view::clipboard::ClipboardType,
    ) -> view::clipboard::Result<()> {
        let mut state = self.0.lock().unwrap();
        if state.fail_writes {
            return Err(view::clipboard::ClipboardError::IoError(
                std::io::ErrorKind::BrokenPipe.into(),
            ));
        }
        state.contents[usize::from(kind == view::clipboard::ClipboardType::Selection)] =
            content.into();
        state.writes.push((provider.clone(), kind, content.into()));
        Ok(())
    }
}

fn clipboard_values(editor: &Editor, register: char) -> Vec<String> {
    editor
        .registers
        .read(register, editor)
        .unwrap()
        .map(|value| value.into_owned())
        .collect()
}

#[tokio::test]
async fn clipboard_backends_are_editor_owned_and_follow_live_configuration() -> anyhow::Result<()> {
    use view::clipboard::{ClipboardProvider, ClipboardType};
    let mut first = Fixture::new("")?;
    let mut second = Fixture::new("")?;
    let a = Arc::new(std::sync::Mutex::new(ClipboardState::default()));
    let b = Arc::new(std::sync::Mutex::new(ClipboardState::default()));
    first
        .editor
        .registers
        .set_clipboard_backend(Box::new(MemoryClipboard(a.clone())));
    second
        .editor
        .registers
        .set_clipboard_backend(Box::new(MemoryClipboard(b.clone())));
    let mut settings = (**first.config.load()).clone();
    settings.clipboard_provider = ClipboardProvider::None;
    first.config.store(Arc::new(settings));
    first
        .editor
        .registers
        .write('+', vec!["one".into(), "two".into()])?;
    first.editor.registers.write('*', vec!["primary".into()])?;
    second
        .editor
        .registers
        .write('+', vec!["other editor".into()])?;
    assert_eq!(clipboard_values(&first.editor, '+'), ["one", "two"]);
    assert_eq!(clipboard_values(&first.editor, '*'), ["primary"]);
    assert_eq!(clipboard_values(&second.editor, '+'), ["other editor"]);
    assert_eq!(a.lock().unwrap().writes[0].0, ClipboardProvider::None);
    assert_eq!(a.lock().unwrap().writes[1].1, ClipboardType::Selection);

    let old = (*first.editor.config()).clone();
    let mut settings = old.clone();
    settings.clipboard_provider = ClipboardProvider::Termcode;
    first.config.store(Arc::new(settings));
    first.editor.refresh_config(&old);
    assert_eq!(
        first.editor.registers.clipboard_provider_name(),
        "memory: Termcode"
    );
    first.editor.registers.push('+', "zero".into())?;
    assert_eq!(clipboard_values(&first.editor, '+'), ["zero", "one", "two"]);
    assert_eq!(
        a.lock().unwrap().writes.last().unwrap().0,
        ClipboardProvider::Termcode
    );
    assert_eq!(clipboard_values(&second.editor, '+'), ["other editor"]);

    // Swapping frontend access preserves saved multi-selection values.
    first
        .editor
        .registers
        .set_clipboard_backend(Box::new(MemoryClipboard(Arc::new(std::sync::Mutex::new(
            ClipboardState {
                unreadable: true,
                ..Default::default()
            },
        )))));
    assert_eq!(clipboard_values(&first.editor, '+'), ["zero", "one", "two"]);
    Ok(())
}

#[tokio::test]
async fn clipboard_registers_preserve_fallback_errors_and_clear_behavior() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("")?;
    let state = Arc::new(std::sync::Mutex::new(ClipboardState {
        unreadable: true,
        ..Default::default()
    }));
    fixture
        .editor
        .registers
        .set_clipboard_backend(Box::new(MemoryClipboard(state.clone())));
    assert!(clipboard_values(&fixture.editor, '+').is_empty());
    fixture
        .editor
        .registers
        .write('+', vec!["α".into(), "β".into()])?;
    assert_eq!(clipboard_values(&fixture.editor, '+'), ["α", "β"]);
    assert!(fixture
        .editor
        .registers
        .push('+', "cannot read".into())
        .is_err());
    state.lock().unwrap().fail_writes = true;
    assert!(fixture
        .editor
        .registers
        .write('+', vec!["failed".into()])
        .is_err());
    assert_eq!(clipboard_values(&fixture.editor, '+'), ["α", "β"]);
    {
        let mut state = state.lock().unwrap();
        state.fail_writes = false;
        state.unreadable = false;
        state.contents[0] = "external clipboard".into();
    }
    assert_eq!(
        clipboard_values(&fixture.editor, '+'),
        ["external clipboard"]
    );
    assert!(fixture
        .editor
        .registers
        .push('+', "mismatch".into())
        .is_err());
    fixture
        .editor
        .registers
        .write('*', vec!["primary".into()])?;
    assert!(fixture.editor.registers.remove('*'));
    assert!(state.lock().unwrap().contents[1].is_empty());
    fixture.editor.registers.clear();
    assert!(state.lock().unwrap().contents.iter().all(String::is_empty));
    assert_eq!(clipboard_values(&fixture.editor, '+'), [""]);
    Ok(())
}
