use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context as _;
use editor_core::Transaction;
use term::application::Application;
use tokio::sync::mpsc;
use view::{
    callbacks::{EditorCallback, EditorCallbackSender},
    current, current_ref,
    document::Mode,
    editor::{Action, ConfigEvent},
    handlers::auto_save::{self, AutoSaveHandler},
};

use super::helpers::{test_config, AppBuilder};

struct Fixture {
    app: Application,
    path: PathBuf,
    callbacks: mpsc::UnboundedReceiver<EditorCallback>,
}

impl Fixture {
    fn new(dir: &Path) -> anyhow::Result<Self> {
        let path = dir.join("autosave.txt");
        std::fs::write(&path, "before\n")?;
        let mut config = test_config();
        config.editor.auto_save.after_delay.enable = true;
        config.editor.auto_save.after_delay.timeout = 10;
        let mut app = AppBuilder::new().with_config(config).build()?;
        let (tx, callbacks) = mpsc::unbounded_channel();
        let blocking = tx.clone();
        app.editor.handlers.auto_save = AutoSaveHandler::new(EditorCallbackSender::new(
            move |callback| {
                let tx = tx.clone();
                async move {
                    let _ = tx.send(callback);
                }
            },
            move |callback| {
                let _ = blocking.send(callback);
            },
        ));
        app.editor.open(&path, Action::Replace)?;
        Ok(Self {
            app,
            path,
            callbacks,
        })
    }

    fn edit(&mut self, text: &str) {
        let (view, doc) = current!(self.app.editor);
        let change = Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some(text.into()))].into_iter(),
        );
        assert!(doc.apply(&change, view.id));
    }

    async fn next(&mut self) -> anyhow::Result<EditorCallback> {
        tokio::time::timeout(Duration::from_secs(5), self.callbacks.recv())
            .await?
            .context("autosave callback queue closed")
    }

    async fn publish(&mut self) -> anyhow::Result<()> {
        self.next().await?(&mut self.app.editor);
        self.app.editor.flush_writes().await?;
        Ok(())
    }

    fn settings(&mut self, delayed: bool, focus: bool) {
        let mut config = (*self.app.editor.config()).clone();
        config.auto_save.after_delay.enable = delayed;
        config.auto_save.focus_lost = focus;
        self.app
            .handle_config_events(ConfigEvent::Update(Box::new(config)));
    }

    fn disk(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_and_callbacks_stay_with_their_own_editor() -> anyhow::Result<()> {
    let first_dir = tempfile::tempdir()?;
    let second_dir = tempfile::tempdir()?;
    let mut first = Fixture::new(first_dir.path())?;
    let mut second = Fixture::new(second_dir.path())?;
    assert_eq!(
        current_ref!(first.app.editor).1.id(),
        current_ref!(second.app.editor).1.id()
    );
    first.edit("first\n");
    first.publish().await?;
    assert_eq!(first.disk(), "first\n");
    assert_eq!(second.disk(), "before\n");
    assert!(second.callbacks.try_recv().is_err());

    first.edit("updated\n");
    let wrong_owner = first.next().await?;
    second.edit("second\n");
    wrong_owner(&mut second.app.editor);
    second.app.editor.flush_writes().await?;
    assert_eq!(second.disk(), "before\n");
    second.publish().await?;
    assert_eq!(second.disk(), "second\n");
    assert_eq!(first.disk(), "first\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_edit_invalidates_an_expired_but_queued_save() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path())?;
    f.edit("first\n");
    let old = f.next().await?;
    f.edit("second\n");
    old(&mut f.app.editor);
    f.app.editor.flush_writes().await?;
    assert_eq!(f.disk(), "before\n");
    f.publish().await?;
    assert_eq!(f.disk(), "second\n");
    assert!(!current_ref!(f.app.editor).1.is_modified());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_mode_defers_saves_until_the_terminal_forwards_exit() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path())?;
    f.app.editor.mode = Mode::Insert;
    f.edit("inserted\n");
    f.publish().await?;
    assert_eq!(f.disk(), "before\n");
    #[cfg(not(windows))]
    let escape = termina::event::Event::Key(ui_core::input::parse_macro("<esc>")?[0].into());
    #[cfg(windows)]
    let escape = crossterm::event::Event::Key(ui_core::input::parse_macro("<esc>")?[0].into());
    f.app.handle_terminal_events(Ok(escape)).await;
    assert_eq!(f.app.editor.mode(), Mode::Normal);
    f.publish().await?;
    assert_eq!(f.disk(), "inserted\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn disabling_autosave_invalidates_queued_and_deferred_saves() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path())?;
    f.edit("queued\n");
    let old = f.next().await?;
    f.settings(false, false);
    f.settings(true, false);
    old(&mut f.app.editor);
    f.app.editor.flush_writes().await?;
    assert_eq!(f.disk(), "before\n");

    f.app.editor.mode = Mode::Insert;
    f.edit("deferred\n");
    f.publish().await?;
    f.app.editor.mode = Mode::Normal;
    f.app.editor.handlers.auto_save.left_insert_mode();
    let deferred = f.next().await?;
    f.settings(false, false);
    f.settings(true, false);
    deferred(&mut f.app.editor);
    f.app.editor.flush_writes().await?;
    assert_eq!(f.disk(), "before\n");
    f.edit("fresh\n");
    f.publish().await?;
    assert_eq!(f.disk(), "fresh\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn replaced_handlers_and_closed_documents_reject_old_work() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path())?;
    f.edit("closed\n");
    let old = f.next().await?;
    let id = current_ref!(f.app.editor).1.id();
    assert!(f.app.editor.close_document(id, true).is_ok());
    old(&mut f.app.editor);
    f.app.editor.flush_writes().await?;
    assert_eq!(f.disk(), "before\n");

    f.app.editor.open(&f.path, Action::Replace)?;
    f.edit("replaced\n");
    let old = f.next().await?;
    f.app.editor.handlers.auto_save =
        AutoSaveHandler::new(EditorCallbackSender::new(|_| async {}, |_| {}));
    old(&mut f.app.editor);
    f.app.editor.flush_writes().await?;
    assert_eq!(f.disk(), "before\n");
    assert!(
        tokio::time::timeout(Duration::from_secs(5), f.callbacks.recv())
            .await?
            .is_none()
    );

    let fresh = Fixture::new(dir.path())?;
    let Fixture {
        app, mut callbacks, ..
    } = fresh;
    drop(app);
    assert!(
        tokio::time::timeout(Duration::from_secs(5), callbacks.recv())
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn focus_loss_uses_its_own_setting_and_can_save_in_insert_mode() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut f = Fixture::new(dir.path())?;
    f.settings(false, false);
    f.app.editor.mode = Mode::Insert;
    f.edit("focus\n");
    auto_save::focus_lost(&mut f.app.editor);
    f.app.editor.flush_writes().await?;
    assert_eq!(f.disk(), "before\n");
    f.settings(false, true);
    auto_save::focus_lost(&mut f.app.editor);
    f.app.editor.flush_writes().await?;
    assert_eq!(f.disk(), "focus\n");
    assert!(f.callbacks.try_recv().is_err());
    Ok(())
}
