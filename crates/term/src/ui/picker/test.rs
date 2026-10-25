use std::time::Duration;

use super::*;
use crate::{application::Application, config::Config as AppConfig, job::Jobs};

pub(crate) fn items<T: Send + Sync, D>(picker: &Picker<T, D>) -> impl Iterator<Item = &T> {
    let snapshot = picker.results.matcher.snapshot();
    snapshot
        .matched_items(0..snapshot.matched_item_count())
        .map(|item| item.data)
}

pub(crate) fn all_items<T: Send + Sync, D>(picker: &Picker<T, D>) -> impl Iterator<Item = &T> {
    let snapshot = picker.results.matcher.snapshot();
    (0..snapshot.item_count()).filter_map(|index| snapshot.get_item(index).map(|item| item.data))
}

pub(crate) fn application(poll: bool) -> anyhow::Result<Application> {
    let mut config = AppConfig::default();
    config.editor.lsp.enable = false;
    config.editor.files.watch.enable = false;
    config.editor.files.watch.watch_vcs = false;
    config.editor.files.reload.enable = false;
    config.editor.files.reload.poll.enable = poll;
    config.editor.files.reload.poll.interval = 100;
    Application::new(
        Default::default(),
        config,
        language::syntax::Loader::new(
            language_config::Configuration {
                language: vec![],
                language_server: Default::default(),
            },
            Default::default(),
        )?,
        workspace::trust::WorkspaceTrust::fully_trusted(loader::workspace_trust_dir()),
    )
}

pub(crate) fn draw<T: Send + Sync, D: Send + Sync>(
    picker: &mut Picker<T, D>,
    app: &mut Application,
) {
    frame(picker, app);
}

pub(crate) fn frame<T: Send + Sync, D: Send + Sync>(
    picker: &mut Picker<T, D>,
    app: &mut Application,
) -> Surface {
    let config = AppConfig::default();
    let updates = tokio::sync::mpsc::unbounded_channel().0;
    let mut jobs = Jobs::new();
    let mut cx = Context {
        config: crate::config::Context {
            current: &config,
            updates: &updates,
        },
        editor: &mut app.editor,
        jobs: &mut jobs,
        scroll: None,
        image_picker: None,
        is_cursor_owner: true,
    };
    let area = Rect::new(0, 0, 70, 20);
    let mut surface = Surface::empty(area);
    picker.render(area, &mut surface, &mut cx);
    surface
}

pub(crate) async fn until<T: Send + Sync, D: Send + Sync>(
    picker: &mut Picker<T, D>,
    app: &mut Application,
    ready: impl Fn(&Picker<T, D>) -> bool,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            draw(picker, app);
            if !picker.results.is_refreshing() && ready(picker) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    Ok(())
}

pub(crate) fn is_settled<T: Send + Sync, D: Send + Sync>(picker: &mut Picker<T, D>) -> bool {
    !picker.dynamic_query_pending
        && !picker.query_refresh
        && !picker.results.is_refreshing()
        && picker
            .file_updates
            .as_ref()
            .is_none_or(|updates| !updates.is_pending())
        && picker.results.matcher.active_injectors() == 0
        && !picker.results.matcher.tick(0).running
}
