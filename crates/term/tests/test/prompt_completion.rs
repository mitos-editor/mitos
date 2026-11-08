//! Delivery of prompt completions computed on a background thread.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

use term::{
    compositor::{Component, Context},
    job::Jobs,
    ui::{
        completers::CompletionResult,
        prompt::{Completion, CompletionDirection},
        Prompt,
    },
};
use tui::text::Span;

use super::helpers::{keys, run_event_loop_until_idle, test_config, AppBuilder};

struct Fixture {
    app: term::application::TestApplication,
    config: term::config::Config,
    jobs: Jobs,
    updates: tokio::sync::mpsc::UnboundedSender<term::config::ConfigEvent>,
}

impl Fixture {
    fn new() -> anyhow::Result<Self> {
        let config = test_config();
        Ok(Self {
            app: AppBuilder::new().with_config(config.clone()).build()?,
            config,
            jobs: Jobs::new(),
            updates: tokio::sync::mpsc::unbounded_channel().0,
        })
    }

    fn context(&mut self) -> Context<'_> {
        Context {
            config: term::config::Context {
                current: &self.config,
                updates: &self.updates,
            },
            editor: &mut self.app.editor,
            interaction: &mut self.app.interaction,
            jobs: &mut self.jobs,
            scroll: None,
            image_picker: None,
            is_cursor_owner: false,
        }
    }
}

/// Holds a worker pending while the prompt handles other input.
#[derive(Clone, Default)]
struct Gate(Arc<AtomicBool>);

impl Gate {
    fn release(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn wait(&self) {
        // Bounded so that a test which never releases the gate can still tear its runtime
        // down instead of waiting on this blocking task forever.
        let deadline = Instant::now() + Duration::from_secs(30);
        while !self.0.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Completer computing `completion for <input>`, blocking on `gate` while doing so and
/// reporting each time it starts on the `started` channel.
fn deferred_completions(
    gate: Gate,
    started: mpsc::Sender<()>,
    running: Arc<AtomicUsize>,
) -> impl FnMut(&view::Editor, &str) -> CompletionResult {
    move |_editor, input| {
        let (gate, started, running) = (gate.clone(), started.clone(), running.clone());
        let input = input.to_owned();
        CompletionResult::Deferred(Box::new(move |_: &event::TaskHandle| -> Vec<Completion> {
            running.fetch_add(1, Ordering::SeqCst);
            let _ = started.send(());
            gate.wait();
            running.fetch_sub(1, Ordering::SeqCst);
            vec![(0.., Span::raw(format!("completion for {input}")))]
        }))
    }
}

/// The line as it would be after selecting the first completion: still the typed text while
/// nothing has been delivered, since selecting from an empty completion list does nothing.
fn selected(prompt: &mut Prompt) -> String {
    prompt.change_completion_selection(CompletionDirection::Forward);
    prompt.line().clone()
}

/// Drive the prompt until `expected` is selectable. A background task reports through a
/// redraw, so no input tells the test that it happened.
fn wait_for_completion(prompt: &mut Prompt, fixture: &mut Fixture, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        prompt.update(view::graphics::Rect::default(), &mut fixture.context());
        let selection = selected(prompt);
        if selection == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for a deferred completion, last selected {selection:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn prompt_applies_deferred_completions_without_further_input() -> anyhow::Result<()> {
    let mut fixture = Fixture::new()?;
    let gate = Gate::default();
    let (started, started_rx) = mpsc::channel();
    let mut prompt = Prompt::new(
        ":".into(),
        None,
        deferred_completions(gate.clone(), started, Arc::new(AtomicUsize::new(0))),
        |_, _, _| {},
    );

    prompt.set_line("first".into(), &fixture.app.editor);
    started_rx.recv_timeout(Duration::from_secs(10))?;
    // Nothing is selectable yet: the prompt handed this task over to the background.
    assert_eq!(selected(&mut prompt), "first");

    // The task reports on its own; the prompt has to collect it without new input.
    gate.release();
    wait_for_completion(&mut prompt, &mut fixture, "completion for first");

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn prompt_computes_one_deferred_completion_at_a_time() -> anyhow::Result<()> {
    let mut fixture = Fixture::new()?;
    let gate = Gate::default();
    let (started, started_rx) = mpsc::channel();
    let running = Arc::new(AtomicUsize::new(0));
    let mut prompt = Prompt::new(
        ":".into(),
        None,
        deferred_completions(gate.clone(), started, running.clone()),
        |_, _, _| {},
    );

    prompt.set_line("first".into(), &fixture.app.editor);
    started_rx.recv_timeout(Duration::from_secs(10))?;
    // Typing while that task runs neither starts another one nor settles for the completion
    // of the text typed before it.
    prompt.set_line("second".into(), &fixture.app.editor);
    prompt.set_line("third".into(), &fixture.app.editor);
    assert_eq!(running.load(Ordering::SeqCst), 1);
    assert_eq!(selected(&mut prompt), "third");

    // Each input names itself, so a request dropped on the way here would be the wrong
    // completion rather than a timeout.
    gate.release();
    wait_for_completion(&mut prompt, &mut fixture, "completion for third");
    assert_eq!(running.load(Ordering::SeqCst), 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn immediate_completion_is_not_blocked_by_obsolete_io() -> anyhow::Result<()> {
    let mut fixture = Fixture::new()?;
    let gate = Gate::default();
    let task_gate = gate.clone();
    let mut prompt = Prompt::new(
        ":".into(),
        None,
        move |_, input| {
            if input == "slow" {
                let gate = task_gate.clone();
                CompletionResult::Deferred(Box::new(move |_| {
                    gate.wait();
                    vec![(0.., Span::raw("obsolete result"))]
                }))
            } else {
                CompletionResult::Immediate(vec![(0.., Span::raw("immediate result"))])
            }
        },
        |_, _, _| {},
    );
    prompt.set_line("slow".into(), &fixture.app.editor);
    prompt.set_line("immediate".into(), &fixture.app.editor);
    let actual = selected(&mut prompt);
    let pending = prompt.has_pending_render();
    fixture.app.editor.redraw().start_frame();
    gate.release();
    assert!(!pending, "obsolete I/O must not prevent idle");
    assert_eq!(actual, "immediate result");
    tokio::time::timeout(
        Duration::from_secs(10),
        fixture.app.editor.redraw().requested(),
    )
    .await?;
    prompt.update(view::graphics::Rect::default(), &mut fixture.context());
    assert_eq!(selected(&mut prompt), "immediate result");
    Ok(())
}

/// Exercise the application event pump, including worker redraws and queued input callbacks.
#[tokio::test(flavor = "multi_thread")]
async fn macro_input_resumes_after_deferred_tab_completion() -> anyhow::Result<()> {
    use term::{compositor::Event, ui::PromptEvent};
    let mut app = AppBuilder::new().build()?;
    app.job_sender()
        .send(|compositor, cx| {
            let gate = Gate::default();
            let worker_gate = gate.clone();
            let prompt = Prompt::new(
                ":".into(),
                None,
                move |_, input| {
                    let input = input.to_owned();
                    let gate = worker_gate.clone();
                    CompletionResult::Deferred(Box::new(move |_| {
                        gate.wait();
                        vec![(0.., Span::raw(format!("completed-{input}")))]
                    }))
                },
                |cx, line, event| {
                    if event == PromptEvent::Validate {
                        cx.editor.registers.write('b', vec![line.into()]).unwrap();
                    }
                },
            );
            compositor.push(Box::new(prompt));
            // Replay dispatches all keys synchronously, before the next component update.
            cx.interaction.macro_replaying.push('a');
            cx.interaction.macro_recording = Some(('q', Vec::new()));
            for key in ui_core::input::parse_macro("abc<tab><ret>iX<esc>").unwrap() {
                compositor.handle_event(&Event::Key(key), cx);
            }
            cx.interaction.macro_replaying.pop();
            gate.release();
        })
        .await;
    tokio::time::timeout(Duration::from_secs(10), run_event_loop_until_idle(&mut app)).await?;
    assert_eq!(
        app.editor.read_register('b').unwrap().collect::<Vec<_>>(),
        ["completed-abc"]
    );
    assert_eq!(view::doc!(app.editor).text().to_string().trim(), "X");
    assert!(app
        .interaction
        .macro_recording
        .as_ref()
        .unwrap()
        .1
        .is_empty());
    assert!(app.interaction.macro_replaying.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn worker_redraw_delivers_completions_without_input() -> anyhow::Result<()> {
    use term::ui::PromptEvent;
    let mut app = AppBuilder::new().build()?;
    app.job_sender()
        .send(|compositor, cx| {
            let mut prompt = Prompt::new(
                ":".into(),
                None,
                |_, _| {
                    CompletionResult::Deferred(Box::new(|_| {
                        std::thread::sleep(Duration::from_millis(80));
                        vec![(0.., Span::raw("delivered"))]
                    }))
                },
                |cx, line, event| {
                    if event == PromptEvent::Validate {
                        cx.editor.registers.write('b', vec![line.into()]).unwrap();
                    }
                },
            );
            prompt.set_line("typed".into(), cx.editor);
            compositor.push(Box::new(prompt));
        })
        .await;
    tokio::time::timeout(Duration::from_secs(10), run_event_loop_until_idle(&mut app)).await?;
    keys(&mut app, "<tab><ret>").await?;
    assert_eq!(
        app.editor.read_register('b').unwrap().collect::<Vec<_>>(),
        ["delivered"]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_remains_available_while_tab_waits_for_io() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    let gate = Gate::default();
    let worker_gate = gate.clone();
    app.job_sender()
        .send(move |compositor, _| {
            let prompt = Prompt::new(
                ":".into(),
                None,
                move |_, _| {
                    let gate = worker_gate.clone();
                    CompletionResult::Deferred(Box::new(move |_| {
                        gate.wait();
                        Vec::new()
                    }))
                },
                |_, _, _| {},
            );
            compositor.push(Box::new(prompt));
        })
        .await;
    // Escape must close the prompt before the worker is released.
    let result = keys(&mut app, "a<tab><esc>iX<esc>").await;
    gate.release();
    result?;
    assert_eq!(view::doc!(app.editor).text().to_string().trim(), "X");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_prompt_cancels_its_worker() -> anyhow::Result<()> {
    let fixture = Fixture::new()?;
    let (finished, canceled) = mpsc::channel();
    let mut prompt = Prompt::new(
        ":".into(),
        None,
        move |_, _| {
            let finished = finished.clone();
            CompletionResult::Deferred(Box::new(move |handle| {
                let deadline = Instant::now() + Duration::from_secs(10);
                while !handle.is_canceled() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let _ = finished.send(handle.is_canceled());
                Vec::new()
            }))
        },
        |_, _, _| {},
    );
    prompt.set_line("typed".into(), &fixture.app.editor);
    drop(prompt);
    assert!(canceled.recv_timeout(Duration::from_secs(10))?);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn deferred_input_is_recorded_once_at_its_eventual_target() -> anyhow::Result<()> {
    use term::{compositor::Event, ui::PromptEvent};
    let mut app = AppBuilder::new().build()?;
    app.job_sender()
        .send(|compositor, cx| {
            let gate = Gate::default();
            let worker_gate = gate.clone();
            let prompt = Prompt::new(
                ":".into(),
                None,
                move |_, _| {
                    let gate = worker_gate.clone();
                    CompletionResult::Deferred(Box::new(move |_| {
                        gate.wait();
                        vec![(0.., Span::raw("completed"))]
                    }))
                },
                |cx, line, event| {
                    if event == PromptEvent::Validate {
                        cx.editor.registers.write('b', vec![line.into()]).unwrap();
                    }
                },
            );
            compositor.push(Box::new(prompt));
            cx.interaction.macro_recording = Some(('a', Vec::new()));
            for key in ui_core::input::parse_macro("abc<tab><ret>").unwrap() {
                compositor.handle_event(&Event::Key(key), cx);
            }
            gate.release();
        })
        .await;
    tokio::time::timeout(Duration::from_secs(10), run_event_loop_until_idle(&mut app)).await?;
    keys(&mut app, "iX<esc>Q").await?;
    let recorded = app.editor.registers.read('a').unwrap().next().unwrap();
    assert_eq!(
        ui_core::input::parse_macro(&recorded)?,
        ui_core::input::parse_macro("abc<tab><ret>iX<esc>")?
    );
    assert_eq!(
        app.editor.read_register('b').unwrap().collect::<Vec<_>>(),
        ["completed"]
    );
    assert_eq!(view::doc!(app.editor).text().to_string().trim(), "X");
    Ok(())
}
