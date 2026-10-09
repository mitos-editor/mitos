use std::{collections::BTreeMap, path::Path};

use editor_core::{Range, Selection, Transaction};
use plugin_api::{Action, Event, Response, SelectionRange, TextEdit};
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
        "abi-version = {}\nmodule = 'plugin.wasm'\ncapabilities = ['ui', 'editor-read', 'editor-edit', 'editor-selection', 'editor-navigate', 'workspace-read']\nevents = {events:?}\n[commands.run]\ndoc = 'Run fixture'\n",
        plugin_api::ABI_VERSION
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
            permissions: crate::support::plugin_guest::permissions(dir),
            ..PluginConfig::default()
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
            permissions: crate::support::plugin_guest::permissions(dir.path()),
            ..PluginConfig::default()
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

#[tokio::test(flavor = "multi_thread")]
async fn flushed_saves_describe_written_text_and_precede_shutdown() -> anyhow::Result<()> {
    use super::support::plugin_guest::{observing, status, Route};
    let mut fixture = Fixture::new("original\n")?;
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("saved-as.txt");
    let id = current_ref!(fixture.editor).1.id();
    fixture.replace("submitted\n");
    let (view, doc) = current!(fixture.editor);
    doc.append_changes_to_history(view);
    let saved_version = doc.version();
    let saved_revision = doc.get_current_revision();
    let config = BTreeMap::from([(
        "observer".into(),
        observing(
            dir.path(),
            &["document-saved"],
            &[
                Route {
                    event: "document-saved",
                    response: status("written snapshot observed"),
                    expected: vec![
                        r#""text":"submitted\n""#.into(),
                        format!("\"saved_version\":{saved_version}"),
                        format!("\"saved_revision\":{saved_revision}"),
                        format!("\"current_version\":{}", saved_version + 1),
                        format!(
                            "\"path\":{}",
                            serde_json::to_string(&target.to_string_lossy())?
                        ),
                        r#""view":null"#.into(),
                    ],
                    ..Route::default()
                },
                Route {
                    event: "shutdown",
                    response: status("shutdown after save"),
                    ..Route::default()
                },
            ],
            Some("document-saved"),
        )?,
    )]);
    assert!(fixture.editor.reload_plugins(&config, dir.path()));
    fixture.editor.save(id, Some(target.clone()), false)?;
    fixture.replace("newer\n");
    fixture.editor.flush_writes().await?;
    assert_eq!(std::fs::read_to_string(&target)?, "submitted\n");
    assert_eq!(
        fixture.editor.document(id).unwrap().path(),
        Some(target.as_path())
    );
    // Shutdown must pump the queued save hook even when no frontend runs again.
    fixture.editor.shutdown_plugins();
    assert_eq!(
        fixture.editor.get_status().unwrap().0,
        "shutdown after save"
    );
    assert_eq!(
        fixture.editor.document(id).unwrap().text().to_string(),
        "newer\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn write_flush_continues_after_failure_and_reports_only_successful_saves(
) -> anyhow::Result<()> {
    use super::support::plugin_guest::{observing, status, Route};
    let mut fixture = Fixture::new("first\n")?;
    let first = current_ref!(fixture.editor).1.id();
    let dir = tempfile::tempdir()?;
    let good = dir.path().join("good.txt");
    let config = BTreeMap::from([(
        "observer".into(),
        observing(
            dir.path(),
            &["document-saved"],
            &[Route {
                event: "document-saved",
                response: status("successful save only"),
                expected: vec![format!(
                    "\"path\":{}",
                    serde_json::to_string(&good.to_string_lossy())?
                )],
                ..Route::default()
            }],
            None,
        )?,
    )]);
    assert!(fixture.editor.reload_plugins(&config, dir.path()));
    fixture
        .editor
        .save(first, Some(dir.path().join("missing/failed.txt")), false)?;
    let second = fixture.editor.new_file(EditorAction::Replace);
    fixture.editor.save(second, Some(good.clone()), false)?;
    assert!(fixture.editor.flush_writes().await.is_err());
    fixture.editor.poll_plugin_events();
    assert_eq!(fixture.editor.write_count, 0);
    assert!(good.exists());
    assert_eq!(
        fixture.editor.document(second).unwrap().path(),
        Some(good.as_path())
    );
    assert_eq!(
        fixture.editor.get_status().unwrap().0,
        "successful save only"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn other_plugins_observe_mutations_with_provenance_without_own_echoes() -> anyhow::Result<()>
{
    use super::support::plugin_guest::{observing, status, Route};
    let mut fixture = Fixture::new("original\n")?;
    let (_, doc) = current_ref!(fixture.editor);
    let alpha = tempfile::tempdir()?;
    let beta = tempfile::tempdir()?;
    let config = BTreeMap::from([
        (
            "alpha".into(),
            observing(
                alpha.path(),
                &["document-changed"],
                &[
                    Route {
                        event: "command",
                        response: Response {
                            actions: vec![Action::Edit {
                                document: doc.id().as_u64(),
                                version: doc.version(),
                                edits: vec![TextEdit {
                                    start: 0,
                                    end: 8,
                                    text: "CHANGED!".into(),
                                }],
                            }],
                            error: None,
                        },
                        ..Route::default()
                    },
                    Route {
                        event: "document-changed",
                        response: Response {
                            actions: vec![],
                            error: Some("own echo was delivered".into()),
                        },
                        ..Route::default()
                    },
                ],
                None,
            )?,
        ),
        (
            "beta".into(),
            observing(
                beta.path(),
                &["document-changed"],
                &[Route {
                    event: "document-changed",
                    response: status("other plugin observed alpha"),
                    expected: vec![
                        r#""origin_plugin":"alpha""#.into(),
                        r#""parent_sequence":2"#.into(),
                        r#""depth":1"#.into(),
                    ],
                    ..Route::default()
                }],
                None,
            )?,
        ),
    ]);
    assert!(fixture.editor.reload_plugins(&config, alpha.path()));
    fixture.editor.execute_plugin_command("alpha.run", vec![])?;
    drain(&mut fixture);
    assert_eq!(
        current_ref!(fixture.editor).1.text().to_string(),
        "CHANGED!\n"
    );
    assert_eq!(
        fixture.editor.get_status().unwrap().0,
        "other plugin observed alpha"
    );
    assert_eq!(fixture.editor.error_revision(), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn full_callback_and_lifecycle_queues_recover_all_startup_documents() -> anyhow::Result<()> {
    use super::support::plugin_guest::{observing, status, Route};
    use plugin_api::{StateCatalog, StateQuery};
    use view::callbacks::{EditorCallback, EditorCallbackSender};
    let dir = tempfile::tempdir()?;
    // Fill the frontend callback queue. Plugin hooks must never block or lose
    // the retained host work when this wake cannot be sent.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<EditorCallback>(1);
    assert!(tx.send(Box::new(|_| {})).await.is_ok());
    let async_tx = tx.clone();
    let sender = EditorCallbackSender::new(
        move |callback| {
            let tx = async_tx.clone();
            async move {
                let _ = tx.send(callback).await;
            }
        },
        |_| panic!("plugin wake used a blocking callback send"),
    )
    .with_try_send(move |callback| tx.try_send(callback).map_err(|err| err.into_inner()));
    let mut fixture = Fixture::with_handler_setup(
        "original\n",
        "language = []",
        loader::syntax::Resources::default(),
        |config| config.word_completion.enable = false,
        move |handlers, config| *handlers = view::handlers::Handlers::new(config, sender),
    )?;
    let config = BTreeMap::from([(
        "observer".into(),
        observing(
            dir.path(),
            &["document-opened"],
            &[
                Route {
                    event: "resync-required",
                    response: Response {
                        actions: vec![Action::RequestState {
                            query: StateQuery::default(),
                        }],
                        error: None,
                    },
                    expected: vec![r#""queue-capacity""#.into()],
                    ..Route::default()
                },
                Route {
                    event: "state",
                    response: status("startup state recovered"),
                    expected: vec![r#""next_document":null"#.into()],
                    ..Route::default()
                },
            ],
            None,
        )?,
    )]);
    assert!(fixture.editor.reload_plugins(&config, dir.path()));
    let mut opened = Vec::new();
    for index in 0..40 {
        let path = dir.path().join(format!("startup-{index}.txt"));
        std::fs::write(&path, format!("document {index}\n"))?;
        opened.push(fixture.editor.open(&path, EditorAction::Load)?);
    }
    assert_eq!(rx.len(), 1, "one full callback destination stays bounded");
    drop(rx.try_recv()?);
    fixture.editor.poll_plugin_events();
    assert_eq!(
        fixture.editor.get_status().unwrap().0,
        "startup state recovered"
    );
    assert!(rx.len() <= 1, "at most one wake can be scheduled");
    let (context, catalog) = fixture.editor.plugin_state(&StateQuery::default());
    assert!(context.document.is_none());
    let wire_catalog: StateCatalog = serde_json::from_value(serde_json::to_value(catalog)?)?;
    for id in opened {
        assert!(wire_catalog
            .documents
            .iter()
            .any(|doc| doc.id == id.as_u64()));
    }
    let mut after_document = None;
    let mut listed = Vec::new();
    loop {
        let (_, page) = fixture.editor.plugin_state(&StateQuery {
            after_document,
            limit: 3,
            ..StateQuery::default()
        });
        assert!(page.documents.len() <= 3);
        listed.extend(page.documents.iter().map(|doc| doc.id));
        after_document = page.next_document;
        if after_document.is_none() {
            break;
        }
    }
    assert_eq!(
        listed,
        fixture
            .editor
            .documents
            .keys()
            .map(|id| id.as_u64())
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_bytes_and_causal_feedback_have_explicit_recovery_limits() -> anyhow::Result<()> {
    use super::support::plugin_guest::{observing, status, Route};
    let mut fixture = Fixture::new("small\n")?;
    let dir = tempfile::tempdir()?;
    let config = BTreeMap::from([(
        "observer".into(),
        observing(
            dir.path(),
            &["post-command"],
            &[
                Route {
                    event: "post-command",
                    ..Route::default()
                },
                Route {
                    event: "resync-required",
                    response: status("byte limit reported"),
                    expected: vec![r#""queue-capacity""#.into()],
                    ..Route::default()
                },
            ],
            None,
        )?,
    )]);
    assert!(fixture.editor.reload_plugins(&config, dir.path()));
    fixture.replace(&format!("{}\n", "x".repeat(512 * 1024)));
    for _ in 0..20 {
        fixture
            .editor
            .queue_plugin_event(Event::PostCommand, serde_json::Value::Null);
    }
    drain(&mut fixture);
    assert_eq!(
        fixture.editor.get_status().unwrap().0,
        "byte limit reported"
    );

    let mut fixture = Fixture::new("seed\n")?;
    let id = current_ref!(fixture.editor).1.id();
    let alpha = tempfile::tempdir()?;
    let beta = tempfile::tempdir()?;
    let mut routes = (1..64)
        .map(|version| Route {
            event: "document-changed",
            filter: Some(format!("\"version\":{version},")),
            response: Response {
                actions: vec![Action::Edit {
                    document: id.as_u64(),
                    version,
                    edits: vec![TextEdit {
                        start: 0,
                        end: 0,
                        text: "X".into(),
                    }],
                }],
                error: None,
            },
            ..Route::default()
        })
        .collect::<Vec<_>>();
    routes.push(Route {
        event: "resync-required",
        response: status("causal limit reported"),
        expected: vec![r#""causal-depth""#.into()],
        ..Route::default()
    });
    let config = BTreeMap::from([
        (
            "alpha".into(),
            observing(alpha.path(), &["document-changed"], &routes, None)?,
        ),
        (
            "beta".into(),
            observing(beta.path(), &["document-changed"], &routes, None)?,
        ),
    ]);
    assert!(fixture.editor.reload_plugins(&config, alpha.path()));
    fixture.replace("seed\n");
    drain(&mut fixture);
    assert!(
        current_ref!(fixture.editor).1.version() <= 12,
        "feedback must stop before the guest runs out of prepared responses"
    );
    assert_eq!(
        fixture.editor.get_status().unwrap().0,
        "causal limit reported"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn focus_lost_identifies_the_previous_document_and_split() -> anyhow::Result<()> {
    use super::support::plugin_guest::{observing, status, Route};
    let mut fixture = Fixture::new("first\n")?;
    let (view, doc) = current_ref!(fixture.editor);
    let id = doc.id().as_u64();
    let view = view.id.as_u64();
    let dir = tempfile::tempdir()?;
    let config = BTreeMap::from([(
        "observer".into(),
        observing(
            dir.path(),
            &["document-focus-lost"],
            &[Route {
                event: "document-focus-lost",
                response: status("old binding observed"),
                expected: vec![
                    format!("\"document\":{id}"),
                    format!("\"view\":{view}"),
                    r#""text":"first\n""#.into(),
                ],
                ..Route::default()
            }],
            None,
        )?,
    )]);
    assert!(fixture.editor.reload_plugins(&config, dir.path()));
    fixture.editor.new_file(EditorAction::Replace);
    drain(&mut fixture);
    assert_eq!(
        fixture.editor.get_status().unwrap().0,
        "old binding observed"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn event_recipients_share_one_frozen_snapshot_and_conflicts_are_deterministic(
) -> anyhow::Result<()> {
    use super::support::plugin_guest::{observing, Route};
    let mut fixture = Fixture::new("original\n")?;
    let (_, doc) = current_ref!(fixture.editor);
    let id = doc.id().as_u64();
    let version = doc.version();
    let alpha = tempfile::tempdir()?;
    let beta = tempfile::tempdir()?;
    let response = |text: &str| Response {
        actions: vec![Action::Edit {
            document: id,
            version,
            edits: vec![TextEdit {
                start: 0,
                end: 8,
                text: text.into(),
            }],
        }],
        error: None,
    };
    let config = BTreeMap::from([
        (
            "alpha".into(),
            observing(
                alpha.path(),
                &["post-command"],
                &[Route {
                    event: "post-command",
                    response: response("FIRST!!!"),
                    ..Route::default()
                }],
                None,
            )?,
        ),
        (
            "beta".into(),
            observing(
                beta.path(),
                &["post-command"],
                &[Route {
                    event: "post-command",
                    response: response("SECOND!!"),
                    expected: vec![
                        format!("\"version\":{version},"),
                        r#""text":"original\n""#.into(),
                    ],
                    ..Route::default()
                }],
                None,
            )?,
        ),
    ]);
    assert!(fixture.editor.reload_plugins(&config, alpha.path()));
    assert!(!fixture
        .editor
        .dispatch_plugin_event(Event::PostCommand, serde_json::Value::Null));
    assert_eq!(
        current_ref!(fixture.editor).1.text().to_string(),
        "FIRST!!!\n"
    );
    let message = fixture.editor.get_status().unwrap().0.as_ref();
    assert!(message.contains("plugin 'beta'"));
    assert!(message.contains("stale document version"));
    assert_eq!(fixture.editor.error_revision(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_disabled_guest_stops_snapshot_capture_and_callback_wakes_immediately(
) -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let dir = tempfile::tempdir()?;
    let wasm = wat::parse_str(
        r#"(module
        (memory (export "memory") 1) (data (i32.const 16) "{}")
        (global $initialized (mut i32) (i32.const 0))
        (func (export "mitos_alloc") (param i32) (result i32) i32.const 32768)
        (func (export "mitos_dealloc") (param i32 i32))
        (func (export "mitos_call") (param i32 i32) (result i64)
            global.get $initialized if unreachable end
            i32.const 1 global.set $initialized
            i64.const 68719476738))"#,
    )?;
    std::fs::write(dir.path().join("plugin.wasm"), wasm)?;
    std::fs::write(dir.path().join("plugin.toml"), format!("abi-version = {}\nmodule = 'plugin.wasm'\ncapabilities = ['ui', 'editor-read', 'editor-edit', 'editor-selection', 'editor-navigate', 'workspace-read']\nevents = ['post-command','document-opened']\n", plugin_api::ABI_VERSION))?;
    let config = BTreeMap::from([(
        "trap".into(),
        PluginConfig {
            path: dir.path().join("plugin.toml"),
            enabled: true,
            config: serde_json::Value::Null,
            permissions: crate::support::plugin_guest::permissions(dir.path()),
            ..PluginConfig::default()
        },
    )]);
    assert!(fixture.editor.reload_plugins(&config, dir.path()));
    assert!(!fixture
        .editor
        .dispatch_plugin_event(Event::PostCommand, serde_json::Value::Null));
    // Clearing existing unrelated completions isolates new plugin wakes.
    while let Ok(callback) = fixture.callbacks.try_recv() {
        callback(&mut fixture.editor);
    }
    for _ in 0..40 {
        fixture
            .editor
            .queue_plugin_event(Event::PostCommand, serde_json::Value::Null);
    }
    assert!(fixture.callbacks.try_recv().is_err());
    fixture.editor.poll_plugin_events();
    assert!(fixture
        .editor
        .get_status()
        .unwrap()
        .0
        .contains("plugin 'trap'"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn write_invocation_observers_finish_once_for_success_failure_and_cancellation(
) -> anyhow::Result<()> {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use view::callbacks::{InvocationTasks, TaskOutcome};
    #[derive(Default)]
    struct Observer {
        started: AtomicUsize,
        outcomes: Mutex<Vec<TaskOutcome>>,
    }
    impl InvocationTasks for Observer {
        fn started(&self) {
            self.started.fetch_add(1, Ordering::Relaxed);
        }
        fn finished(&self, outcome: TaskOutcome) {
            self.outcomes.lock().unwrap().push(outcome);
        }
    }
    let observer = Arc::new(Observer::default());
    let mut fixture = Fixture::new("original\n")?;
    let id = current_ref!(fixture.editor).1.id();
    fixture
        .editor
        .replace_invocation_tasks(Some(observer.clone()));
    fixture.editor.save(id, None::<std::path::PathBuf>, false)?;
    assert_eq!(observer.started.load(Ordering::Relaxed), 1);
    assert!(observer.outcomes.lock().unwrap().is_empty());
    // The captured task survives restoration of the command's observer scope.
    fixture.editor.replace_invocation_tasks(None);
    fixture.editor.flush_writes().await?;
    assert_eq!(
        *observer.outcomes.lock().unwrap(),
        vec![TaskOutcome::Success]
    );

    fixture
        .editor
        .replace_invocation_tasks(Some(observer.clone()));
    fixture.editor.save(
        id,
        Some(fixture.dir.path().join("missing/failed.txt")),
        false,
    )?;
    fixture.editor.replace_invocation_tasks(None);
    assert!(fixture.editor.flush_writes().await.is_err());
    assert_eq!(observer.started.load(Ordering::Relaxed), 2);
    assert!(matches!(
        observer.outcomes.lock().unwrap().as_slice(),
        [TaskOutcome::Success, TaskOutcome::Error(_)]
    ));

    let mut cancelled = Fixture::new("cancelled\n")?;
    let cancelled_id = current_ref!(cancelled.editor).1.id();
    cancelled
        .editor
        .replace_invocation_tasks(Some(observer.clone()));
    cancelled
        .editor
        .save(cancelled_id, None::<std::path::PathBuf>, false)?;
    drop(cancelled);
    assert_eq!(observer.started.load(Ordering::Relaxed), 3);
    assert!(matches!(
        observer.outcomes.lock().unwrap().as_slice(),
        [
            TaskOutcome::Success,
            TaskOutcome::Error(_),
            TaskOutcome::Cancelled(_)
        ]
    ));

    // A disconnected write queue rejects the submission with one Error outcome;
    // dropping its returned future must not add a second cancellation outcome.
    fixture.editor.save_queue.clear();
    fixture
        .editor
        .replace_invocation_tasks(Some(observer.clone()));
    assert!(fixture
        .editor
        .save(id, None::<std::path::PathBuf>, false)
        .is_err());
    assert_eq!(observer.started.load(Ordering::Relaxed), 4);
    assert!(matches!(
        observer.outcomes.lock().unwrap().as_slice(),
        [
            TaskOutcome::Success,
            TaskOutcome::Error(_),
            TaskOutcome::Cancelled(_),
            TaskOutcome::Error(_)
        ]
    ));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delayed_frontend_events_retain_the_original_view_binding() -> anyhow::Result<()> {
    use crate::support::plugin_guest::{observing, status, Route};
    let mut fixture = Fixture::new("original\n")?;
    let (view, doc) = current_ref!(fixture.editor);
    let origin = view.id;
    let binding = view.binding_revision();
    let document = doc.id();
    let dir = tempfile::tempdir()?;
    let original = format!("\"document\":{{\"id\":{}", document.as_u64());
    let config = observing(
        dir.path(),
        &["post-command"],
        &[
            Route {
                event: "post-command",
                filter: Some("\"case\":\"bound\"".into()),
                expected: vec![
                    original.clone(),
                    format!("\"view\":{{\"id\":{}", origin.as_u64()),
                ],
                response: status("bound"),
            },
            Route {
                event: "post-command",
                filter: Some("\"case\":\"unbound\"".into()),
                expected: vec![
                    original,
                    "\"text\":\"original\\n\"".into(),
                    "\"view\":null".into(),
                ],
                response: status("unbound"),
            },
            Route {
                event: "post-command",
                filter: Some("\"case\":\"closed\"".into()),
                expected: vec!["\"document\":null".into(), "\"view\":null".into()],
                response: status("closed"),
            },
        ],
        None,
    )?;
    assert!(fixture
        .editor
        .reload_plugins(&BTreeMap::from([("fixture".into(), config)]), dir.path()));
    fixture.editor.queue_plugin_event_for_view_binding(
        Event::PostCommand,
        origin,
        document,
        binding,
        serde_json::json!({"case":"bound"}),
    );
    drain(&mut fixture);
    assert_eq!(fixture.editor.status_msg.as_ref().unwrap().0, "bound");

    fixture.editor.new_file(EditorAction::Replace);
    fixture.editor.queue_plugin_event_for_view_binding(
        Event::PostCommand,
        origin,
        document,
        binding,
        serde_json::json!({"case":"unbound"}),
    );
    drain(&mut fixture);
    assert_eq!(fixture.editor.status_msg.as_ref().unwrap().0, "unbound");
    // Returning to the original document must not revive the expired binding.
    fixture.editor.switch(document, EditorAction::Replace);
    fixture.editor.queue_plugin_event_for_view_binding(
        Event::PostCommand,
        origin,
        document,
        binding,
        serde_json::json!({"case":"unbound"}),
    );
    drain(&mut fixture);
    assert_eq!(fixture.editor.status_msg.as_ref().unwrap().0, "unbound");
    fixture.editor.close(origin);
    fixture.editor.queue_plugin_event_for_view_binding(
        Event::PostCommand,
        origin,
        document,
        binding,
        serde_json::json!({"case":"unbound"}),
    );
    drain(&mut fixture);
    assert_eq!(fixture.editor.status_msg.as_ref().unwrap().0, "unbound");
    assert!(fixture.editor.close_document(document, true).is_ok());
    fixture.editor.queue_plugin_event_for_view_binding(
        Event::PostCommand,
        origin,
        document,
        binding,
        serde_json::json!({"case":"closed"}),
    );
    drain(&mut fixture);
    assert_eq!(fixture.editor.status_msg.as_ref().unwrap().0, "closed");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn capability_denial_rejects_the_complete_response() -> anyhow::Result<()> {
    use plugin_api::{Capability, ErrorCode, ServiceError};
    let mut fixture = Fixture::new("original\n")?;
    let doc = current_ref!(fixture.editor).1;
    let dir = tempfile::tempdir()?;
    let mut config = plugin(
        dir.path(),
        Response {
            actions: vec![
                Action::Status {
                    message: "must not appear".into(),
                },
                Action::Edit {
                    document: doc.id().as_u64(),
                    version: doc.version(),
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
    config
        .get_mut("fixture")
        .unwrap()
        .permissions
        .capabilities
        .remove(&Capability::EditorEdit);
    assert!(fixture.editor.reload_plugins(&config, dir.path()));
    let error = fixture
        .editor
        .execute_plugin_command("fixture.run", vec![])
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<ServiceError>().unwrap().code,
        ErrorCode::PermissionDenied
    );
    assert_eq!(
        current_ref!(fixture.editor).1.text().to_string(),
        "original\n"
    );
    assert!(fixture
        .editor
        .status_msg
        .as_ref()
        .is_none_or(|message| message.0 != "must not appear"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn status_only_guests_do_not_capture_large_buffer_text() -> anyhow::Result<()> {
    use crate::support::plugin_guest::{observing, status, Route};
    use plugin_api::{Capability, Permissions};
    let mut fixture = Fixture::new(&"sensitive".repeat(300_000))?;
    let dir = tempfile::tempdir()?;
    let mut config = observing(
        dir.path(),
        &["document-opened"],
        &[Route {
            event: "command",
            expected: vec![
                "\"document\":null".into(),
                "\"view\":null".into(),
                "\"args\":[\"explicit argument\"]".into(),
            ],
            response: status("allowed"),
            ..Route::default()
        }],
        None,
    )?;
    config.permissions = Permissions {
        capabilities: [Capability::Ui].into(),
        ..Permissions::default()
    };
    assert!(fixture
        .editor
        .reload_plugins(&BTreeMap::from([("fixture".into(), config)]), dir.path()));
    fixture
        .editor
        .execute_plugin_command("fixture.run", vec!["explicit argument".into()])?;
    assert_eq!(fixture.editor.status_msg.as_ref().unwrap().0, "allowed");
    fixture.editor.new_file(EditorAction::VerticalSplit);
    assert!(
        fixture.callbacks.try_recv().is_err(),
        "ungranted document subscriptions must not capture or schedule"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn scoped_plugin_opens_suppress_ambient_reload_and_save_until_native_adoption(
) -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("opened.txt");
    std::fs::write(&path, "owned bytes\n")?;
    let config = plugin(
        dir.path(),
        Response {
            actions: vec![Action::Open {
                path: path.to_string_lossy().into_owned(),
            }],
            error: None,
        },
        &[],
    )?;
    assert!(fixture.editor.reload_plugins(&config, dir.path()));
    fixture
        .editor
        .execute_plugin_command("fixture.run", vec![])?;
    let (view, doc) = current!(fixture.editor);
    let id = doc.id();
    assert!(doc.is_restricted_adoption());
    assert_eq!(doc.text().to_string(), "owned bytes\n");
    let transaction = Transaction::change(doc.text(), [(0, 0, Some("X".into()))].into_iter());
    assert!(doc.apply(&transaction, view.id));
    doc.append_changes_to_history(view);
    view::save::auto_save(&mut fixture.editor)?;
    assert_eq!(fixture.editor.write_count, 0);
    assert!(fixture
        .editor
        .document(id)
        .unwrap()
        .is_restricted_adoption());
    std::fs::write(&path, "external change\n")?;
    let mut config = (**fixture.config.load()).clone();
    config.auto_reload.enable = true;
    fixture.config.store(std::sync::Arc::new(config));
    view::handlers::auto_reload::check_unwatched(&mut fixture.editor);
    assert_eq!(
        fixture.editor.document(id).unwrap().text().to_string(),
        "Xowned bytes\n"
    );
    assert_eq!(fixture.editor.open(&path, EditorAction::Replace)?, id);
    assert!(!fixture
        .editor
        .document(id)
        .unwrap()
        .is_restricted_adoption());
    view::save::auto_save(&mut fixture.editor)?;
    assert_eq!(fixture.editor.write_count, 1);
    fixture.editor.flush_writes().await?;
    assert_eq!(std::fs::read_to_string(path)?, "Xowned bytes\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unsafe_or_binary_opens_leave_earlier_edits_and_navigation_unapplied() -> anyhow::Result<()>
{
    use plugin_api::{ErrorCode, ServiceError};
    for target in ["missing.txt", "binary.png"] {
        let mut fixture = Fixture::new("original\n")?;
        let doc = current_ref!(fixture.editor).1;
        let id = doc.id();
        let version = doc.version();
        let dir = tempfile::tempdir()?;
        let first = dir.path().join("first.txt");
        std::fs::write(&first, "first\n")?;
        std::fs::write(dir.path().join("binary.png"), b"\x89PNG\r\n\x1a\n")?;
        let config = plugin(
            dir.path(),
            Response {
                actions: vec![
                    Action::Edit {
                        document: id.as_u64(),
                        version,
                        edits: vec![TextEdit {
                            start: 0,
                            end: 1,
                            text: "X".into(),
                        }],
                    },
                    Action::Open {
                        path: first.to_string_lossy().into_owned(),
                    },
                    Action::Open {
                        path: dir.path().join(target).to_string_lossy().into_owned(),
                    },
                ],
                error: None,
            },
            &[],
        )?;
        assert!(fixture.editor.reload_plugins(&config, dir.path()));
        let count = fixture.editor.documents().count();
        let error = fixture
            .editor
            .execute_plugin_command("fixture.run", vec![])
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ServiceError>().unwrap().code,
            ErrorCode::PermissionDenied | ErrorCode::InvalidRequest
        ));
        assert_eq!(fixture.editor.documents().count(), count);
        assert_eq!(current_ref!(fixture.editor).1.id(), id);
        assert_eq!(
            fixture.editor.document(id).unwrap().text().to_string(),
            "original\n"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_status_is_bounded_and_has_no_terminal_controls() -> anyhow::Result<()> {
    let mut fixture = Fixture::new("original\n")?;
    let dir = tempfile::tempdir()?;
    let config = plugin(
        dir.path(),
        Response {
            actions: vec![Action::Status {
                message: format!("\x1b\x07\n{}", "é".repeat(4000)),
            }],
            error: None,
        },
        &[],
    )?;
    assert!(fixture.editor.reload_plugins(&config, dir.path()));
    fixture
        .editor
        .execute_plugin_command("fixture.run", vec![])?;
    let message = &fixture.editor.status_msg.as_ref().unwrap().0;
    assert!(message.len() <= 4096);
    assert!(!message.chars().any(char::is_control));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delayed_open_rejects_changed_selection_text_or_focus() -> anyhow::Result<()> {
    for change in ["selection", "text", "focus"] {
        let mut fixture = Fixture::new("original\n")?;
        let dir = tempfile::tempdir()?;
        let target = dir.path().join("opened.txt");
        std::fs::write(&target, "opened\n")?;
        let config = plugin(
            dir.path(),
            Response {
                actions: vec![Action::Open {
                    path: target.to_string_lossy().into_owned(),
                }],
                error: None,
            },
            &["post-command"],
        )?;
        assert!(fixture.editor.reload_plugins(&config, dir.path()));
        fixture
            .editor
            .queue_plugin_event(Event::PostCommand, serde_json::json!({"command":"run"}));
        match change {
            "selection" => {
                let (view, doc) = current!(fixture.editor);
                doc.set_selection(view.id, Selection::single(1, 2));
            }
            "text" => {
                let (view, doc) = current!(fixture.editor);
                let transaction =
                    Transaction::change(doc.text(), [(0, 0, Some("X".into()))].into_iter());
                assert!(doc.apply(&transaction, view.id));
            }
            "focus" => {
                fixture.editor.new_file(EditorAction::VerticalSplit);
            }
            _ => unreachable!(),
        }
        let focused = fixture.editor.tree.focus;
        let count = fixture.editor.documents().count();
        drain(&mut fixture);
        assert_eq!(fixture.editor.tree.focus, focused);
        assert_eq!(fixture.editor.documents().count(), count);
        let error = fixture.editor.status_msg.as_ref().unwrap().0.as_ref();
        let expected = match change {
            "selection" => "stale selection",
            "text" => "stale document",
            _ => "no longer focused",
        };
        assert!(error.contains(expected), "{error}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_rejects_guest_navigation_and_edits_before_any_effect() -> anyhow::Result<()> {
    for open in [false, true] {
        let mut fixture = Fixture::new("original\n")?;
        let doc = current_ref!(fixture.editor).1;
        let id = doc.id();
        let dir = tempfile::tempdir()?;
        let target = dir.path().join("opened.txt");
        std::fs::write(&target, "opened\n")?;
        let action = if open {
            Action::Open {
                path: target.to_string_lossy().into_owned(),
            }
        } else {
            Action::Edit {
                document: id.as_u64(),
                version: doc.version(),
                edits: vec![TextEdit {
                    start: 0,
                    end: 1,
                    text: "X".into(),
                }],
            }
        };
        let config = plugin(
            dir.path(),
            Response {
                actions: vec![
                    Action::Status {
                        message: "must not apply".into(),
                    },
                    action,
                ],
                error: None,
            },
            &[],
        )?;
        assert!(fixture.editor.reload_plugins(&config, dir.path()));
        let count = fixture.editor.documents().count();
        fixture.editor.shutdown_plugins();
        assert_eq!(fixture.editor.documents().count(), count);
        assert_eq!(
            fixture.editor.document(id).unwrap().text().to_string(),
            "original\n"
        );
        assert!(fixture
            .editor
            .status_msg
            .as_ref()
            .unwrap()
            .0
            .contains("only diagnostics"));
    }
    Ok(())
}
