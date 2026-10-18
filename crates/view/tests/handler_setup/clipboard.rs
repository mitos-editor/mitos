use ::clipboard::{ClipboardBackend, ClipboardError, ClipboardProvider, ClipboardType};
use std::sync::Arc;
use view::Editor;

use super::Fixture;

#[derive(Default)]
struct ClipboardState {
    contents: [String; 2],
    writes: Vec<(ClipboardProvider, ClipboardType, String)>,
    unreadable: bool,
    fail_writes: bool,
}

struct MemoryClipboard(Arc<std::sync::Mutex<ClipboardState>>);

impl ClipboardBackend for MemoryClipboard {
    fn name(&self, provider: &ClipboardProvider) -> String {
        format!("memory: {provider:?}")
    }

    fn get_contents(
        &self,
        _: &ClipboardProvider,
        kind: ClipboardType,
    ) -> ::clipboard::Result<String> {
        let state = self.0.lock().unwrap();
        if state.unreadable {
            Err(ClipboardError::ReadingNotSupported)
        } else {
            Ok(state.contents[usize::from(kind == ClipboardType::Selection)].clone())
        }
    }

    fn set_contents(
        &self,
        provider: &ClipboardProvider,
        content: &str,
        kind: ClipboardType,
    ) -> ::clipboard::Result<()> {
        let mut state = self.0.lock().unwrap();
        if state.fail_writes {
            return Err(ClipboardError::IoError(
                std::io::ErrorKind::BrokenPipe.into(),
            ));
        }
        state.contents[usize::from(kind == ClipboardType::Selection)] = content.into();
        state.writes.push((provider.clone(), kind, content.into()));
        Ok(())
    }
}

fn clipboard_values(editor: &Editor, register: char) -> Vec<String> {
    editor
        .read_register(register)
        .unwrap()
        .map(|value| value.into_owned())
        .collect()
}

#[tokio::test]
async fn clipboard_backends_are_editor_owned_and_follow_live_configuration() -> anyhow::Result<()> {
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
    settings.clipboard.provider = ClipboardProvider::None;
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
    settings.clipboard.provider = ClipboardProvider::Termcode;
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

#[tokio::test]
async fn contextual_registers_follow_the_focused_selection_and_document() -> anyhow::Result<()> {
    use editor_core::{smallvec, Range, Selection};
    use view::{current, current_ref, editor::Action};

    let mut fixture = Fixture::new("one two\n")?;
    let (view, doc) = current!(fixture.editor);
    let (first, id) = (view.id, doc.id());
    doc.set_selection(
        first,
        Selection::new(smallvec![Range::new(0, 3), Range::new(4, 7)], 0),
    );
    fixture
        .editor
        .registers
        .write('a', vec!["saved one".into(), "saved two".into()])?;
    fixture.editor.switch(id, Action::VerticalSplit);
    let (view, doc) = current!(fixture.editor);
    doc.set_selection(view.id, Selection::single(4, 7));
    assert_eq!(clipboard_values(&fixture.editor, '.'), ["two"]);
    fixture.editor.focus(first);
    assert_eq!(clipboard_values(&fixture.editor, '#'), ["1", "2"]);
    assert_eq!(clipboard_values(&fixture.editor, '.'), ["one", "two"]);
    assert_eq!(
        fixture.editor.first_register_value('.').as_deref(),
        Some("one")
    );
    assert_eq!(
        fixture.editor.last_register_value('.').as_deref(),
        Some("two")
    );
    assert_eq!(
        fixture.editor.first_register_value('%').unwrap(),
        current_ref!(fixture.editor).1.display_name()
    );
    fixture.editor.new_file(Action::Replace);
    assert_eq!(
        fixture.editor.first_register_value('%').as_deref(),
        Some("[scratch]")
    );
    let saved: Vec<_> = fixture.editor.registers.read('a').unwrap().collect();
    assert_eq!(saved, ["saved one", "saved two"]);
    Ok(())
}

#[tokio::test]
async fn external_clipboard_suffixes_are_not_saved_selections() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("")?;
    let state = Arc::new(std::sync::Mutex::new(ClipboardState::default()));
    fixture
        .editor
        .registers
        .set_clipboard_backend(Box::new(MemoryClipboard(state.clone())));
    for (register, slot) in [('+', 0), ('*', 1)] {
        for values in [vec!["foo".into()], vec!["α".into(), "β".into()]] {
            fixture.editor.registers.write(register, values.clone())?;
            let external = {
                let mut state = state.lock().unwrap();
                state.contents[slot].push_str("bar");
                state.contents[slot].clone()
            };
            assert_eq!(clipboard_values(&fixture.editor, register), [external]);
            assert!(fixture
                .editor
                .registers
                .push(register, "new".into())
                .is_err());
            state.lock().unwrap().unreadable = true;
            assert_eq!(clipboard_values(&fixture.editor, register), values);
            state.lock().unwrap().unreadable = false;
        }
    }
    Ok(())
}

#[tokio::test]
async fn failed_clipboard_push_preserves_saved_selections() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("")?;
    let state = Arc::new(std::sync::Mutex::new(ClipboardState::default()));
    fixture
        .editor
        .registers
        .set_clipboard_backend(Box::new(MemoryClipboard(state.clone())));
    for (register, slot) in [('+', 0), ('*', 1)] {
        fixture
            .editor
            .registers
            .write(register, vec!["one".into(), "two".into()])?;
        let original = state.lock().unwrap().contents[slot].clone();
        state.lock().unwrap().fail_writes = true;
        assert!(fixture
            .editor
            .registers
            .push(register, "failed".into())
            .is_err());
        assert_eq!(state.lock().unwrap().contents[slot], original);
        assert_eq!(clipboard_values(&fixture.editor, register), ["one", "two"]);
        state.lock().unwrap().unreadable = true;
        assert_eq!(clipboard_values(&fixture.editor, register), ["one", "two"]);
        {
            let mut state = state.lock().unwrap();
            state.fail_writes = false;
            state.unreadable = false;
        }
        fixture.editor.registers.push(register, "zero".into())?;
        assert_eq!(
            clipboard_values(&fixture.editor, register),
            ["zero", "one", "two"]
        );
    }
    Ok(())
}
