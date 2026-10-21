//! A headless editor with explicit callbacks and a private document directory.
use std::sync::Arc;

use arc_swap::ArcSwap;
use editor_config::Config;
use editor_core::{Selection, Transaction};
use language::syntax;
use tokio::sync::mpsc;
use view::{
    callbacks::{EditorCallback, EditorCallbackSender},
    current, current_ref,
    editor::Action,
    graphics::Rect,
    handlers::Handlers,
    Editor,
};

pub(crate) struct Fixture {
    pub editor: Editor,
    pub config: Arc<ArcSwap<Config>>,
    pub callbacks: mpsc::UnboundedReceiver<EditorCallback>,
    pub dir: tempfile::TempDir,
}

impl Fixture {
    pub fn new(text: &str) -> anyhow::Result<Self> {
        Self::with_languages(text, "language = []")
    }

    pub fn with_languages(text: &str, languages: &str) -> anyhow::Result<Self> {
        Self::with_resources(text, languages, loader::syntax::Resources::default())
    }

    pub fn with_resources(
        text: &str,
        languages: &str,
        resources: loader::syntax::Resources,
    ) -> anyhow::Result<Self> {
        Self::with_config(text, languages, resources, |_| {})
    }

    pub fn with_config(
        text: &str,
        languages: &str,
        resources: loader::syntax::Resources,
        configure: impl FnOnce(&mut Config),
    ) -> anyhow::Result<Self> {
        Self::with_handler_setup(text, languages, resources, configure, |_, _| {})
    }

    pub fn with_handler_setup(
        text: &str,
        languages: &str,
        resources: loader::syntax::Resources,
        configure: impl FnOnce(&mut Config),
        setup: impl FnOnce(&mut Handlers, &Config),
    ) -> anyhow::Result<Self> {
        let mut config = Config::default();
        config.files.watch.enable = false;
        config.files.reload.enable = false;
        config.files.reload.poll.enable = false;
        config.lsp.enable = false;
        config.completion.auto_trigger = false;
        config.completion.word.enable = true;
        config.completion.path.enable = false;
        configure(&mut config);
        let config = Arc::new(ArcSwap::from_pointee(config));
        let (callbacks_tx, callbacks) = callback_channel();
        let mut handlers = Handlers::new(&config.load(), callbacks_tx);
        setup(&mut handlers, &config.load());
        let mut editor = Editor::new(
            Rect::new(0, 0, 80, 24),
            Arc::new(theme::Loader::new(loader::theme::Resources::new(vec![]))),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::new(
                toml::from_str(languages)?,
                resources,
            )?)),
            config.clone(),
            handlers,
            workspace::trust::WorkspaceTrust::fully_trusted(loader::workspace_trust_dir()),
        );
        editor.new_file(Action::VerticalSplit);
        let dir = tempfile::tempdir()?;
        let path = stdx::path::normalize(dir.path().canonicalize()?).join("document.words");
        std::fs::write(&path, text)?;
        editor.open(&path, Action::Replace)?;
        Ok(Self {
            editor,
            config,
            callbacks,
            dir,
        })
    }

    /// A real formatter for headless editing and save scenarios.
    pub fn with_formatter(text: &str, command: &str) -> anyhow::Result<Self> {
        Self::with_config(
            text,
            &format!(
                r#"
                [[language]]
                name = "words"
                scope = "text.words"
                roots = []
                file-types = ["words"]
                formatter = {{ command = "bash", args = ["-c", {command:?}] }}
            "#
            ),
            loader::syntax::Resources::default(),
            |config| config.format.on_save = true,
        )
    }

    pub fn configure(&mut self, configure: impl FnOnce(&mut Config)) {
        let old = self.config.load_full();
        let mut new = (*old).clone();
        configure(&mut new);
        self.config.store(Arc::new(new));
        self.editor.refresh_config(&old);
    }

    pub fn replace(&mut self, text: &str) {
        let (view, doc) = current!(self.editor);
        let transaction = Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some(text.into()))].into_iter(),
        )
        .with_selection(Selection::point(0));
        view::editing::apply(doc, &transaction, view.id).unwrap();
    }

    /// Replace the current buffer as one completed undo group.
    pub fn replace_and_commit(&mut self, text: &str) {
        let (view, doc) = current!(self.editor);
        doc.set_selection(view.id, Selection::single(0, doc.text().len_chars()));
        view::editing::replace_selections(doc, view.id, text).unwrap();
    }

    pub fn close_current(&mut self) -> anyhow::Result<()> {
        let doc = current_ref!(self.editor).1.id();
        self.editor.new_file(Action::Replace);
        assert!(self.editor.close_document(doc, true).is_ok());
        Ok(())
    }
}

/// Feature tests can capture one handler without mixing in other editor callbacks.
pub fn callback_channel() -> (
    EditorCallbackSender,
    mpsc::UnboundedReceiver<EditorCallback>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let blocking = tx.clone();
    let sender = EditorCallbackSender::new(
        event::Redraw::default(),
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
    (sender, rx)
}
