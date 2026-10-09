use std::{collections::BTreeMap, path::Path};

use editor_core::{Range, Selection};
use plugin_sdk::{Action, Response, SelectionRange, TextEdit};
use plugins::PluginConfig;
use view::{current, current_ref};

use super::Fixture;

fn plugin(
    dir: &Path,
    response: Response,
    events: &[&str],
) -> anyhow::Result<BTreeMap<String, PluginConfig>> {
    plugin_with_init(dir, response, events, Response::default())
}

fn plugin_with_init(
    dir: &Path,
    response: Response,
    events: &[&str],
    initial: Response,
) -> anyhow::Result<BTreeMap<String, PluginConfig>> {
    std::fs::write(dir.join("plugin.toml"), format!(
        "abi-version = 1\nmodule = 'plugin.wasm'\nevents = {events:?}\n[commands.run]\ndoc = 'Run fixture'\n"
    ))?;
    let response = serde_json::to_vec(&response)?;
    let initial = serde_json::to_vec(&initial)?;
    let initial_data = initial
        .iter()
        .map(|byte| format!("\\{byte:02x}"))
        .collect::<String>();
    let data = response
        .iter()
        .map(|byte| format!("\\{byte:02x}"))
        .collect::<String>();
    let wasm = wat::parse_str(format!(
        r#"(module
        (memory (export "memory") 1)
        (global $calls (mut i32) (i32.const 0))
        (data (i32.const 16) "{initial_data}")
        (data (i32.const 8192) "{data}")
        (func (export "mitos_alloc") (param i32) (result i32) i32.const 32768)
        (func (export "mitos_dealloc") (param i32 i32))
        (func (export "mitos_call") (param i32 i32) (result i64)
            global.get $calls i32.const 1 i32.add global.set $calls
            global.get $calls i32.const 1 i32.eq
            if (result i64)
                i64.const {}
            else
                i64.const {}
            end)
    )"#,
        (16_u64 << 32) | initial.len() as u64,
        (8192_u64 << 32) | response.len() as u64
    ))?;
    std::fs::write(dir.join("plugin.wasm"), wasm)?;
    Ok(BTreeMap::from([(
        "fixture".into(),
        PluginConfig {
            path: dir.join("plugin.toml"),
            enabled: true,
            config: serde_json::Value::Null,
        },
    )]))
}

fn drain(fixture: &mut Fixture) {
    for _ in 0..100 {
        match fixture.callbacks.try_recv() {
            Ok(callback) => callback(&mut fixture.editor),
            Err(_) => return,
        }
    }
    panic!("plugin callbacks did not terminate");
}

#[tokio::test(flavor = "multi_thread")]
async fn lifecycle_opens_without_a_view_and_runs_for_large_documents() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let view = current_ref!(fixture.editor).0.id;
    fixture.editor.close(view);
    assert!(fixture.editor.tree.views().next().is_none());
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("startup.txt");
    std::fs::write(&target, "opened\n")?;
    let config = plugin_with_init(
        dir.path(),
        Response {
            actions: vec![Action::Status {
                message: "shutdown ran".into(),
            }],
            error: None,
        },
        &[],
        Response {
            actions: vec![Action::Open {
                path: target.to_string_lossy().into_owned(),
            }],
            error: None,
        },
    )?;
    assert!(fixture.editor.reload_plugins(&config, dir.path()));
    assert_eq!(
        current_ref!(fixture.editor).1.text().to_string(),
        "opened\n"
    );
    fixture.replace(&"x".repeat(2 * 1024 * 1024 + 1));
    fixture.editor.shutdown_plugins();
    assert_eq!(
        fixture.editor.status_msg.as_ref().unwrap().0,
        "shutdown ran"
    );
    let config = plugin_with_init(
        dir.path(),
        Response::default(),
        &[],
        Response {
            actions: vec![Action::Status {
                message: "large document init ran".into(),
            }],
            error: None,
        },
    )?;
    assert!(fixture.editor.reload_plugins(&config, dir.path()));
    assert_eq!(
        fixture.editor.status_msg.as_ref().unwrap().0,
        "large document init ran"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_use_character_offsets_and_are_one_undo_step() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("ßéx\n")?;
    let (view, doc) = current_ref!(fixture.editor);
    let id = doc.id().as_u64();
    let version = doc.version();
    let original = doc.selection(view.id).clone();
    let dir = tempfile::tempdir()?;
    let config = plugin(
        dir.path(),
        Response {
            actions: vec![
                Action::Edit {
                    document: id,
                    version,
                    edits: vec![TextEdit {
                        start: 0,
                        end: 2,
                        text: "SSÉ".into(),
                    }],
                },
                Action::SetSelection {
                    document: id,
                    version,
                    ranges: vec![SelectionRange { anchor: 0, head: 3 }],
                    primary: 0,
                },
            ],
            error: None,
        },
        &[],
    )?;
    fixture.editor.reload_plugins(&config, dir.path());
    assert!(fixture
        .editor
        .execute_plugin_command("fixture.run", vec![])?);
    let (view, doc) = current!(fixture.editor);
    assert_eq!(doc.text().to_string(), "SSÉx\n");
    assert_eq!(doc.selection(view.id).primary(), Range::new(0, 3));
    assert!(doc.undo(view));
    assert_eq!(doc.text().to_string(), "ßéx\n");
    assert_eq!(doc.selection(view.id), &original);
    assert!(doc.redo(view));
    assert_eq!(doc.text().to_string(), "SSÉx\n");
    assert_eq!(doc.selection(view.id).primary(), Range::new(0, 3));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_batches_and_stale_selections_leave_documents_unchanged() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let (_, doc) = current_ref!(fixture.editor);
    let id = doc.id().as_u64();
    let version = doc.version();
    let dir = tempfile::tempdir()?;
    let config = plugin(
        dir.path(),
        Response {
            actions: vec![
                Action::Edit {
                    document: id,
                    version,
                    edits: vec![TextEdit {
                        start: 0,
                        end: 8,
                        text: "changed".into(),
                    }],
                },
                Action::SetSelection {
                    document: id,
                    version,
                    ranges: vec![],
                    primary: 0,
                },
            ],
            error: None,
        },
        &[],
    )?;
    fixture.editor.reload_plugins(&config, dir.path());
    let error = fixture
        .editor
        .execute_plugin_command("fixture.run", vec![])
        .unwrap_err();
    assert!(format!("{error:#}").contains("invalid primary selection"));
    assert_eq!(
        current_ref!(fixture.editor).1.text().to_string(),
        "original\n"
    );

    let config = plugin(
        dir.path(),
        Response {
            actions: vec![Action::SetSelection {
                document: id,
                version: version - 1,
                ranges: vec![SelectionRange { anchor: 1, head: 2 }],
                primary: 0,
            }],
            error: None,
        },
        &[],
    )?;
    fixture.editor.reload_plugins(&config, dir.path());
    let before = current_ref!(fixture.editor)
        .1
        .selection(current_ref!(fixture.editor).0.id)
        .clone();
    let error = fixture
        .editor
        .execute_plugin_command("fixture.run", vec![])
        .unwrap_err();
    assert!(format!("{error:#}").contains("stale document version"));
    assert_eq!(
        current_ref!(fixture.editor)
            .1
            .selection(current_ref!(fixture.editor).0.id),
        &before
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn reload_discards_queued_events_and_edit_hooks_do_not_recurse() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let dir = tempfile::tempdir()?;
    let config = plugin(
        dir.path(),
        Response {
            actions: vec![Action::Status {
                message: "old plugin".into(),
            }],
            error: None,
        },
        &["document-changed"],
    )?;
    fixture.editor.reload_plugins(&config, dir.path());
    fixture.replace("before\n");
    let (_, doc) = current_ref!(fixture.editor);
    let config = plugin(
        dir.path(),
        Response {
            actions: vec![Action::Edit {
                document: doc.id().as_u64(),
                version: doc.version() + 1,
                edits: vec![TextEdit {
                    start: 0,
                    end: 6,
                    text: "AFTER!".into(),
                }],
            }],
            error: None,
        },
        &["document-changed"],
    )?;
    fixture.editor.reload_plugins(&config, dir.path());
    fixture.editor.set_status("reloaded");
    drain(&mut fixture);
    assert_eq!(fixture.editor.status_msg.as_ref().unwrap().0, "reloaded");
    assert_eq!(
        current_ref!(fixture.editor).1.text().to_string(),
        "before\n"
    );
    fixture.replace("before\n");
    drain(&mut fixture);
    assert_eq!(
        current_ref!(fixture.editor).1.text().to_string(),
        "AFTER!\n"
    );
    assert_eq!(fixture.editor.status_msg.as_ref().unwrap().0, "reloaded");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn open_cannot_invalidate_prepared_document_actions() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let (_, doc) = current_ref!(fixture.editor);
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("opened.txt");
    std::fs::write(&target, "opened\n")?;
    let config = plugin(
        dir.path(),
        Response {
            actions: vec![
                Action::Open {
                    path: target.to_string_lossy().into_owned(),
                },
                Action::Edit {
                    document: doc.id().as_u64(),
                    version: doc.version(),
                    edits: vec![TextEdit {
                        start: 0,
                        end: 1,
                        text: "x".into(),
                    }],
                },
            ],
            error: None,
        },
        &[],
    )?;
    fixture.editor.reload_plugins(&config, dir.path());
    let error = fixture
        .editor
        .execute_plugin_command("fixture.run", vec![])
        .unwrap_err();
    assert!(format!("{error:#}").contains("edit actions must precede open"));
    assert_eq!(
        current_ref!(fixture.editor).1.text().to_string(),
        "original\n"
    );
    Ok(())
}

// Building a Rust WASM target is optional for contributors. Run this explicitly
// after the documented example build to verify the complete guest-to-editor ABI.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the compiled examples/plugins/uppercase WASM module"]
async fn rust_sdk_plugin_runs_through_the_editor() -> anyhow::Result<()> {
    let module = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../examples/plugins/uppercase/target/wasm32-unknown-unknown/release/uppercase.wasm",
    );
    let dir = tempfile::tempdir()?;
    std::fs::copy(module, dir.path().join("uppercase.wasm"))?;
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/uppercase/plugin.toml"),
        dir.path().join("plugin.toml"),
    )?;
    let mut fixture = Fixture::new("ßé hello\n")?;
    let (view, doc) = current!(fixture.editor);
    doc.set_selection(
        view.id,
        Selection::new(vec![Range::new(0, 2), Range::new(3, 8)].into(), 1),
    );
    let config = BTreeMap::from([(
        "uppercase".into(),
        PluginConfig {
            path: dir.path().join("plugin.toml"),
            enabled: true,
            config: serde_json::Value::Null,
        },
    )]);
    fixture.editor.reload_plugins(&config, dir.path());
    assert!(fixture
        .editor
        .execute_plugin_command("uppercase.uppercase", vec![])?);
    let (view, doc) = current_ref!(fixture.editor);
    assert_eq!(doc.text().to_string(), "SSÉ HELLO\n");
    assert_eq!(doc.selection(view.id).primary_index(), 1);
    assert_eq!(
        doc.selection(view.id).iter().copied().collect::<Vec<_>>(),
        vec![Range::new(0, 3), Range::new(4, 9)]
    );
    Ok(())
}
