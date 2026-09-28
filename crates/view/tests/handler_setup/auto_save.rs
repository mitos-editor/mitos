use std::time::Duration;

use anyhow::Context as _;
use view::{callbacks::EditorCallback, current_ref, document::Mode, handlers::auto_save};

use super::{support::callback_channel, Fixture};

impl Fixture {
    fn auto_save() -> anyhow::Result<Self> {
        let mut fixture = Self::with_config(
            "before\n",
            "language = []",
            loader::syntax::Resources::default(),
            |config| {
                config.word_completion.enable = false;
                config.auto_save.after_delay.enable = true;
                config.auto_save.after_delay.timeout = 10;
            },
        )?;
        // Reopen after installing the dedicated handler so the document's trigger
        // uses this queue. Other editor callbacks must not masquerade as autosaves.
        let path = current_ref!(fixture.editor).1.path().unwrap().to_path_buf();
        fixture.close_current()?;
        let (sender, callbacks) = callback_channel();
        fixture.editor.handlers.auto_save = auto_save::AutoSaveHandler::new(sender);
        fixture.callbacks = callbacks;
        fixture.editor.open(&path, view::editor::Action::Replace)?;
        Ok(fixture)
    }

    async fn next(&mut self) -> anyhow::Result<EditorCallback> {
        tokio::time::timeout(Duration::from_secs(5), self.callbacks.recv())
            .await?
            .context("autosave callback queue closed")
    }

    async fn publish(&mut self) -> anyhow::Result<()> {
        self.next().await?(&mut self.editor);
        self.editor.flush_writes().await?;
        Ok(())
    }

    fn settings(&mut self, delayed: bool, focus: bool) {
        self.configure(|config| {
            config.auto_save.after_delay.enable = delayed;
            config.auto_save.focus_lost = focus;
        });
    }

    fn disk(&self) -> String {
        std::fs::read_to_string(current_ref!(self.editor).1.path().unwrap()).unwrap()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_and_callbacks_stay_with_their_own_editor() -> anyhow::Result<()> {
    let mut first = Fixture::auto_save()?;
    let mut second = Fixture::auto_save()?;
    assert_eq!(
        current_ref!(first.editor).1.id(),
        current_ref!(second.editor).1.id()
    );
    first.replace("first\n");
    first.publish().await?;
    assert_eq!(first.disk(), "first\n");
    assert_eq!(second.disk(), "before\n");
    assert!(second.callbacks.try_recv().is_err());

    first.replace("updated\n");
    let wrong_owner = first.next().await?;
    second.replace("second\n");
    wrong_owner(&mut second.editor);
    second.editor.flush_writes().await?;
    assert_eq!(second.disk(), "before\n");
    second.publish().await?;
    assert_eq!(second.disk(), "second\n");
    assert_eq!(first.disk(), "first\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_edit_invalidates_an_expired_but_queued_save() -> anyhow::Result<()> {
    let mut f = Fixture::auto_save()?;
    f.replace("first\n");
    let old = f.next().await?;
    f.replace("second\n");
    old(&mut f.editor);
    f.editor.flush_writes().await?;
    assert_eq!(f.disk(), "before\n");
    f.publish().await?;
    assert_eq!(f.disk(), "second\n");
    assert!(!current_ref!(f.editor).1.is_modified());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn disabling_autosave_invalidates_queued_and_deferred_saves() -> anyhow::Result<()> {
    let mut f = Fixture::auto_save()?;
    f.replace("queued\n");
    let old = f.next().await?;
    f.settings(false, false);
    f.settings(true, false);
    old(&mut f.editor);
    f.editor.flush_writes().await?;
    assert_eq!(f.disk(), "before\n");

    f.editor.mode = Mode::Insert;
    f.replace("deferred\n");
    f.publish().await?;
    f.editor.mode = Mode::Normal;
    f.editor.handlers.auto_save.left_insert_mode();
    let deferred = f.next().await?;
    f.settings(false, false);
    f.settings(true, false);
    deferred(&mut f.editor);
    f.editor.flush_writes().await?;
    assert_eq!(f.disk(), "before\n");
    f.replace("fresh\n");
    f.publish().await?;
    assert_eq!(f.disk(), "fresh\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn focus_loss_uses_its_own_setting_and_can_save_in_insert_mode() -> anyhow::Result<()> {
    let mut f = Fixture::auto_save()?;
    f.settings(false, false);
    f.editor.mode = Mode::Insert;
    f.replace("focus\n");
    auto_save::focus_lost(&mut f.editor);
    f.editor.flush_writes().await?;
    assert_eq!(f.disk(), "before\n");
    f.settings(false, true);
    auto_save::focus_lost(&mut f.editor);
    f.editor.flush_writes().await?;
    assert_eq!(f.disk(), "focus\n");
    assert!(f.callbacks.try_recv().is_err());
    Ok(())
}
