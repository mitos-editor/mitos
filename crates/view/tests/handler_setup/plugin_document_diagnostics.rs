//! Actual SDK guests publish versioned annotations into native documents.
use crate::support::plugin_host::{drain, load};
use std::{collections::BTreeMap, path::Path};

use editor_core::{
    diagnostic::{Diagnostic, DiagnosticProvider, Severity},
    Transaction,
};
use plugin_api::{
    editor::{
        DiagnosticSeverity, DocumentDiagnostic, DocumentTarget, EditorRequest, TextRange,
        MAX_DOCUMENT_DIAGNOSTICS, MAX_DOCUMENT_DIAGNOSTIC_MESSAGE_BYTES,
    },
    Capability, ErrorCode, StateQuery,
};
use plugins::PluginConfig;
use view::{current, current_ref};

use super::Fixture;

fn target(fixture: &Fixture) -> DocumentTarget {
    let doc = current_ref!(fixture.editor).1;
    DocumentTarget {
        document: doc.id().as_u64(),
        version: doc.version(),
    }
}
fn annotation(start: u64, end: u64, message: &str) -> DocumentDiagnostic {
    DocumentDiagnostic {
        range: TextRange { start, end },
        severity: DiagnosticSeverity::Error,
        message: message.into(),
    }
}
fn publish(target: DocumentTarget, diagnostics: Vec<DocumentDiagnostic>) -> EditorRequest {
    EditorRequest::PublishDiagnostics {
        target,
        diagnostics,
    }
}
fn service(request: EditorRequest) -> serde_json::Value {
    serde_json::json!({"request":request})
}
fn failure(request: EditorRequest, code: ErrorCode) -> serde_json::Value {
    serde_json::json!({"request":request,"error_code":code})
}
fn route(command: &str, requests: Vec<serde_json::Value>) -> serde_json::Value {
    serde_json::json!({"event":"command","filter":format!("\"args\":[\"{command}\"]"),"requests":requests})
}
fn package(dir: &Path, routes: Vec<serde_json::Value>) -> anyhow::Result<PluginConfig> {
    std::fs::create_dir_all(dir)?;
    let mut config = crate::support::plugin_guest::observing(dir, &[], &[], None)?;
    config.config = serde_json::json!({"routes":routes});
    config.permissions.capabilities = [Capability::EditorRead, Capability::Ui].into();
    config.permissions.read_roots.clear();
    Ok(config)
}
async fn run(fixture: &mut Fixture, plugin: &str, command: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        crate::support::plugin_host::execute(
            fixture,
            &format!("{plugin}.run"),
            vec![command.into()]
        )
        .await?,
        "plugin command unavailable"
    );
    Ok(())
}
fn diagnostics(fixture: &Fixture, target: DocumentTarget) -> &[Diagnostic] {
    fixture
        .editor
        .documents()
        .find(|doc| doc.id().as_u64() == target.document)
        .unwrap()
        .diagnostics()
}
fn baseline(provider: DiagnosticProvider, message: &str) -> Diagnostic {
    Diagnostic {
        range: editor_core::diagnostic::Range { start: 0, end: 1 },
        ends_at_word: false,
        starts_at_word: false,
        zero_width: false,
        line: 0,
        message: message.into(),
        severity: Some(Severity::Warning),
        code: None,
        provider,
        tags: Vec::new(),
        source: None,
        data: None,
    }
}
fn native_providers(fixture: &mut Fixture) {
    let doc = current!(fixture.editor).1;
    doc.replace_diagnostics(
        [
            baseline(
                DiagnosticProvider::Lsp {
                    server_id: Default::default(),
                    identifier: None,
                },
                "native LSP",
            ),
            baseline(DiagnosticProvider::Spelling, "native spelling"),
        ],
        &[],
        None,
    );
}
fn assert_native_providers(fixture: &Fixture, target: DocumentTarget) {
    for message in ["native LSP", "native spelling"] {
        assert!(diagnostics(fixture, target)
            .iter()
            .any(|diagnostic| diagnostic.message.as_ref() == message));
    }
}

fn plugin_diagnostics(fixture: &Fixture, target: DocumentTarget) -> Vec<&Diagnostic> {
    diagnostics(fixture, target)
        .iter()
        .filter(|diagnostic| matches!(diagnostic.provider, DiagnosticProvider::Plugin { .. }))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publication_uses_scalar_ranges_and_clear_preserves_other_providers() -> anyhow::Result<()>
{
    let _compilation = crate::support::plugin_guest::compilation_permit().await;
    let mut fixture = Fixture::new("é前\nlast\n")?;
    native_providers(&mut fixture);
    let document = target(&fixture);
    let dir = tempfile::tempdir()?;
    let doc = current_ref!(fixture.editor).1;
    let catalog = serde_json::to_string(&plugin_api::DocumentInfo {
        id: document.document,
        version: document.version,
        path: Some(doc.path().unwrap().to_string_lossy().into_owned()),
        language: doc.language_name().map(str::to_owned),
        readonly: false,
        binary: false,
        bytes: 11,
        chars: 8,
    })?;
    fixture.editor.new_file(view::editor::Action::Replace);
    let second = target(&fixture);
    let cases = [
        (1, 2, 0, "error", DiagnosticSeverity::Error, Severity::Error),
        (
            3,
            4,
            1,
            "warning",
            DiagnosticSeverity::Warning,
            Severity::Warning,
        ),
        (5, 6, 1, "info", DiagnosticSeverity::Info, Severity::Info),
        (8, 8, 2, "hint", DiagnosticSeverity::Hint, Severity::Hint),
    ];
    let published = cases
        .iter()
        .map(
            |&(start, end, _, message, severity, _)| DocumentDiagnostic {
                range: TextRange { start, end },
                severity,
                message: message.into(),
            },
        )
        .collect();
    let alpha = package(
        &dir.path().join("alpha"),
        vec![
            route(
                "publish",
                vec![
                    serde_json::json!({"request":EditorRequest::ReadState { query:StateQuery::default() },
                "expected":[catalog,"\"kind\":\"state\""]}),
                    service(publish(document, published)),
                    service(publish(second, vec![annotation(0, 0, "second document")])),
                ],
            ),
            route(
                "clear",
                vec![service(EditorRequest::ClearDiagnostics {
                    target: Some(document),
                })],
            ),
            route(
                "clear-all",
                vec![service(EditorRequest::ClearDiagnostics { target: None })],
            ),
        ],
    )?;
    let beta = package(
        &dir.path().join("beta"),
        vec![route(
            "publish",
            vec![service(publish(document, vec![annotation(0, 1, "beta")]))],
        )],
    )?;
    load(
        &mut fixture,
        dir.path(),
        &BTreeMap::from([("alpha".into(), alpha), ("beta".into(), beta)]),
    )
    .await;
    run(&mut fixture, "alpha", "publish").await?;
    let generation = fixture
        .editor
        .plugin_diagnostics()
        .into_iter()
        .find(|diagnostics| diagnostics.plugin == "alpha")
        .unwrap()
        .generation;
    for &(start, end, line, message, _, severity) in &cases {
        let native = diagnostics(&fixture, document)
            .iter()
            .find(|diagnostic| diagnostic.message.as_ref() == message)
            .unwrap();
        assert_eq!(
            (native.range.start, native.range.end),
            (start as usize, end as usize)
        );
        assert_eq!(native.line, line);
        assert_eq!(native.zero_width, start == end);
        assert_eq!(native.severity, Some(severity));
        // The host supplies the owner; the guest request carries no provider identity.
        assert!(matches!(native.provider, DiagnosticProvider::Plugin { .. }));
    }
    assert!(generation > 0);
    assert_native_providers(&fixture, document);
    assert_eq!(plugin_diagnostics(&fixture, second).len(), 1);
    run(&mut fixture, "beta", "publish").await?;
    assert_eq!(plugin_diagnostics(&fixture, document).len(), 5);
    let alpha_provider = &diagnostics(&fixture, document)
        .iter()
        .find(|diagnostic| diagnostic.message.as_ref() == "error")
        .unwrap()
        .provider;
    let beta_provider = &diagnostics(&fixture, document)
        .iter()
        .find(|diagnostic| diagnostic.message.as_ref() == "beta")
        .unwrap()
        .provider;
    assert_ne!(alpha_provider, beta_provider);
    run(&mut fixture, "alpha", "clear").await?;
    assert_eq!(
        plugin_diagnostics(&fixture, document)
            .iter()
            .map(|diagnostic| diagnostic.message.as_ref())
            .collect::<Vec<_>>(),
        ["beta"]
    );
    assert_native_providers(&fixture, document);
    assert_eq!(plugin_diagnostics(&fixture, second).len(), 1);
    run(&mut fixture, "alpha", "publish").await?;
    run(&mut fixture, "alpha", "clear-all").await?;
    assert_eq!(plugin_diagnostics(&fixture, document).len(), 1);
    assert!(plugin_diagnostics(&fixture, second).is_empty());
    assert_native_providers(&fixture, document);
    fixture.editor.shutdown_plugins();
    fixture.editor.finish_plugin_shutdown().await;
    assert!(plugin_diagnostics(&fixture, document).is_empty());
    assert_native_providers(&fixture, document);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_publication_is_bounded_and_does_not_replace_existing_annotations(
) -> anyhow::Result<()> {
    let _compilation = crate::support::plugin_guest::compilation_permit().await;
    let mut fixture = Fixture::new("é前\nlast\n")?;
    let document = target(&fixture);
    let dir = tempfile::tempdir()?;
    let mut overlong = annotation(0, 1, "unused");
    overlong.message = "μ".repeat(MAX_DOCUMENT_DIAGNOSTIC_MESSAGE_BYTES / 2 + 1);
    let wrong_version = DocumentTarget {
        version: document.version + 1,
        ..document
    };
    let config = package(
        dir.path(),
        vec![route(
            "check",
            vec![
                service(publish(
                    document,
                    vec![annotation(0, 1, "retained"); MAX_DOCUMENT_DIAGNOSTICS],
                )),
                failure(
                    publish(
                        document,
                        vec![annotation(0, 1, "x"); MAX_DOCUMENT_DIAGNOSTICS + 1],
                    ),
                    ErrorCode::ResourceExhausted,
                ),
                failure(
                    publish(document, vec![overlong]),
                    ErrorCode::ResourceExhausted,
                ),
                failure(
                    publish(document, vec![annotation(2, 1, "reversed")]),
                    ErrorCode::InvalidRequest,
                ),
                failure(
                    publish(document, vec![annotation(0, 9, "outside document")]),
                    ErrorCode::InvalidRequest,
                ),
                failure(
                    publish(document, vec![annotation(0, 1, "\u{1b}\u{7}")]),
                    ErrorCode::InvalidRequest,
                ),
                failure(
                    publish(document, vec![annotation(u64::MAX, u64::MAX, "overflow")]),
                    ErrorCode::InvalidRequest,
                ),
                failure(
                    publish(wrong_version, vec![annotation(0, 1, "stale")]),
                    ErrorCode::StaleState,
                ),
                failure(
                    EditorRequest::ClearDiagnostics {
                        target: Some(wrong_version),
                    },
                    ErrorCode::StaleState,
                ),
            ],
        )],
    )?;
    load(
        &mut fixture,
        dir.path(),
        &BTreeMap::from([("fixture".into(), config)]),
    )
    .await;
    run(&mut fixture, "fixture", "check").await?;
    let annotations = plugin_diagnostics(&fixture, document);
    assert_eq!(annotations.len(), MAX_DOCUMENT_DIAGNOSTICS);
    assert!(annotations
        .iter()
        .all(|diagnostic| diagnostic.message.as_ref() == "retained"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_annotations_expire_on_edit_reload_trap_and_permission_removal() -> anyhow::Result<()>
{
    let _compilation = crate::support::plugin_guest::compilation_permit().await;
    let mut fixture = Fixture::new("é前\nlast\n")?;
    native_providers(&mut fixture);
    let original = target(&fixture);
    let dir = tempfile::tempdir()?;
    let config = package(
        dir.path(),
        vec![
            route(
                "publish",
                vec![service(publish(original, vec![annotation(1, 2, "owned")]))],
            ),
            route(
                "stale",
                vec![failure(
                    publish(original, vec![annotation(1, 2, "stale")]),
                    ErrorCode::StaleState,
                )],
            ),
        ],
    )?;
    load(
        &mut fixture,
        dir.path(),
        &BTreeMap::from([("fixture".into(), config)]),
    )
    .await;
    run(&mut fixture, "fixture", "publish").await?;
    assert_eq!(plugin_diagnostics(&fixture, original).len(), 1);
    let (view, doc) = current!(fixture.editor);
    let change = Transaction::insert(doc.text(), doc.selection(view.id), "new".into());
    assert!(doc.apply(&change, view.id));
    let revised = target(&fixture);
    assert!(plugin_diagnostics(&fixture, revised).is_empty());
    assert_native_providers(&fixture, revised);
    run(&mut fixture, "fixture", "stale").await?;
    assert!(plugin_diagnostics(&fixture, revised).is_empty());

    let trap =
        serde_json::json!({"event":"command","filter":"\"args\":[\"trap\"]","operation":"trap"});
    let config = package(
        dir.path(),
        vec![
            route(
                "publish",
                vec![service(publish(revised, vec![annotation(4, 5, "owned")]))],
            ),
            trap,
        ],
    )?;
    load(
        &mut fixture,
        dir.path(),
        &BTreeMap::from([("fixture".into(), config.clone())]),
    )
    .await;
    run(&mut fixture, "fixture", "publish").await?;
    assert_eq!(plugin_diagnostics(&fixture, revised).len(), 1);
    load(
        &mut fixture,
        dir.path(),
        &BTreeMap::from([("fixture".into(), config.clone())]),
    )
    .await;
    assert!(plugin_diagnostics(&fixture, revised).is_empty());
    assert_native_providers(&fixture, revised);
    run(&mut fixture, "fixture", "publish").await?;
    assert!(run(&mut fixture, "fixture", "trap").await.is_err());
    assert!(plugin_diagnostics(&fixture, revised).is_empty());
    assert_native_providers(&fixture, revised);

    load(
        &mut fixture,
        dir.path(),
        &BTreeMap::from([("fixture".into(), config)]),
    )
    .await;
    run(&mut fixture, "fixture", "publish").await?;
    assert_eq!(plugin_diagnostics(&fixture, revised).len(), 1);
    let mut revoked = package(
        dir.path(),
        vec![route(
            "denied",
            vec![
                failure(
                    publish(revised, vec![annotation(4, 5, "denied")]),
                    ErrorCode::PermissionDenied,
                ),
                failure(
                    EditorRequest::ClearDiagnostics { target: None },
                    ErrorCode::PermissionDenied,
                ),
                service(EditorRequest::ReadState {
                    query: StateQuery::default(),
                }),
            ],
        )],
    )?;
    revoked.permissions.capabilities.remove(&Capability::Ui);
    load(
        &mut fixture,
        dir.path(),
        &BTreeMap::from([("fixture".into(), revoked)]),
    )
    .await;
    assert!(plugin_diagnostics(&fixture, revised).is_empty());
    assert_native_providers(&fixture, revised);
    run(&mut fixture, "fixture", "denied").await?;
    assert!(plugin_diagnostics(&fixture, revised).is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn language_refresh_and_failed_replacement_preserve_owned_annotations() -> anyhow::Result<()>
{
    let _compilation = crate::support::plugin_guest::compilation_permit().await;
    let mut fixture = Fixture::new("original\n")?;
    native_providers(&mut fixture);
    let document = target(&fixture);
    let dir = tempfile::tempdir()?;
    let config = package(
        dir.path(),
        vec![route(
            "publish",
            vec![service(publish(document, vec![annotation(0, 1, "owned")]))],
        )],
    )?;
    let packages = BTreeMap::from([("fixture".into(), config)]);
    load(&mut fixture, dir.path(), &packages).await;
    run(&mut fixture, "fixture", "publish").await?;

    let languages = editor_core::syntax::Loader::new(
        toml::from_str("language = []")?,
        loader::syntax::Resources::default(),
    )?;
    let theme = fixture.editor.theme.clone();
    fixture.editor.apply_language_config(languages, theme)?;
    let id = current_ref!(fixture.editor).1.id();
    assert_eq!(plugin_diagnostics(&fixture, document).len(), 1);
    assert!(diagnostics(&fixture, document)
        .iter()
        .any(|d| d.message.as_ref() == "native spelling"));
    assert!(!diagnostics(&fixture, document)
        .iter()
        .any(|d| matches!(d.provider, DiagnosticProvider::Lsp { .. })));

    // A language refresh explicitly rechecks spelling, but plugin annotations
    // remain valid while the document text and owner are unchanged.
    fixture.editor.refresh_doc_language(id);
    assert_eq!(plugin_diagnostics(&fixture, document).len(), 1);

    let missing = BTreeMap::from([(
        "missing".into(),
        PluginConfig {
            path: dir.path().join("missing.toml"),
            enabled: true,
            ..PluginConfig::default()
        },
    )]);
    assert!(fixture.editor.reload_plugins(&missing, dir.path()));
    drain(&mut fixture).await;
    assert_eq!(plugin_diagnostics(&fixture, document).len(), 1);
    run(&mut fixture, "fixture", "publish").await?;

    load(&mut fixture, dir.path(), &packages).await;
    assert!(plugin_diagnostics(&fixture, document).is_empty());
    Ok(())
}
