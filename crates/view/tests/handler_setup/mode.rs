use super::Fixture;
use std::sync::{Arc, Mutex};
use view::{document::Mode, events::ModeDidChange};

#[tokio::test(flavor = "multi_thread")]
async fn mode_observers_see_each_completed_transition_once() -> anyhow::Result<()> {
    let mut f = Fixture::new("text\n")?;
    let observed = Arc::new(Mutex::new(Vec::new()));
    let record = observed.clone();
    event::register_hook!(move |event: &mut ModeDidChange<'_>| {
        assert_eq!(event.editor.mode(), event.new);
        assert!(event
            .editor
            .tree
            .views()
            .all(|(view, _)| view.diagnostics_handler.active == (event.new != Mode::Insert)));
        record.lock().unwrap().push((event.old, event.new));
        Ok(())
    });
    f.editor.set_mode(Mode::Insert);
    f.editor.set_mode(Mode::Insert);
    f.editor.set_mode(Mode::Select);
    f.editor.enter_debug_mode();
    f.editor.enter_debug_mode();
    f.editor.enter_normal_mode();
    assert_eq!(
        *observed.lock().unwrap(),
        vec![
            (Mode::Normal, Mode::Insert),
            (Mode::Insert, Mode::Select),
            (Mode::Select, Mode::Debug),
            (Mode::Debug, Mode::Normal)
        ]
    );
    Ok(())
}
