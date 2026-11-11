use crate::{
    application::TestApplication,
    compositor::{self, Component, Context, Event},
    job::Jobs,
    ui::{picker::test::frame, Picker},
};
use std::path::Path;

pub(super) fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env_remove("GIT_DIR")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

pub(super) fn key(
    app: &mut TestApplication,
    compositor: &mut compositor::Compositor,
    jobs: &mut Jobs,
    key: &str,
) {
    let config = crate::config::Config::default();
    key_with_config(app, compositor, jobs, &config, key);
}

pub(super) fn key_with_config(
    app: &mut TestApplication,
    compositor: &mut compositor::Compositor,
    jobs: &mut Jobs,
    config: &crate::config::Config,
    key: &str,
) {
    let updates = tokio::sync::mpsc::unbounded_channel().0;
    let mut cx = Context {
        config: crate::config::Context {
            current: config,
            updates: &updates,
        },
        editor: &mut app.editor,
        interaction: &mut app.interaction,
        jobs,
        scroll: None,
        image_picker: None,
        is_cursor_owner: true,
    };
    assert!(compositor.handle_event(&Event::Key(key.parse().unwrap()), &mut cx));
}

pub(super) fn screen<T: Send + Sync, D: Send + Sync>(
    picker: &mut Picker<T, D>,
    app: &mut TestApplication,
) -> String {
    frame(picker, app)
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

pub(super) fn press(component: &mut impl Component, app: &mut TestApplication, key: &str) {
    let config = crate::config::Config::default();
    let updates = tokio::sync::mpsc::unbounded_channel().0;
    let mut jobs = Jobs::new();
    component.handle_event(
        &Event::Key(key.parse().unwrap()),
        &mut Context {
            config: crate::config::Context {
                current: &config,
                updates: &updates,
            },
            editor: &mut app.editor,
            interaction: &mut app.interaction,
            jobs: &mut jobs,
            scroll: None,
            image_picker: None,
            is_cursor_owner: true,
        },
    );
}
