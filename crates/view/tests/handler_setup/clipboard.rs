use std::sync::Arc;
use view::Editor;

use super::Fixture;

#[derive(Default)]
struct ClipboardState {
    contents: [String; 2],
    writes: Vec<(
        view::clipboard::ClipboardProvider,
        view::clipboard::ClipboardType,
        String,
    )>,
    unreadable: bool,
    fail_writes: bool,
}

struct MemoryClipboard(Arc<std::sync::Mutex<ClipboardState>>);

impl view::clipboard::ClipboardBackend for MemoryClipboard {
    fn name(&self, provider: &view::clipboard::ClipboardProvider) -> String {
        format!("memory: {provider:?}")
    }

    fn get_contents(
        &self,
        _: &view::clipboard::ClipboardProvider,
        kind: view::clipboard::ClipboardType,
    ) -> view::clipboard::Result<String> {
        let state = self.0.lock().unwrap();
        if state.unreadable {
            Err(view::clipboard::ClipboardError::ReadingNotSupported)
        } else {
            Ok(
                state.contents[usize::from(kind == view::clipboard::ClipboardType::Selection)]
                    .clone(),
            )
        }
    }

    fn set_contents(
        &self,
        provider: &view::clipboard::ClipboardProvider,
        content: &str,
        kind: view::clipboard::ClipboardType,
    ) -> view::clipboard::Result<()> {
        let mut state = self.0.lock().unwrap();
        if state.fail_writes {
            return Err(view::clipboard::ClipboardError::IoError(
                std::io::ErrorKind::BrokenPipe.into(),
            ));
        }
        state.contents[usize::from(kind == view::clipboard::ClipboardType::Selection)] =
            content.into();
        state.writes.push((provider.clone(), kind, content.into()));
        Ok(())
    }
}

fn clipboard_values(editor: &Editor, register: char) -> Vec<String> {
    editor
        .registers
        .read(register, editor)
        .unwrap()
        .map(|value| value.into_owned())
        .collect()
}

#[tokio::test]
async fn clipboard_backends_are_editor_owned_and_follow_live_configuration() -> anyhow::Result<()> {
    use view::clipboard::{ClipboardProvider, ClipboardType};
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
    settings.clipboard_provider = ClipboardProvider::None;
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
    settings.clipboard_provider = ClipboardProvider::Termcode;
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
