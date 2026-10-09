//! Real generated-binding guest used by normal runtime tests. Rebuild explicitly
//! after WIT/source changes; no guest compilation occurs inside a host test.
wit_bindgen::generate!({ path: "../../../../plugin-api/wit", world: "plugin" });
use mitos::plugin::{host, types};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Mutex,
};
static COUNT: AtomicU32 = AtomicU32::new(0);
static JOB: Mutex<Option<host::Job>> = Mutex::new(None);
struct Plugin;
fn effects(message: &str) -> Result<(), types::Failure> {
    let effects = host::begin_effects()?;
    effects.status(message)?;
    effects.finish()
}
impl Guest for Plugin {
    fn handle(request: types::Request) -> Result<(), types::Failure> {
        match request.command.as_deref().unwrap_or("status") {
            "config" => effects(&request.config_json),
            "status" => effects(&format!(
                "call {}",
                COUNT.fetch_add(1, Ordering::Relaxed) + 1
            )),
            "uppercase" => {
                let doc = request.editor.document.unwrap();
                let view = request.editor.view.unwrap();
                let range = &view.selections[view.primary as usize];
                let start = range.anchor.min(range.head);
                let end = range.anchor.max(range.head);
                let text = host::read_document(doc.id, doc.version, start, end)?.to_uppercase();
                let effects = host::begin_effects()?;
                let group = effects.edit(doc.id, doc.version)?;
                group.add(start, end, &text)?;
                group.finish()?;
                drop(group);
                let selection = effects.selection(
                    doc.id,
                    doc.version,
                    view.id,
                    view.binding_revision,
                    view.selection_revision,
                    0,
                )?;
                selection.add(start, start + text.chars().count() as u64)?;
                selection.finish()?;
                drop(selection);
                effects.finish()
            }
            "trap-after-finish" => {
                effects("discard me")?;
                panic!("trap after finishing effects");
            }
            "quota" => {
                let effects = host::begin_effects()?;
                for _ in 0..257 {
                    let _ = effects.status("exceed action quota");
                }
                let _ = effects.finish();
                Ok(())
            }
            "unfinished" => {
                let effects = host::begin_effects()?;
                effects.status("unfinished")?;
                Ok(())
            }
            "loop" => loop {
                std::hint::black_box(request.abi_version);
            },
            "grow" => {
                std::arch::wasm32::memory_grow::<0>(1024);
                Ok(())
            }
            "grow-32" => {
                std::arch::wasm32::memory_grow::<0>(512);
                Ok(())
            }
            "large-string" => effects(&"x".repeat(8 * 1024 * 1024)),
            "large-status" => effects(&"x".repeat(2 * 1024 * 1024)),
            "job-start" => {
                *JOB.lock().unwrap() = Some(host::start_timer(60_000)?);
                Ok(())
            }
            "job-poll" => {
                let result = JOB.lock().unwrap().as_ref().unwrap().poll()?;
                effects(&result)
            }
            "job-cancel" => {
                if let Some(job) = JOB.lock().unwrap().take() {
                    job.cancel()?;
                }
                Ok(())
            }
            "job-quota" => {
                let mut jobs = Vec::new();
                for _ in 0..17 {
                    jobs.push(host::start_timer(60_000)?);
                }
                Ok(())
            }
            "picker" | "picker-quota" => {
                let effects = host::begin_effects()?;
                let picker = effects.picker(17, None, "Choose a result")?;
                let count = if request.command.as_deref() == Some("picker-quota") {
                    1025
                } else {
                    2
                };
                for index in 0..count {
                    let row = picker.row(&index.to_string())?;
                    row.label("Result")?;
                    row.description("Details")?;
                    row.preview("Preview")?;
                    row.finish()?;
                }
                picker.finish()?;
                drop(picker);
                effects.finish()
            }
            "builtin" => {
                let view = request.editor.view.unwrap();
                let document = request.editor.document.unwrap();
                let effects = host::begin_effects()?;
                let group = effects.builtins(
                    18,
                    types::UiOrigin {
                        view: view.id,
                        document: document.id,
                        binding_revision: view.binding_revision,
                        version: document.version,
                        selection_revision: view.selection_revision,
                    },
                )?;
                group.add(types::BuiltinCommand::DeleteSelectionNoYank, Some(1))?;
                group.finish()?;
                drop(group);
                effects.finish()
            }
            "keymap" => {
                let effects = host::begin_effects()?;
                let group = effects.keymap(19)?;
                let binding = group.binding(types::KeymapMode::Normal, "uppercase")?;
                binding.key("space")?;
                binding.key("u")?;
                binding.finish()?;
                drop(binding);
                group.finish()?;
                drop(group);
                effects.finish()
            }
            _ => Err(types::Failure {
                code: types::ErrorCode::UnsupportedInterface,
                message: "unknown test command".into(),
            }),
        }
    }
}
export!(Plugin);
