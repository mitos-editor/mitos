//! Public SDK calls exercise closed native services and their real actor lifetime.
use super::Fixture;
use plugin_api::{
    editor::{
        DocumentTarget, EditorRequest, OpenDisposition, SettingValue, SettingsScope, ViewTarget,
    },
    Capability, ErrorCode, HostFuture,
};
use plugins::PluginConfig;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use view::{
    clipboard::{ClipboardBackend, ClipboardProvider, ClipboardType},
    current, current_ref,
    editor::Action,
};

fn target(fixture: &Fixture) -> ViewTarget {
    let (view, doc) = current_ref!(fixture.editor);
    ViewTarget {
        view: view.id.as_u64(),
        document: doc.id().as_u64(),
        version: doc.version(),
        binding_revision: view.binding_revision(),
        selection_revision: doc.selection_revision(view.id).unwrap(),
    }
}
fn service(request: EditorRequest, expected: &[&str]) -> serde_json::Value {
    serde_json::json!({"request":request,"expected":expected})
}
fn error(request: EditorRequest, code: ErrorCode) -> serde_json::Value {
    serde_json::json!({"request":request,"error_code":code})
}
fn package(
    dir: &Path,
    requests: Vec<serde_json::Value>,
    caps: &[Capability],
) -> anyhow::Result<PluginConfig> {
    let mut package = crate::support::plugin_guest::observing(dir, &[], &[], None)?;
    let path = dir.join("plugin.toml");
    let mut manifest: toml::Value = toml::from_str(&std::fs::read_to_string(&path)?)?;
    let mut declared = package.permissions.capabilities.clone();
    declared.extend(caps);
    manifest["capabilities"] = toml::Value::try_from(&declared)?;
    std::fs::write(path, toml::to_string(&manifest)?)?;
    package.permissions.capabilities = declared;
    package.config = serde_json::json!({"routes":[{"event":"command","requests":requests,
        "response":{"actions":[{"type":"status","message":"services completed"}]}}]});
    Ok(package)
}
async fn drain(fixture: &mut Fixture) {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            fixture.editor.poll_plugin_events();
            while let Ok(callback) = fixture.callbacks.try_recv() {
                callback(&mut fixture.editor);
            }
            if !fixture.editor.has_pending_plugin_work() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("editor service work did not settle");
}
async fn load(fixture: &mut Fixture, dir: &Path, packages: BTreeMap<String, PluginConfig>) {
    assert!(
        fixture.editor.reload_plugins(&packages, dir),
        "service package reload was rejected: status={:?}; diagnostics={:?}",
        fixture.editor.get_status(),
        fixture.editor.plugin_diagnostics()
    );
    drain(fixture).await;
    for (name, package) in packages {
        if package.enabled {
            assert!(
                fixture
                    .editor
                    .plugin_command_doc(&format!("{name}.run"))
                    .is_some(),
                "service package '{name}' did not activate: status={:?}; diagnostics={:?}",
                fixture.editor.get_status(),
                fixture.editor.plugin_diagnostics()
            );
        }
    }
}
async fn run(fixture: &mut Fixture, name: &str) -> anyhow::Result<()> {
    let errors = fixture.editor.error_revision();
    assert!(
        fixture.editor.execute_plugin_command(name, vec![])?,
        "service command '{name}' is unavailable: status={:?}; diagnostics={:?}",
        fixture.editor.get_status(),
        fixture.editor.plugin_diagnostics()
    );
    drain(fixture).await;
    anyhow::ensure!(
        fixture.editor.error_revision() == errors,
        "{}",
        fixture
            .editor
            .get_status()
            .map(|status| status.0.as_ref())
            .unwrap_or("unknown error")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn document_status_reports_unsaved_changes_and_rejects_old_versions() -> anyhow::Result<()> {
    let _compilation = crate::support::plugin_guest::compilation_permit().await;
    let mut fixture = Fixture::new("original\n")?;
    view::doc_mut!(fixture.editor).reset_modified();
    let initial = target(&fixture);
    let saved = DocumentTarget {
        document: initial.document,
        version: initial.version,
    };
    let dir = tempfile::tempdir()?;
    let config = package(
        dir.path(),
        vec![
            service(
                EditorRequest::DocumentStatus { target: saved },
                &["\"modified\":false"],
            ),
            service(
                EditorRequest::UnsavedDocuments { max_documents: 0 },
                &["\"documents\":[]", "\"truncated\":false"],
            ),
        ],
        &[],
    )?;
    load(
        &mut fixture,
        dir.path(),
        BTreeMap::from([("fixture".into(), config)]),
    )
    .await;
    run(&mut fixture, "fixture.run").await?;
    let (view, doc) = current!(fixture.editor);
    let change =
        editor_core::Transaction::insert(doc.text(), doc.selection(view.id), "changed".into());
    doc.apply(&change, view.id);
    let current = DocumentTarget {
        document: doc.id().as_u64(),
        version: doc.version(),
    };
    let config = package(
        dir.path(),
        vec![
            service(
                EditorRequest::DocumentStatus { target: current },
                &["\"modified\":true"],
            ),
            error(
                EditorRequest::DocumentStatus { target: saved },
                ErrorCode::StaleState,
            ),
        ],
        &[],
    )?;
    load(
        &mut fixture,
        dir.path(),
        BTreeMap::from([("fixture".into(), config)]),
    )
    .await;
    run(&mut fixture, "fixture.run").await?;
    fixture.editor.new_file(Action::Replace);
    let (view, doc) = current!(fixture.editor);
    let change =
        editor_core::Transaction::insert(doc.text(), doc.selection(view.id), "second".into());
    doc.apply(&change, view.id);
    let second = DocumentTarget {
        document: doc.id().as_u64(),
        version: doc.version(),
    };
    let first_json = serde_json::to_string(&current)?;
    let second_json = serde_json::to_string(&second)?;
    let config = package(
        dir.path(),
        vec![
            service(
                EditorRequest::UnsavedDocuments { max_documents: 0 },
                &[
                    &first_json,
                    &second_json,
                    "\"path\":null",
                    "\"truncated\":false",
                ],
            ),
            service(
                EditorRequest::UnsavedDocuments { max_documents: 1 },
                &["\"truncated\":true"],
            ),
            error(
                EditorRequest::UnsavedDocuments { max_documents: 129 },
                ErrorCode::ResourceExhausted,
            ),
        ],
        &[],
    )?;
    load(
        &mut fixture,
        dir.path(),
        BTreeMap::from([("fixture".into(), config)]),
    )
    .await;
    run(&mut fixture, "fixture.run").await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_scratch_can_be_read_and_edited_in_the_creating_invocation() -> anyhow::Result<()> {
    let _compilation = crate::support::plugin_guest::compilation_permit().await;
    let mut fixture = Fixture::new("original\n")?;
    let original = target(&fixture);
    let dir = tempfile::tempdir()?;
    let mut config = package(dir.path(), vec![], &[])?;
    config.config["routes"][0]["operation"] = "scratch".into();
    load(
        &mut fixture,
        dir.path(),
        BTreeMap::from([("fixture".into(), config)]),
    )
    .await;
    run(&mut fixture, "fixture.run").await?;
    let (view, doc) = current!(fixture.editor);
    assert_ne!(doc.id().as_u64(), original.document);
    assert_eq!(doc.display_name(), "Guest scratch");
    assert_eq!(doc.text().to_string(), "ÉSS\n");
    assert!(doc.undo(view));
    assert_eq!(doc.text().to_string(), "éß\n");
    assert_eq!(fixture.editor.get_status().unwrap().0, "scratch edited");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_at_uses_its_explicit_unfocused_origin_and_unicode_column() -> anyhow::Result<()> {
    let _compilation = crate::support::plugin_guest::compilation_permit().await;
    let mut fixture = Fixture::new("original\n")?;
    let origin = target(&fixture);
    let original_doc = current_ref!(fixture.editor).1.id();
    fixture.editor.switch(original_doc, Action::VerticalSplit);
    let other_view = fixture.editor.tree.focus;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("unicode.txt");
    std::fs::write(&path, "first\néßz\n")?;
    let config = package(
        dir.path(),
        vec![service(
            EditorRequest::OpenAt {
                origin: Some(origin),
                path: path.to_string_lossy().into_owned(),
                line: 1,
                column: 2,
                action: OpenDisposition::Replace,
            },
            &["\"kind\":\"view\""],
        )],
        &[],
    )?;
    load(
        &mut fixture,
        dir.path(),
        BTreeMap::from([("fixture".into(), config)]),
    )
    .await;
    run(&mut fixture, "fixture.run").await?;
    let (view, doc) = current_ref!(fixture.editor);
    assert_eq!(view.id.as_u64(), origin.view);
    assert_eq!(doc.text().to_string(), "first\néßz\n");
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
        8
    );
    assert_eq!(fixture.editor.tree.get(other_view).doc, original_doc);
    assert!(doc.is_restricted_adoption());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_navigation_and_invalid_coordinates_leave_focus_and_documents_untouched(
) -> anyhow::Result<()> {
    let _compilation = crate::support::plugin_guest::compilation_permit().await;
    let mut fixture = Fixture::new("original\n")?;
    let origin = target(&fixture);
    let (view, doc) = current!(fixture.editor);
    doc.set_selection(view.id, editor_core::Selection::point(2));
    let current_origin = target(&fixture);
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("target.txt");
    std::fs::write(&path, "é\n")?;
    let config = package(
        dir.path(),
        vec![
            error(
                EditorRequest::Focus { target: origin },
                ErrorCode::StaleState,
            ),
            error(
                EditorRequest::OpenAt {
                    origin: Some(current_origin),
                    path: path.to_string_lossy().into_owned(),
                    line: 0,
                    column: 99,
                    action: OpenDisposition::Replace,
                },
                ErrorCode::InvalidRequest,
            ),
        ],
        &[],
    )?;
    load(
        &mut fixture,
        dir.path(),
        BTreeMap::from([("fixture".into(), config)]),
    )
    .await;
    let before = fixture.editor.documents.len();
    run(&mut fixture, "fixture.run").await?;
    assert_eq!(fixture.editor.documents.len(), before);
    let (view, doc) = current_ref!(fixture.editor);
    assert_eq!(view.id.as_u64(), origin.view);
    assert_eq!(doc.id().as_u64(), origin.document);
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
        2
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registers_and_setting_overrides_use_owned_limits_and_current_native_baselines(
) -> anyhow::Result<()> {
    let _compilation = crate::support::plugin_guest::compilation_permit().await;
    let mut fixture = Fixture::new("éß text\n")?;
    let origin = target(&fixture);
    let scope = SettingsScope::Document {
        target: DocumentTarget {
            document: origin.document,
            version: origin.version,
        },
    };
    let dir = tempfile::tempdir()?;
    let config = package(
        dir.path(),
        vec![
            service(
                EditorRequest::WriteRegister {
                    name: 'a',
                    values: vec!["é".into(), "ß".into()],
                },
                &["\"kind\":\"updated\""],
            ),
            service(
                EditorRequest::ReadRegister {
                    name: 'a',
                    origin: None,
                },
                &["\"values\":[\"é\",\"ß\"]"],
            ),
            service(
                EditorRequest::ReadRegister {
                    name: '#',
                    origin: Some(origin),
                },
                &["\"values\":[\"1\"]"],
            ),
            service(
                EditorRequest::OverrideSetting {
                    scope,
                    value: SettingValue::CursorLine(true),
                },
                &[],
            ),
            service(
                EditorRequest::OverrideSetting {
                    scope: SettingsScope::Editor,
                    value: SettingValue::AutoFormat(false),
                },
                &[],
            ),
            service(
                EditorRequest::ReadSettings { scope },
                &[
                    "\"kind\":\"auto-format\",\"value\":false",
                    "\"kind\":\"cursor-line\",\"value\":true",
                ],
            ),
        ],
        &[Capability::EditorSettings],
    )?;
    load(
        &mut fixture,
        dir.path(),
        BTreeMap::from([("fixture".into(), config)]),
    )
    .await;
    run(&mut fixture, "fixture.run").await?;
    assert!(current_ref!(fixture.editor).1.plugin_cursorline(false));
    fixture.configure(|config| {
        config.cursorline = false;
        config.auto_format = true;
    });
    assert!(current_ref!(fixture.editor).1.plugin_cursorline(false));
    load(&mut fixture, dir.path(), BTreeMap::new()).await;
    assert!(!current_ref!(fixture.editor).1.plugin_cursorline(false));
    let config = package(
        dir.path(),
        vec![service(
            EditorRequest::ReadSettings { scope },
            &[
                "\"kind\":\"auto-format\",\"value\":true",
                "\"kind\":\"cursor-line\",\"value\":false",
            ],
        )],
        &[],
    )?;
    load(
        &mut fixture,
        dir.path(),
        BTreeMap::from([("fixture".into(), config)]),
    )
    .await;
    run(&mut fixture, "fixture.run").await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trapped_owner_restores_settings_but_keeps_completed_native_register_milestones(
) -> anyhow::Result<()> {
    let _compilation = crate::support::plugin_guest::compilation_permit().await;
    let mut fixture = Fixture::new("original\n")?;
    let dir = tempfile::tempdir()?;
    let mut config = package(
        dir.path(),
        vec![
            service(
                EditorRequest::OverrideSetting {
                    scope: SettingsScope::Editor,
                    value: SettingValue::CursorLine(true),
                },
                &[],
            ),
            service(
                EditorRequest::WriteRegister {
                    name: 'a',
                    values: vec!["native milestone".into()],
                },
                &[],
            ),
        ],
        &[Capability::EditorSettings],
    )?;
    config.config["routes"][0]["operation"] = "trap".into();
    load(
        &mut fixture,
        dir.path(),
        BTreeMap::from([("fixture".into(), config)]),
    )
    .await;
    assert!(run(&mut fixture, "fixture.run").await.is_err());
    assert!(!current_ref!(fixture.editor).1.plugin_cursorline(false));
    assert_eq!(
        fixture
            .editor
            .registers
            .read('a', &fixture.editor)
            .unwrap()
            .map(|value| value.into_owned())
            .collect::<Vec<_>>(),
        ["native milestone"]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_setting_grant_and_large_native_register_return_typed_errors() -> anyhow::Result<()>
{
    let _compilation = crate::support::plugin_guest::compilation_permit().await;
    let mut fixture = Fixture::new("original\n")?;
    fixture
        .editor
        .registers
        .write('a', vec!["x".repeat(4097)])?;
    let dir = tempfile::tempdir()?;
    let mut config = package(
        dir.path(),
        vec![
            error(
                EditorRequest::OverrideSetting {
                    scope: SettingsScope::Editor,
                    value: SettingValue::CursorLine(true),
                },
                ErrorCode::PermissionDenied,
            ),
            error(
                EditorRequest::ReadRegister {
                    name: 'a',
                    origin: None,
                },
                ErrorCode::ResourceExhausted,
            ),
        ],
        &[Capability::EditorSettings],
    )?;
    config
        .permissions
        .capabilities
        .remove(&Capability::EditorSettings);
    load(
        &mut fixture,
        dir.path(),
        BTreeMap::from([("fixture".into(), config)]),
    )
    .await;
    run(&mut fixture, "fixture.run").await?;
    assert!(!current_ref!(fixture.editor).1.plugin_cursorline(false));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_theme_unload_preserves_later_native_theme_changes() -> anyhow::Result<()> {
    let _compilation = crate::support::plugin_guest::compilation_permit().await;
    let mut fixture = Fixture::new("original\n")?;
    let dir = tempfile::tempdir()?;
    let config = package(
        dir.path(),
        vec![service(
            EditorRequest::OverrideSetting {
                scope: SettingsScope::Editor,
                value: SettingValue::Theme("base16_default".into()),
            },
            &[],
        )],
        &[Capability::EditorSettings],
    )?;
    load(
        &mut fixture,
        dir.path(),
        BTreeMap::from([("fixture".into(), config)]),
    )
    .await;
    run(&mut fixture, "fixture.run").await?;
    assert_eq!(fixture.editor.theme.name(), "base16_default");
    // A committed native choice supersedes the owner's older theme baseline.
    fixture
        .editor
        .set_theme(view::theme::DEFAULT_THEME.clone())?;
    load(&mut fixture, dir.path(), BTreeMap::new()).await;
    assert_eq!(fixture.editor.theme.name(), "default");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn own_last_view_close_completes_but_modified_document_close_is_rejected(
) -> anyhow::Result<()> {
    let _compilation = crate::support::plugin_guest::compilation_permit().await;
    let mut fixture = Fixture::new("original\n")?;
    fixture.replace("modified\n");
    let origin = target(&fixture);
    let dir = tempfile::tempdir()?;
    let config = package(
        dir.path(),
        vec![
            error(
                EditorRequest::CloseDocument {
                    target: DocumentTarget {
                        document: origin.document,
                        version: origin.version,
                    },
                },
                ErrorCode::StaleState,
            ),
            service(
                EditorRequest::CloseView { target: origin },
                &["\"kind\":\"closed\""],
            ),
        ],
        &[],
    )?;
    load(
        &mut fixture,
        dir.path(),
        BTreeMap::from([("fixture".into(), config)]),
    )
    .await;
    run(&mut fixture, "fixture.run").await?;
    assert!(fixture.editor.tree.views().next().is_none());
    assert_eq!(
        fixture
            .editor
            .documents
            .values()
            .find(|doc| doc.id().as_u64() == origin.document)
            .unwrap()
            .text()
            .to_string(),
        "modified\n"
    );
    Ok(())
}

struct ClipboardProbe(Arc<AtomicUsize>);
impl ClipboardBackend for ClipboardProbe {
    fn name(&self, _: &ClipboardProvider) -> String {
        "test".into()
    }
    fn get_contents(
        &self,
        _: &ClipboardProvider,
        _: ClipboardType,
    ) -> view::clipboard::Result<String> {
        panic!("synchronous clipboard path")
    }
    fn set_contents(
        &self,
        _: &ClipboardProvider,
        _: &str,
        _: ClipboardType,
    ) -> view::clipboard::Result<()> {
        panic!("synchronous clipboard path")
    }
    fn get_plugin_contents(&self, _: ClipboardProvider, _: ClipboardType) -> HostFuture<String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok("owned clipboard".into()) })
    }
    fn set_plugin_contents(
        &self,
        _: ClipboardProvider,
        _: String,
        _: ClipboardType,
    ) -> HostFuture<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clipboard_custom_provider_requires_the_exact_executed_argv_before_backend_dispatch(
) -> anyhow::Result<()> {
    let _compilation = crate::support::plugin_guest::compilation_permit().await;
    let mut fixture = Fixture::new("original\n")?;
    fixture.configure(|config| {
        config.clipboard_provider = serde_json::from_value(serde_json::json!({"custom":{
            "yank":{"command":"actual-read","args":["safe"]},
            "paste":{"command":"actual-write","args":[]},
            "yank-primary":{"command":"other-read","args":[]}
        }}))
        .unwrap()
    });
    let calls = Arc::new(AtomicUsize::new(0));
    fixture
        .editor
        .registers
        .set_clipboard_backend(Box::new(ClipboardProbe(calls.clone())));
    let dir = tempfile::tempdir()?;
    let mut config = package(
        dir.path(),
        vec![error(
            EditorRequest::ReadRegister {
                name: '*',
                origin: None,
            },
            ErrorCode::PermissionDenied,
        )],
        &[Capability::Clipboard, Capability::Process],
    )?;
    config.permissions.processes = vec![plugin_api::ProcessGrant {
        command: "other-read".into(),
        args: vec![],
    }];
    load(
        &mut fixture,
        dir.path(),
        BTreeMap::from([("fixture".into(), config.clone())]),
    )
    .await;
    run(&mut fixture, "fixture.run").await?;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    config.permissions.processes = vec![plugin_api::ProcessGrant {
        command: "actual-read".into(),
        args: vec!["safe".into()],
    }];
    config.config["routes"][0]["requests"] = serde_json::json!([service(
        EditorRequest::ReadRegister {
            name: '*',
            origin: None
        },
        &["owned clipboard"]
    )]);
    load(
        &mut fixture,
        dir.path(),
        BTreeMap::from([("fixture".into(), config)]),
    )
    .await;
    run(&mut fixture, "fixture.run").await?;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn editing_builtins_reject_readonly_and_binary_documents_before_any_response_effect(
) -> anyhow::Result<()> {
    use plugin_api::ui::{BuiltinCommand, BuiltinInvocation, UiOrigin};
    let _compilation = crate::support::plugin_guest::compilation_permit().await;
    for binary in [false, true] {
        let mut fixture = Fixture::new("original\n")?;
        if binary {
            let path = fixture.dir.path().join("binary.bin");
            std::fs::write(&path, [0, 1, 0, 2, 0, 3])?;
            fixture.editor.open(&path, Action::Replace)?;
            assert!(current_ref!(fixture.editor).1.is_binary());
            // The explicit binary guard must stand even if a host toggles
            // the ordinary readonly flag independently.
            current!(fixture.editor).1.readonly = false;
        } else {
            current!(fixture.editor).1.readonly = true;
        }
        let origin = target(&fixture);
        let before = current_ref!(fixture.editor).1.text().to_string();
        let dir = tempfile::tempdir()?;
        let mut config = package(dir.path(), vec![], &[])?;
        config.config["routes"][0]["response"] = serde_json::to_value(plugin_api::Response {
            actions: vec![
                plugin_api::Action::Status {
                    message: "earlier status must not apply".into(),
                },
                plugin_api::Action::InvokeBuiltin {
                    request: 1,
                    origin: UiOrigin {
                        view: origin.view,
                        document: origin.document,
                        binding_revision: origin.binding_revision,
                        version: origin.version,
                        selection_revision: origin.selection_revision,
                    },
                    commands: vec![BuiltinInvocation {
                        command: BuiltinCommand::DeleteSelectionNoYank,
                        count: None,
                    }],
                },
            ],
            error: None,
        })?;
        load(
            &mut fixture,
            dir.path(),
            BTreeMap::from([("fixture".into(), config)]),
        )
        .await;
        assert!(run(&mut fixture, "fixture.run").await.is_err());
        assert!(fixture
            .editor
            .get_status()
            .unwrap()
            .0
            .contains("readonly or binary"));
        assert!(fixture.editor.take_plugin_builtin_requests().is_empty());
        assert_eq!(current_ref!(fixture.editor).1.text().to_string(), before);
    }
    Ok(())
}
