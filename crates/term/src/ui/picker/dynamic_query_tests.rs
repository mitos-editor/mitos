use super::*;
use crate::{
    application::Application,
    compositor::Compositor,
    config::Config,
    job::Jobs,
    ui::{overlay::overlaid, picker::Column},
};
use view::graphics::Rect;

fn application() -> anyhow::Result<Application> {
    let mut config = Config::default();
    config.editor.files.watch.enable = false;
    config.editor.lsp.enable = false;
    let syntax = language::Loader::new(
        language::config::Configuration {
            language: vec![],
            language_server: Default::default(),
        },
        Default::default(),
    )?;
    Application::new(
        Default::default(),
        config,
        syntax,
        workspace::trust::WorkspaceTrust::fully_trusted(loader::workspace_trust_dir()),
    )
}

fn picker(editor: &view::Editor) -> Picker<u8, ()> {
    Picker::new(
        editor.redraw().clone(),
        [Column::new("name", |_: &u8, _: &()| "entry".into())],
        0,
        [1],
        (),
        |_, _, _| {},
    )
}

fn query(query: &str) -> DynamicQueryChange {
    DynamicQueryChange {
        query: query.into(),
        is_paste: true,
    }
}

fn handler(picker: &Picker<u8, ()>, jobs: &Jobs) -> DynamicQueryHandler<u8, ()> {
    DynamicQueryHandler::new(
        |query, editor, _, _| {
            editor.set_status(format!("query: {query}"));
            Box::pin(async { Ok(()) })
        },
        None,
        jobs.sender(),
        picker.version.clone(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_queries_validate_picker_identity_and_generation() -> anyhow::Result<()> {
    for change in ["none", "replace", "supersede", "close"] {
        let mut app = application()?;
        let mut jobs = Jobs::new();
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let mut original = picker(&app.editor);
        original.prompt.set_line("old".into(), &app.editor);
        original.handle_prompt_change(false);
        let mut handler = handler(&original, &jobs);
        handler.handle_event(query("old"), None);
        compositor.push(Box::new(overlaid(original)));
        let callback = jobs.callbacks.recv().await.unwrap();
        match change {
            "replace" => {
                compositor.pop();
                compositor.push(Box::new(overlaid(picker(&app.editor))));
            }
            "supersede" => {
                handler.handle_event(query("new"), None);
            }
            "close" => {
                let original = &compositor
                    .find::<Overlay<Picker<u8, ()>>>()
                    .unwrap()
                    .content;
                original.version.fetch_add(1, atomic::Ordering::Relaxed);
            }
            _ => {}
        }
        jobs.handle_callback(&mut app.editor, &mut compositor, Ok(Some(callback)));
        assert_eq!(
            app.editor.get_status().map(|(message, _)| message.as_ref()),
            (change == "none").then_some("query: old"),
            "{change}"
        );
        assert!(app.close().await.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_latest_queued_query_starts_a_provider() -> anyhow::Result<()> {
    let mut app = application()?;
    let mut jobs = Jobs::new();
    let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
    let mut picker = picker(&app.editor);
    picker.prompt.set_line("new".into(), &app.editor);
    picker.handle_prompt_change(false);
    let mut handler = handler(&picker, &jobs);
    compositor.push(Box::new(overlaid(picker)));
    handler.handle_event(query("old"), None);
    handler.handle_event(query("new"), None);
    let old = jobs.callbacks.recv().await.unwrap();
    let new = jobs.callbacks.recv().await.unwrap();
    jobs.handle_callback(&mut app.editor, &mut compositor, Ok(Some(old)));
    assert!(app.editor.get_status().is_none());
    jobs.handle_callback(&mut app.editor, &mut compositor, Ok(Some(new)));
    assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "query: new");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queries_skipped_during_debounce_can_be_requested_again() -> anyhow::Result<()> {
    let mut app = application()?;
    let mut jobs = Jobs::new();
    let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
    let mut picker = picker(&app.editor);
    picker.prompt.set_line("new".into(), &app.editor);
    picker.handle_prompt_change(false);
    let mut handler = handler(&picker, &jobs);
    compositor.push(Box::new(overlaid(picker)));
    handler.handle_event(query("old"), None);
    let skipped = jobs.callbacks.recv().await.unwrap();
    jobs.handle_callback(&mut app.editor, &mut compositor, Ok(Some(skipped)));
    assert!(app.editor.get_status().is_none());
    let picker = &mut compositor
        .find::<Overlay<Picker<u8, ()>>>()
        .unwrap()
        .content;
    picker.prompt.set_line("old".into(), &app.editor);
    picker.handle_prompt_change(false);
    handler.handle_event(query("old"), None);
    let callback = jobs
        .callbacks
        .try_recv()
        .expect("revisited query is queued");
    jobs.handle_callback(&mut app.editor, &mut compositor, Ok(Some(callback)));
    assert_eq!(app.editor.get_status().unwrap().0.as_ref(), "query: old");
    assert!(app.close().await.is_empty());
    Ok(())
}
