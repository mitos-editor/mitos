use crate::compositor::DrawContext;
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::{FutureExt, StreamExt};
use language::syntax;
use view::{callbacks::EditorCallback, graphics::Rect};

use crate::application::TestApplication;
use crate::{
    args::Args,
    compositor::{Component, Context, Cursor},
    config::Config,
    ui::completers::CompletionResult,
};

#[derive(Clone, Default)]
struct FrameProbe {
    frames: Arc<Mutex<Vec<bool>>>,
    ready: Arc<tokio::sync::Notify>,
    request_on_first_frame: bool,
}

impl FrameProbe {
    fn frames(&self) -> Vec<bool> {
        self.frames.lock().unwrap().clone()
    }
}

impl Component for FrameProbe {
    fn render(&mut self, _area: Rect, _frame: &mut tui::buffer::Buffer, cx: &mut DrawContext) {
        self.frames
            .lock()
            .unwrap()
            .push(doc!(cx.editor).syntax().is_some());
        self.ready.notify_one();
        if self.request_on_first_frame {
            self.request_on_first_frame = false;
            cx.editor.redraw().request();
        }
    }
}

/// Drive the production pump until one complete frame, without a render-time pump.
async fn next_frame(app: &mut TestApplication, frames: &FrameProbe) {
    let count = frames.frames().len();
    let mut input = app.event_stream();
    let pump = app.event_loop(&mut input);
    tokio::pin!(pump);
    loop {
        tokio::select! {
            _ = &mut pump => return,
            _ = frames.ready.notified() => if frames.frames().len() > count { return; },
        }
    }
}

fn draw_now(app: &mut TestApplication) {
    let area = app.update_components();
    app.draw_frame(area);
}

/// Keep the existing pump since event_loop() requests a startup frame.
async fn background_frame(app: &mut TestApplication, frames: &FrameProbe) -> anyhow::Result<()> {
    let count = frames.frames().len();
    let mut input = app.event_stream();
    let pump = app.pump(&mut input, false);
    tokio::pin!(pump);
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            tokio::select! {
                _ = &mut pump => panic!("editor closed before background frame"),
                _ = frames.ready.notified() => if frames.frames().len() > count { break; },
            }
        }
    })
    .await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn picker_spinner_animates_without_input_and_stops_on_completion() -> anyhow::Result<()> {
    use crate::ui::{Picker, PickerColumn};

    let (mut app, _) = application(Args::default())?;
    let frames = FrameProbe::default();
    let picker = Picker::new(
        app.editor.redraw().clone(),
        [PickerColumn::new("item", |item: &String, _: &()| {
            item.as_str().into()
        })],
        0,
        [],
        (),
        |_, _, _| {},
    );
    let producer = picker.injector();
    app.compositor.push(Box::new(picker));
    app.compositor.push(Box::new(frames.clone()));

    let count = |app: &TestApplication| {
        let screen = app.terminal_backend().buffer();
        (screen.area.width - 5..screen.area.width - 2)
            .map(|x| screen[(x, 1)].symbol())
            .collect::<String>()
    };
    draw_now(&mut app);
    let first = count(&app);
    assert_eq!(first, "0/⣾");
    let deadline = app.render_state.animation_deadline.unwrap();
    while tokio::time::Instant::now() < deadline {
        background_frame(&mut app, &frames).await?;
    }
    let next = count(&app);
    assert!(next.starts_with("0/"));
    assert_ne!(next, first);

    producer.complete();
    drop(producer);
    draw_now(&mut app);
    assert_eq!(count(&app), "0/0");
    assert!(app.render_state.animation_deadline.is_none());
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn terminal_shrink_between_preparation_and_drawing_keeps_file_picker_open(
) -> anyhow::Result<()> {
    let (mut app, _) = application(Args::default())?;
    let dir = tempfile::tempdir()?;
    let picker = crate::ui::file_picker(&app.editor, dir.path().to_path_buf());
    app.compositor
        .push(Box::new(crate::ui::overlay::overlaid(picker)));
    draw_now(&mut app);

    let area = app.update_components();
    app.terminal.backend_mut().resize(60, 20);
    app.draw_frame(area);

    assert_eq!(
        app.terminal_backend().buffer().area,
        Rect::new(0, 0, 60, 20)
    );
    assert!(app.terminal_backend().cursor_visible());
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn user_activity_wakes_the_pump_without_input_and_stops_when_finished() -> anyhow::Result<()>
{
    let (mut app, _) = application(Args::default())?;
    app.drain_ready_callbacks();
    let frames = FrameProbe::default();
    app.compositor.push(Box::new(frames.clone()));
    draw_now(&mut app);
    tokio::time::pause();
    let document = view::current_ref!(app.editor).1.id();
    let activity = app.editor.start_activity("Formatting", Some(document));
    let start = tokio::time::Instant::now();
    assert_eq!(
        app.activity_frame_deadline(),
        Some(start + crate::ui::ACTIVITY_DELAY)
    );
    draw_now(&mut app);
    let mut last_frame = start;
    // Other ready editor events may draw before the activity's delayed frame.
    while tokio::time::Instant::now() < start + crate::ui::ACTIVITY_DELAY {
        background_frame(&mut app, &frames).await?;
        last_frame = tokio::time::Instant::now();
    }
    let text: String = app
        .terminal_backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("Formatting"), "{text:?}");
    background_frame(&mut app, &frames).await?;
    assert!(tokio::time::Instant::now() > last_frame);
    assert!(app.activity_frame_deadline().is_some());
    drop(activity);
    assert!(app.activity_frame_deadline().is_none());
    background_frame(&mut app, &frames).await?;
    let text: String = app
        .terminal_backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(!text.contains("Formatting"), "{text:?}");
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn user_activity_deadline_is_not_postponed_by_background_callbacks() -> anyhow::Result<()> {
    let (mut app, _) = application(Args::default())?;
    let _activity = app.editor.start_activity("Formatting", None);
    tokio::time::pause();
    tokio::time::advance(crate::ui::ACTIVITY_DELAY).await;
    draw_now(&mut app);
    let deadline = app.activity_frame_deadline().expect("animation deadline");
    tokio::time::advance(Duration::from_millis(40)).await;
    app.apply_callback(Ok(Some(crate::job::Callback::editor(|_| {
        view::callbacks::CallbackOutcome::None
    }))));
    assert_eq!(app.activity_frame_deadline(), Some(deadline));
    Ok(())
}

#[tokio::test]
async fn user_activity_deadlines_follow_visible_statuslines_and_configuration() -> anyhow::Result<()>
{
    let (mut app, _) = application(Args::default())?;
    let document = view::current_ref!(app.editor).1.id();
    let activity = app.editor.start_activity("Formatting", Some(document));
    app.editor.new_file(view::editor::Action::VerticalSplit);
    assert!(app.activity_frame_deadline().is_some());
    let mut config = (*app.editor.config()).clone();
    config.statusline.global = true;
    app.apply_config_update(crate::config::ConfigEvent::UpdateEditor(Box::new(config)));
    assert!(app.activity_frame_deadline().is_none());
    drop(activity);
    let workspace = app.editor.start_activity("Git", None);
    assert!(app.activity_frame_deadline().is_some());
    let mut config = (*app.editor.config()).clone();
    config
        .statusline
        .left
        .retain(|element| *element != editor_config::StatusLineElement::Activity);
    app.apply_config_update(crate::config::ConfigEvent::UpdateEditor(Box::new(config)));
    assert!(app.activity_frame_deadline().is_none());
    drop(workspace);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn user_activity_lives_through_job_delivery_and_clears_on_error_or_drop() -> anyhow::Result<()>
{
    use crate::job::{Callback, Job};
    let (mut app, _) = application(Args::default())?;
    let activity = app.editor.start_activity("Formatting", None);
    let callback = Job::with_callback(async {
        Ok(Callback::editor(|editor| {
            assert_eq!(editor.activities().count(), 1);
        }))
    })
    .with_activity(activity)
    .future
    .await;
    assert_eq!(app.editor.activities().count(), 1);
    app.apply_callback(callback);
    assert_eq!(app.editor.activities().count(), 0);

    let activity = app.editor.start_activity("Failing", None);
    let result = Job::with_callback(async { anyhow::bail!("test failure") })
        .with_activity(activity)
        .future
        .await;
    assert!(result.is_err());
    assert_eq!(app.editor.activities().count(), 0);

    let activity = app.editor.start_activity("Canceled", None);
    let job = Job::new(std::future::pending()).with_activity(activity);
    assert_eq!(app.editor.activities().count(), 1);
    drop(job);
    assert_eq!(app.editor.activities().count(), 0);
    assert!(app.close().await.is_empty());
    Ok(())
}

fn application(
    args: Args,
) -> anyhow::Result<(TestApplication, tokio::sync::mpsc::Receiver<EditorCallback>)> {
    let mut config = Config::default();
    config.editor.lsp.enable = false;
    config.editor.files.watch.enable = false;
    config.editor.files.reload.enable = false;
    config.editor.completion.word.enable = false;
    let lang_loader = syntax::Loader::new(
        toml::from_str(
            r#"
            [[language]]
            name = "json"
            scope = "source.json"
            file-types = ["json"]
            "#,
        )?,
        loader::syntax::Resources::default(),
    )?;
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let blocking_tx = tx.clone();
    let sender = view::callbacks::EditorCallbackSender::new(
        event::Redraw::default(),
        move |callback| {
            let tx = tx.clone();
            async move {
                let _ = tx.send(callback).await;
            }
        },
        move |callback| event::send_blocking(&blocking_tx, callback),
    );
    let app = TestApplication::new_with_handler_setup(
        args,
        config,
        lang_loader,
        workspace::trust::WorkspaceTrust::fully_trusted(loader::workspace_trust_dir()),
        event::EventHub::default(),
        move |handlers, _| {
            handlers.syntax = view::features::syntax::SyntaxHandler::new(sender);
        },
    )?;

    Ok((app, rx))
}

async fn syntax_completion(
    rx: &mut tokio::sync::mpsc::Receiver<EditorCallback>,
) -> anyhow::Result<EditorCallback> {
    Ok(tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await?
        .expect("syntax completion was not delivered"))
}

async fn pending_application(path: &Path) -> anyhow::Result<(TestApplication, EditorCallback)> {
    std::fs::write(path, "{}\n")?;
    let mut args = Args::default();
    args.files.insert(path.to_owned(), vec![Default::default()]);
    let (app, mut rx) = application(args)?;
    let callback = syntax_completion(&mut rx).await?;
    Ok((app, callback))
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "runtime-grammars"),
    ignore = "requires compiled runtime grammars; enable the runtime-grammars feature"
)]
async fn ready_syntax_is_published_before_first_frame() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (mut app, callback) = pending_application(&dir.path().join("startup.json")).await?;
    assert!(doc!(app.editor).syntax().is_none());
    app.jobs.editor_callback_sender().send(callback).await;

    let frames = FrameProbe::default();
    app.compositor.push(Box::new(frames.clone()));
    let mut input = app.event_stream();
    // Poll through the first render and stop when the event loop waits for work.
    assert!(app.event_loop(&mut input).now_or_never().is_none());
    assert_eq!(frames.frames().first(), Some(&true));
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "runtime-grammars"),
    ignore = "requires compiled runtime grammars; enable the runtime-grammars feature"
)]
async fn opening_a_file_publishes_ready_syntax_before_input_frame() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("opened.json");
    std::fs::write(&path, "{}\n")?;
    let (mut app, mut rx) = application(Args::default())?;
    draw_now(&mut app);
    app.editor.open(&path, view::editor::Action::Replace)?;
    let callback = syntax_completion(&mut rx).await?;
    assert!(doc!(app.editor).syntax().is_none());
    app.jobs.editor_callback_sender().send(callback).await;

    let frames = FrameProbe::default();
    app.compositor.push(Box::new(frames.clone()));
    let key = ui_core::input::parse_macro("l")?
        .into_iter()
        .next()
        .unwrap();
    #[cfg(not(windows))]
    let event = termina::event::Event::Key(tui::backend::native::adapt(key));
    #[cfg(windows)]
    let event = crossterm::event::Event::Key(tui::backend::native::adapt(key));
    app.handle_terminal_events(Ok(event)).await;
    next_frame(&mut app, &frames).await;
    assert_eq!(frames.frames().first(), Some(&true));
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    not(feature = "runtime-grammars"),
    ignore = "requires compiled runtime grammars; enable the runtime-grammars feature"
)]
async fn syntax_finishing_during_grace_is_in_the_first_frame() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (mut app, callback) = pending_application(&dir.path().join("startup.json")).await?;
    let frames = FrameProbe::default();
    app.compositor.push(Box::new(frames.clone()));
    let sender = app.jobs.editor_callback_sender();
    tokio::time::pause();
    let start = tokio::time::Instant::now();

    {
        let render = next_frame(&mut app, &frames);
        tokio::pin!(render);
        assert!(render.as_mut().now_or_never().is_none());
        assert!(frames.frames().is_empty());
        tokio::time::advance(Duration::from_millis(8)).await;
        sender.send(callback).await;
        render.await;
    }

    assert_eq!(start.elapsed(), Duration::from_millis(8));
    assert_eq!(frames.frames(), [true]);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn grace_expires_after_16_ms_and_is_not_repeated() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (mut app, callback) = pending_application(&dir.path().join("startup.json")).await?;
    let frames = FrameProbe::default();
    app.compositor.push(Box::new(frames.clone()));
    let sender = app.jobs.editor_callback_sender();
    tokio::time::pause();
    let start = tokio::time::Instant::now();

    {
        let render = next_frame(&mut app, &frames);
        tokio::pin!(render);
        assert!(render.as_mut().now_or_never().is_none());
        tokio::time::advance(Duration::from_millis(15)).await;
        // Unrelated results may be processed, but must not reset the deadline.
        sender
            .send(|editor| editor.set_status("Still loading syntax"))
            .await;
        assert!(render.as_mut().now_or_never().is_none());
        assert!(frames.frames().is_empty());
        tokio::time::advance(Duration::from_millis(1)).await;
        render.await;
    }

    // Tokio rounds timer deadlines up to its millisecond tick.
    let elapsed = start.elapsed();
    assert!((Duration::from_millis(16)..=Duration::from_millis(17)).contains(&elapsed));
    assert_eq!(frames.frames(), [false]);
    next_frame(&mut app, &frames).await;
    assert_eq!(frames.frames(), [false, false]);
    assert_eq!(start.elapsed(), elapsed);
    sender.send(callback).await;
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn background_syntax_does_not_delay_a_plain_text_frame() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (mut app, _callback) = pending_application(&dir.path().join("hidden.json")).await?;
    app.editor.new_file(view::editor::Action::Replace);
    let frames = FrameProbe::default();
    app.compositor.push(Box::new(frames.clone()));
    tokio::time::pause();
    let start = tokio::time::Instant::now();

    next_frame(&mut app, &frames).await;
    assert_eq!(start.elapsed(), Duration::ZERO);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn focus_cleanup_stays_with_its_application_and_unregisters_on_drop() -> anyhow::Result<()> {
    let (mut first, _) = application(Args::default())?;
    let (mut second, _) = application(Args::default())?;
    for app in [&mut first, &mut second] {
        app.compositor.push(Box::new(crate::ui::Prompt::new(
            ":".into(),
            None,
            |_, _| CompletionResult::Immediate(Vec::new()),
            |_, _, _| {},
        )));
    }
    let doc = doc!(first.editor).id();
    let view = first.editor.tree.focus();
    first
        .editor
        .events()
        .clone()
        .dispatch(view::events::DocumentFocusLost {
            editor: &mut first.editor,
            doc,
            view,
        })
        .report_to(first.editor.status_sender());
    let callback = first.jobs.callbacks.try_recv()?;
    first.jobs.handle_callback(
        crate::config::Context {
            current: &first.config.load(),
            updates: &first.config_updates.0,
        },
        &mut first.editor,
        &mut first.interaction,
        &mut first.compositor,
        Ok(Some(callback)),
    );
    assert!(first.compositor.find::<crate::ui::Prompt>().is_none());
    assert!(second.compositor.find::<crate::ui::Prompt>().is_some());
    assert!(second.jobs.callbacks.try_recv().is_err());

    // Dropping the application removes its hook and the strong queue handle it owns.
    let sender = first.job_sender();
    drop(first);
    sender.send(|_, _| {}).await;
    let doc = doc!(second.editor).id();
    let view = second.editor.tree.focus();
    second
        .editor
        .events()
        .clone()
        .dispatch(view::events::DocumentFocusLost {
            editor: &mut second.editor,
            doc,
            view,
        })
        .report_to(second.editor.status_sender());
    let callback = second.jobs.callbacks.try_recv()?;
    second.jobs.handle_callback(
        crate::config::Context {
            current: &second.config.load(),
            updates: &second.config_updates.0,
        },
        &mut second.editor,
        &mut second.interaction,
        &mut second.compositor,
        Ok(Some(callback)),
    );
    assert!(second.compositor.find::<crate::ui::Prompt>().is_none());
    assert!(second.jobs.callbacks.try_recv().is_err());
    Ok(())
}

#[tokio::test]
async fn background_job_failures_are_reported_to_the_application_that_started_them(
) -> anyhow::Result<()> {
    let (mut first, _) = application(Args::default())?;
    let (mut second, _) = application(Args::default())?;
    second
        .jobs
        .spawn(async { anyhow::bail!("second application's failure") });
    let callback = tokio::time::timeout(Duration::from_secs(1), second.jobs.callbacks.recv())
        .await?
        .expect("job failure was not delivered");
    second.jobs.handle_callback(
        crate::config::Context {
            current: &second.config.load(),
            updates: &second.config_updates.0,
        },
        &mut second.editor,
        &mut second.interaction,
        &mut second.compositor,
        Ok(Some(callback)),
    );
    assert_eq!(
        second
            .editor
            .get_status()
            .map(|(message, _)| message.as_ref()),
        Some("second application's failure")
    );
    assert!(first.jobs.callbacks.try_recv().is_err());
    assert!(first.editor.get_status().is_none());
    Ok(())
}

#[tokio::test]
async fn queued_focus_cleanup_preserves_prompts_created_after_the_focus_change(
) -> anyhow::Result<()> {
    let (mut app, _) = application(Args::default())?;
    let prompt = || {
        crate::ui::Prompt::new(
            ":".into(),
            None,
            |_, _| CompletionResult::Immediate(Vec::new()),
            |_, _, _| {},
        )
    };
    app.compositor.push(Box::new(prompt()));
    let doc = doc!(app.editor).id();
    let view = app.editor.tree.focus();
    app.editor
        .events()
        .clone()
        .dispatch(view::events::DocumentFocusLost {
            editor: &mut app.editor,
            doc,
            view,
        })
        .report_to(app.editor.status_sender());
    let callback = app.jobs.callbacks.try_recv()?;
    app.compositor.pop();
    app.compositor.push(Box::new(prompt()));
    let current = app
        .compositor
        .find::<crate::ui::Prompt>()
        .unwrap()
        .session()
        .clone();
    app.jobs.handle_callback(
        crate::config::Context {
            current: &app.config.load(),
            updates: &app.config_updates.0,
        },
        &mut app.editor,
        &mut app.interaction,
        &mut app.compositor,
        Ok(Some(callback)),
    );
    assert!(app
        .compositor
        .find::<crate::ui::Prompt>()
        .unwrap()
        .session()
        .same(&current));
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn compositor_resolves_native_blocks_and_prompt_cursors() -> anyhow::Result<()> {
    use editor_config::BlockCursorRendering;
    use view::graphics::CursorKind;

    fn render_cursor(app: &mut TestApplication) -> Cursor {
        let area = app.compositor.size();
        let mut buffer = tui::buffer::Buffer::empty(area);
        let config = app.config.load();
        let mut cx = Context {
            config: crate::config::Context {
                current: &config,
                updates: &app.config_updates.0,
            },
            editor: &mut app.editor,
            interaction: &mut app.interaction,
            jobs: &mut app.jobs,
            scroll: None,
            image_picker: app.image_picker.as_ref(),
            is_cursor_owner: false,
        };
        app.compositor.update(area, &mut cx);
        app.compositor
            .render(area, &mut buffer, &mut cx.draw_context())
    }

    let (mut app, _) = application(Args::default())?;
    let mut config = (*app.editor.config()).clone();
    config.welcome_screen = false;
    config.cursor.shape.insert = CursorKind::Bar;
    app.apply_config_update(crate::config::ConfigEvent::UpdateEditor(Box::new(
        config.clone(),
    )));
    let (view, doc) = view::current_ref!(app.editor);
    let inner = view.inner_area(doc);
    let position = (inner.x, inner.y).into();
    assert_eq!(render_cursor(&mut app), Cursor::Software(position));

    config.cursor.block_rendering = BlockCursorRendering::Native;
    app.apply_config_update(crate::config::ConfigEvent::UpdateEditor(Box::new(config)));
    let block = Cursor::Native(position, CursorKind::Block);
    assert_eq!(render_cursor(&mut app), block);

    #[cfg(not(windows))]
    let focus_events = [termina::Event::FocusOut, termina::Event::FocusIn];
    #[cfg(windows)]
    let focus_events = [
        crossterm::event::Event::FocusLost,
        crossterm::event::Event::FocusGained,
    ];
    for event in focus_events {
        app.handle_terminal_events(Ok(event)).await;
        assert_eq!(render_cursor(&mut app), block);
    }

    app.compositor.push(Box::new(crate::ui::Prompt::new(
        ":".into(),
        None,
        |_, _| CompletionResult::Immediate(Vec::new()),
        |_, _, _| {},
    )));
    assert!(matches!(
        render_cursor(&mut app),
        Cursor::Native(prompt_position, CursorKind::Bar) if prompt_position != position
    ));
    app.compositor.pop();
    assert_eq!(render_cursor(&mut app), block);

    // Each frame owns its cursor output, including a known-offscreen result.
    let (view, doc) = view::current!(app.editor);
    let transaction = editor_core::Transaction::insert(
        doc.text(),
        doc.selection(view.id),
        "界 line\n".repeat(300).into(),
    );
    view::editing::apply(doc, &transaction, view.id).unwrap();
    doc.set_selection(view.id, editor_core::Selection::point(0));
    doc.set_view_offset(view.id, Default::default());
    assert!(matches!(
        render_cursor(&mut app),
        Cursor::Native(_, CursorKind::Block)
    ));
    let (view, doc) = view::current!(app.editor);
    doc.set_selection(
        view.id,
        editor_core::Selection::point(doc.text().len_chars() - 1),
    );
    assert_eq!(render_cursor(&mut app), Cursor::Hidden);
    let (view, doc) = view::current!(app.editor);
    doc.set_selection(view.id, editor_core::Selection::point(0));
    assert!(matches!(
        render_cursor(&mut app),
        Cursor::Native(_, CursorKind::Block)
    ));
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn disabled_breadcrumb_edit_does_not_require_a_callback_frame() -> anyhow::Result<()> {
    let (mut app, _) = application(Args::default())?;
    app.drain_ready_callbacks();
    for key in ui_core::input::parse_macro("ix")? {
        #[cfg(not(windows))]
        let event = termina::event::Event::Key(tui::backend::native::adapt(key));
        #[cfg(windows)]
        let event = crossterm::event::Event::Key(tui::backend::native::adapt(key));
        app.handle_terminal_events(Ok(event)).await;
    }
    draw_now(&mut app);
    assert!(
        {
            app.drain_ready_callbacks();
            app.render_state.requested.is_none()
        },
        "disabled features must not require another frame"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_direct_frame_consumes_the_armed_redraw_deadline() -> anyhow::Result<()> {
    let (mut app, _) = application(Args::default())?;
    for _ in 0..16 {
        let Some(event) = app.editor.wait_event().now_or_never() else {
            break;
        };
        app.handle_editor_event(event).await;
    }
    app.editor.reset_idle_timer();
    tokio::time::pause();
    app.editor.redraw().request();
    let event = app
        .editor
        .wait_event()
        .now_or_never()
        .expect("ready redraw");
    app.handle_editor_event(event).await;
    assert!(app.frame_deadline().is_some());
    draw_now(&mut app);
    assert!(app.render_state.requested.is_none());
    tokio::time::advance(Duration::from_millis(34)).await;
    assert!(
        app.editor.wait_event().now_or_never().is_none(),
        "an already presented request must not draw again"
    );
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn work_callbacks_keep_fifo_order_and_legacy_callbacks_still_redraw() -> anyhow::Result<()> {
    let (mut app, _) = application(Args::default())?;
    app.drain_ready_callbacks();
    let order = Arc::new(Mutex::new(Vec::new()));
    let sender = app.jobs.editor_callback_sender();
    let first = order.clone();
    sender.send_work_blocking(move |_| first.lock().unwrap().push(1));
    let batch = app.drain_ready_callbacks();
    assert_eq!(batch, 1);
    assert!(app.render_state.requested.is_none());
    let second = order.clone();
    sender.send_blocking(move |_| second.lock().unwrap().push(2));
    let third = order.clone();
    sender.send_work_blocking(move |_| third.lock().unwrap().push(3));
    let batch = app.drain_ready_callbacks();
    assert_eq!(batch, 2);
    assert!(app.render_state.requested.is_some());
    assert_eq!(*order.lock().unwrap(), [1, 2, 3]);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn frame_callbacks_draw_immediately_and_redraws_wait() -> anyhow::Result<()> {
    let (mut app, _) = application(Args::default())?;
    app.drain_ready_callbacks();
    draw_now(&mut app);
    let sender = app.jobs.editor_callback_sender();
    sender.send_blocking(|_| view::callbacks::CallbackOutcome::Frame);
    app.drain_ready_callbacks();
    let now = tokio::time::Instant::now();
    assert!(app.render_state.requested.is_some_and(|due| due <= now));
    draw_now(&mut app);
    sender.send_blocking(|_| ());
    app.drain_ready_callbacks();
    let now = tokio::time::Instant::now();
    assert!(app.render_state.requested.is_some_and(|due| due > now));
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_new_request_after_a_direct_frame_still_draws() -> anyhow::Result<()> {
    let (mut app, _) = application(Args::default())?;
    for _ in 0..16 {
        let Some(event) = app.editor.wait_event().now_or_never() else {
            break;
        };
        app.handle_editor_event(event).await;
    }
    app.editor.reset_idle_timer();
    tokio::time::pause();
    draw_now(&mut app);
    app.editor.redraw().request();
    let event = app
        .editor
        .wait_event()
        .now_or_never()
        .expect("ready redraw");
    app.handle_editor_event(event).await;
    assert!(app.frame_deadline().unwrap() > tokio::time::Instant::now());
    tokio::time::advance(Duration::from_millis(34)).await;
    assert!(app.frame_deadline().unwrap() <= tokio::time::Instant::now());
    draw_now(&mut app);
    assert!(app.render_state.requested.is_none());
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn background_callback_bursts_share_the_next_input_frame() -> anyhow::Result<()> {
    let (mut app, _) = application(Args::default())?;
    let frames = FrameProbe::default();
    app.compositor.push(Box::new(frames.clone()));
    next_frame(&mut app, &frames).await;
    tokio::time::pause();
    let applied = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sender = app.jobs.editor_callback_sender();
    for _ in 0..32 {
        let applied = applied.clone();
        sender
            .send(move |_| {
                applied.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            })
            .await;
    }
    let (input, events) = tokio::sync::mpsc::unbounded_channel();
    let mut events = tokio_stream::wrappers::UnboundedReceiverStream::new(events);
    {
        let pump = app.pump(&mut events, false);
        tokio::pin!(pump);
        assert!(pump.as_mut().now_or_never().is_none());
        assert_eq!(
            frames.frames().len(),
            1,
            "background callbacks must coalesce"
        );
        tokio::time::advance(Duration::from_millis(10)).await;
        let key = ui_core::input::parse_macro("i")?.remove(0);
        input.send(Ok(crate::application::TerminalEvent::Key(
            tui::backend::native::adapt(key),
        )))?;
        assert!(pump.as_mut().now_or_never().is_none());
        assert_eq!(
            frames.frames().len(),
            2,
            "input must not wait for the 33ms background deadline"
        );
    }
    assert_eq!(applied.load(std::sync::atomic::Ordering::Relaxed), 32);
    assert_eq!(app.editor.mode(), view::document::Mode::Insert);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_request_raised_during_drawing_reaches_the_next_frame() -> anyhow::Result<()> {
    let (mut app, _) = application(Args::default())?;
    let frames = FrameProbe {
        request_on_first_frame: true,
        ..FrameProbe::default()
    };
    app.compositor.push(Box::new(frames.clone()));
    tokio::time::pause();
    let mut input = app.event_stream();
    {
        let pump = app.event_loop(&mut input);
        tokio::pin!(pump);
        assert!(pump.as_mut().now_or_never().is_none());
        assert_eq!(frames.frames().len(), 1);
        tokio::time::advance(Duration::from_millis(34)).await;
        assert!(pump.as_mut().now_or_never().is_none());
        assert_eq!(frames.frames().len(), 2);
    }
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_completion_between_preparation_and_drawing_keeps_its_redraw() -> anyhow::Result<()> {
    let (mut app, _) = application(Args::default())?;
    for _ in 0..16 {
        let Some(event) = app.editor.wait_event().now_or_never() else {
            break;
        };
        app.handle_editor_event(event).await;
    }
    app.editor.reset_idle_timer();
    tokio::time::pause();
    let area = app.update_components();
    // A preview worker can publish after update sampled its result receiver.
    app.editor.redraw().request();
    app.draw_frame(area);
    let event = app
        .editor
        .wait_event()
        .now_or_never()
        .expect("completion redraw");
    assert!(matches!(event, view::editor::EditorEvent::Redraw));
    app.handle_editor_event(event).await;
    assert!(app.frame_deadline().is_some());
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    not(feature = "runtime-grammars"),
    ignore = "requires compiled runtime grammars; enable the runtime-grammars feature"
)]
async fn syntax_grace_includes_newly_visible_unfocused_splits() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (mut app, callback) = pending_application(&dir.path().join("unfocused.json")).await?;
    let document = doc!(app.editor).id();
    app.editor.new_file(view::editor::Action::VerticalSplit);
    let frames = FrameProbe::default();
    app.compositor.push(Box::new(frames.clone()));
    let sender = app.jobs.editor_callback_sender();
    tokio::time::pause();
    let start = tokio::time::Instant::now();
    {
        let frame = next_frame(&mut app, &frames);
        tokio::pin!(frame);
        assert!(frame.as_mut().now_or_never().is_none());
        assert!(frames.frames().is_empty());
        tokio::time::advance(Duration::from_millis(8)).await;
        sender.send(callback).await;
        frame.await;
    }
    assert_eq!(start.elapsed(), Duration::from_millis(8));
    assert!(app.editor.document(document).unwrap().syntax().is_some());
    assert_eq!(frames.frames().len(), 1);
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn closing_during_syntax_grace_does_not_draw_a_closed_editor() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (mut app, _callback) = pending_application(&dir.path().join("closing.json")).await?;
    let frames = FrameProbe::default();
    app.compositor.push(Box::new(frames.clone()));
    let sender = app.jobs.editor_callback_sender();
    tokio::time::pause();
    {
        let frame = next_frame(&mut app, &frames);
        tokio::pin!(frame);
        assert!(frame.as_mut().now_or_never().is_none());
        sender
            .send(|editor| {
                let views: Vec<_> = editor.tree.views().map(|(view, _)| view.id).collect();
                for view in views {
                    editor.close(view);
                }
            })
            .await;
        frame.await;
    }
    assert!(frames.frames().is_empty());
    assert!(app.editor.should_close());
    assert!(app.close().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn replenished_callback_batches_yield_to_ready_input_after_drawing() -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn replenish(sender: crate::job::JobSender, applied: Arc<AtomicUsize>, rounds: usize) {
        sender
            .clone()
            .enqueue(crate::job::Callback::editor(move |_| {
                applied.fetch_add(1, Ordering::Relaxed);
                // Model a callback's elapsed work without a real sleep. advance moves
                // the paused clock before its first yield, which we need not retain.
                let _ = tokio::time::advance(Duration::from_millis(1)).now_or_never();
                if rounds > 1 {
                    replenish(sender, applied, rounds - 1);
                }
            }));
    }

    let (mut app, _) = application(Args::default())?;
    tokio::time::pause();
    let applied = Arc::new(AtomicUsize::new(0));
    let observed = Arc::new(AtomicUsize::new(usize::MAX));
    let input_seen = Arc::new(tokio::sync::Notify::new());
    // Every 64 callbacks exhaust the background deadline and replenish the queue.
    // A draw-to-drain loop reaches all 4096 callbacks before polling ready input.
    for _ in 0..64 {
        replenish(app.jobs.sender(), applied.clone(), 64);
    }
    let key = ui_core::input::parse_macro("i")?.remove(0);
    let mut input = futures_util::stream::iter([Ok(crate::application::TerminalEvent::Key(
        tui::backend::native::adapt(key),
    ))])
    .inspect(|_| {
        observed.store(applied.load(Ordering::Relaxed), Ordering::Relaxed);
        input_seen.notify_one();
    });
    app.request_frame();
    {
        let pump = app.pump(&mut input, false);
        tokio::pin!(pump);
        tokio::select! {
            _ = &mut pump => panic!("application closed before input"),
            _ = input_seen.notified() => {},
        }
    }
    assert!(
        observed.load(Ordering::Relaxed) < 4096,
        "drawing starved ready input"
    );
    assert_eq!(app.editor.mode(), view::document::Mode::Insert);
    assert!(app.close().await.is_empty());
    Ok(())
}
