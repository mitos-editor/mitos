use std::{collections::BTreeMap, path::Path};

use editor_core::{Range, Selection, Transaction};
use plugin_sdk::{Action, Event, Response, SelectionRange, TextEdit};
use plugins::PluginConfig;
use view::{
    current, current_ref, editor::Action as EditorAction, plugins::PluginConflict, Editor, ViewId,
};

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
        "abi-version = {}\nmodule = 'plugin.wasm'\nevents = {events:?}\n[commands.run]\ndoc = 'Run fixture'\n",
        plugin_sdk::ABI_VERSION
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
    let view_id = view.id.as_u64();
    let binding_revision = view.binding_revision();
    let selection_revision = doc.selection_revision(view.id).unwrap();
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
                    view: view_id,
                    binding_revision,
                    selection_revision,
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
    let (view, doc) = current_ref!(fixture.editor);
    let id = doc.id().as_u64();
    let version = doc.version();
    let view_id = view.id.as_u64();
    let binding_revision = view.binding_revision();
    let selection_revision = doc.selection_revision(view.id).unwrap();
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
                    view: view_id,
                    binding_revision,
                    selection_revision,
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
                view: view_id,
                binding_revision,
                selection_revision,
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

fn selection_action(editor: &Editor, view: ViewId, range: Range) -> Action {
    let target = editor.tree.get(view);
    let doc = editor.document(target.doc).unwrap();
    Action::SetSelection {
        document: doc.id().as_u64(),
        version: doc.version(),
        view: view.as_u64(),
        binding_revision: target.binding_revision(),
        selection_revision: doc.selection_revision(view).unwrap(),
        ranges: vec![SelectionRange {
            anchor: range.anchor,
            head: range.head,
        }],
        primary: 0,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn background_selection_hooks_keep_the_originating_split() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("abcdef\n")?;
    let (view, doc) = current_ref!(fixture.editor);
    let foreground = view.id;
    let id = doc.id();
    fixture.editor.switch(id, EditorAction::VerticalSplit);
    let background = current_ref!(fixture.editor).0.id;
    fixture.editor.focus(foreground);
    let before = fixture
        .editor
        .document(id)
        .unwrap()
        .selection(foreground)
        .clone();
    let mut action = selection_action(&fixture.editor, background, Range::new(4, 5));
    // This hook originates from the following selection change.
    if let Action::SetSelection {
        selection_revision, ..
    } = &mut action
    {
        *selection_revision += 1;
    }
    let dir = tempfile::tempdir()?;
    let config = plugin(
        dir.path(),
        Response {
            actions: vec![action],
            error: None,
        },
        &["selection-changed"],
    )?;
    assert!(fixture.editor.reload_plugins(&config, dir.path()));
    fixture
        .editor
        .document_mut(id)
        .unwrap()
        .set_selection(background, Selection::single(2, 3));
    drain(&mut fixture);
    let doc = fixture.editor.document(id).unwrap();
    assert_eq!(doc.selection(foreground), &before);
    assert_eq!(doc.selection(background).primary(), Range::new(4, 5));
    assert_eq!(fixture.editor.tree.focus, foreground);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn selection_only_changes_expire_queued_snapshots_without_text_edits() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("abcdef\n")?;
    let (view, doc) = current_ref!(fixture.editor);
    let view = view.id;
    let id = doc.id();
    let version = doc.version();
    let action = selection_action(&fixture.editor, view, Range::new(1, 2));
    let dir = tempfile::tempdir()?;
    let config = plugin(
        dir.path(),
        Response {
            actions: vec![action],
            error: None,
        },
        &["post-command"],
    )?;
    assert!(fixture.editor.reload_plugins(&config, dir.path()));
    fixture
        .editor
        .queue_plugin_event(Event::PostCommand, serde_json::Value::Null);
    fixture
        .editor
        .document_mut(id)
        .unwrap()
        .set_selection(view, Selection::single(5, 6));
    drain(&mut fixture);
    let doc = fixture.editor.document(id).unwrap();
    assert_eq!(doc.version(), version);
    assert_eq!(doc.selection(view).primary(), Range::new(5, 6));
    let error = fixture
        .editor
        .execute_plugin_command("fixture.run", vec![])
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<PluginConflict>(),
        Some(&PluginConflict::SelectionChanged(view.as_u64()))
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closed_and_rebound_views_are_typed_conflicts_before_any_edit() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("abcdef\n")?;
    let id = current_ref!(fixture.editor).1.id();
    fixture.editor.switch(id, EditorAction::VerticalSplit);
    let origin = current_ref!(fixture.editor).0.id;
    let selection = selection_action(&fixture.editor, origin, Range::new(4, 5));
    let edit = Action::Edit {
        document: id.as_u64(),
        version: fixture.editor.document(id).unwrap().version(),
        edits: vec![TextEdit {
            start: 0,
            end: 1,
            text: "Z".into(),
        }],
    };
    let dir = tempfile::tempdir()?;
    let config = plugin(
        dir.path(),
        Response {
            actions: vec![edit, selection],
            error: None,
        },
        &[],
    )?;
    assert!(fixture.editor.reload_plugins(&config, dir.path()));
    fixture.editor.new_file(EditorAction::Replace);
    fixture.editor.switch(id, EditorAction::Replace);
    let error = fixture
        .editor
        .execute_plugin_command("fixture.run", vec![])
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<PluginConflict>(),
        Some(&PluginConflict::ViewRebound(origin.as_u64()))
    );
    assert_eq!(
        fixture.editor.document(id).unwrap().text().to_string(),
        "abcdef\n"
    );
    fixture.editor.close(origin);
    fixture.editor.switch(id, EditorAction::VerticalSplit);
    assert_ne!(current_ref!(fixture.editor).0.id, origin);
    let error = fixture
        .editor
        .execute_plugin_command("fixture.run", vec![])
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<PluginConflict>(),
        Some(&PluginConflict::ViewClosed(origin.as_u64()))
    );
    assert_eq!(
        fixture.editor.document(id).unwrap().text().to_string(),
        "abcdef\n"
    );
    assert!(fixture.editor.close_document(id, true).is_ok());
    let error = fixture
        .editor
        .execute_plugin_command("fixture.run", vec![])
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<PluginConflict>(),
        Some(&PluginConflict::DocumentClosed(id.as_u64()))
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_document_edits_compose_and_undo_without_a_view() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("ßéx\n")?;
    let id = current_ref!(fixture.editor).1.id();
    let version = current_ref!(fixture.editor).1.version();
    fixture.editor.new_file(EditorAction::Replace);
    assert!(fixture.editor.tree.views().all(|(view, _)| view.doc != id));
    let unrelated = current_ref!(fixture.editor).1.text().clone();
    let unrelated_selection = current_ref!(fixture.editor)
        .1
        .selection(fixture.editor.tree.focus)
        .clone();
    let dir = tempfile::tempdir()?;
    let config = plugin(
        dir.path(),
        Response {
            actions: vec![
                Action::Edit {
                    document: id.as_u64(),
                    version,
                    edits: vec![TextEdit {
                        start: 0,
                        end: 2,
                        text: "SSÉ".into(),
                    }],
                },
                Action::Edit {
                    document: id.as_u64(),
                    version,
                    edits: vec![TextEdit {
                        start: 3,
                        end: 4,
                        text: "XX".into(),
                    }],
                },
            ],
            error: None,
        },
        &[],
    )?;
    assert!(fixture.editor.reload_plugins(&config, dir.path()));
    fixture
        .editor
        .execute_plugin_command("fixture.run", vec![])?;
    assert_eq!(
        fixture.editor.document(id).unwrap().text().to_string(),
        "SSÉXX\n"
    );
    assert_eq!(current_ref!(fixture.editor).1.text(), &unrelated);
    assert_eq!(
        current_ref!(fixture.editor)
            .1
            .selection(fixture.editor.tree.focus),
        &unrelated_selection
    );
    fixture.editor.switch(id, EditorAction::Replace);
    let (view, doc) = current!(fixture.editor);
    assert!(doc.undo(view));
    assert_eq!(doc.text().to_string(), "ßéx\n");
    assert!(!doc.undo(view));
    assert!(doc.redo(view));
    assert_eq!(doc.text().to_string(), "SSÉXX\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_edits_sync_prior_history_in_every_split() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("abcdef\n")?;
    let foreground = current_ref!(fixture.editor).0.id;
    let id = current_ref!(fixture.editor).1.id();
    fixture.editor.switch(id, EditorAction::VerticalSplit);
    let background = current_ref!(fixture.editor).0.id;
    {
        let (view, doc) = current!(fixture.editor);
        view.push_jump(doc, (id, Selection::single(4, 5)));
    }
    fixture.editor.focus(foreground);
    fixture.editor.new_file(EditorAction::VerticalSplit);
    {
        let view = fixture.editor.tree.get_mut(foreground);
        let doc = fixture.editor.documents.get_mut(&id).unwrap();
        let transaction = Transaction::change(doc.text(), [(0, 0, Some("YY".into()))].into_iter());
        assert!(doc.apply(&transaction, view.id));
        doc.append_changes_to_history(view);
    }
    let doc = fixture.editor.document(id).unwrap();
    let dir = tempfile::tempdir()?;
    let config = plugin(
        dir.path(),
        Response {
            actions: vec![Action::Edit {
                document: id.as_u64(),
                version: doc.version(),
                edits: vec![TextEdit {
                    start: 0,
                    end: 0,
                    text: "Z".into(),
                }],
            }],
            error: None,
        },
        &[],
    )?;
    assert!(fixture.editor.reload_plugins(&config, dir.path()));
    fixture
        .editor
        .execute_plugin_command("fixture.run", vec![])?;
    assert_eq!(
        fixture
            .editor
            .tree
            .get(background)
            .jumps
            .iter()
            .last()
            .unwrap()
            .1
            .primary(),
        Range::new(7, 8)
    );
    fixture.editor.focus(background);
    let (view, doc) = current!(fixture.editor);
    assert!(doc.undo(view));
    assert_eq!(doc.text().to_string(), "YYabcdef\n");
    assert_eq!(
        view.jumps.iter().last().unwrap().1.primary(),
        Range::new(6, 7)
    );
    assert!(doc.redo(view));
    assert_eq!(
        view.jumps.iter().last().unwrap().1.primary(),
        Range::new(7, 8)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn composed_edits_project_interleaved_multi_view_selections_and_one_undo(
) -> anyhow::Result<()> {
    let mut fixture = Fixture::new("abcdef\n")?;
    let origin = current_ref!(fixture.editor).0.id;
    let id = current_ref!(fixture.editor).1.id();
    let version = current_ref!(fixture.editor).1.version();
    fixture.editor.switch(id, EditorAction::VerticalSplit);
    let other = current_ref!(fixture.editor).0.id;
    fixture.editor.focus(origin);
    let original = fixture
        .editor
        .document(id)
        .unwrap()
        .selection(origin)
        .clone();
    let selections = vec![
        selection_action(&fixture.editor, origin, Range::new(2, 3)),
        selection_action(&fixture.editor, other, Range::new(4, 5)),
    ];
    let mut actions = vec![Action::Edit {
        document: id.as_u64(),
        version,
        edits: vec![TextEdit {
            start: 0,
            end: 1,
            text: "XX".into(),
        }],
    }];
    actions.extend(selections);
    actions.push(Action::Edit {
        document: id.as_u64(),
        version,
        edits: vec![TextEdit {
            start: 0,
            end: 0,
            text: "Y".into(),
        }],
    });
    let dir = tempfile::tempdir()?;
    let config = plugin(
        dir.path(),
        Response {
            actions,
            error: None,
        },
        &[],
    )?;
    assert!(fixture.editor.reload_plugins(&config, dir.path()));
    fixture
        .editor
        .execute_plugin_command("fixture.run", vec![])?;
    let doc = fixture.editor.document(id).unwrap();
    assert_eq!(doc.text().to_string(), "YXXbcdef\n");
    assert_eq!(doc.selection(origin).primary(), Range::new(3, 4));
    assert_eq!(doc.selection(other).primary(), Range::new(5, 6));
    let (view, doc) = current!(fixture.editor);
    assert!(doc.undo(view));
    assert_eq!(doc.text().to_string(), "abcdef\n");
    assert_eq!(doc.selection(origin), &original);
    assert!(!doc.undo(view));
    assert!(doc.redo(view));
    assert_eq!(doc.selection(origin).primary(), Range::new(3, 4));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn readonly_targets_reject_the_complete_batch() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("abcdef\n")?;
    let (view, doc) = current_ref!(fixture.editor);
    let id = doc.id();
    let version = doc.version();
    let action = selection_action(&fixture.editor, view.id, Range::new(2, 3));
    let dir = tempfile::tempdir()?;
    let config = plugin(
        dir.path(),
        Response {
            actions: vec![
                action,
                Action::Edit {
                    document: id.as_u64(),
                    version,
                    edits: vec![TextEdit {
                        start: 0,
                        end: 1,
                        text: "X".into(),
                    }],
                },
            ],
            error: None,
        },
        &[],
    )?;
    assert!(fixture.editor.reload_plugins(&config, dir.path()));
    fixture.editor.document_mut(id).unwrap().readonly = true;
    let before = current_ref!(fixture.editor)
        .1
        .selection(fixture.editor.tree.focus)
        .clone();
    let error = fixture
        .editor
        .execute_plugin_command("fixture.run", vec![])
        .unwrap_err();
    assert!(format!("{error:#}").contains("readonly"));
    assert_eq!(
        current_ref!(fixture.editor).1.text().to_string(),
        "abcdef\n"
    );
    assert_eq!(
        current_ref!(fixture.editor)
            .1
            .selection(fixture.editor.tree.focus),
        &before
    );
    Ok(())
}
