//! Declarative package sources flow through native completion and diagnostics.

use std::time::Duration;

use super::helpers::{run_event_loop_until_idle, test_config, test_key_sequences, AppBuilder};
use view::{
    document::Mode,
    editor::EditorEvent,
    handlers::completion::{self, CompletionEvent, CompletionHandler},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn declarative_snippets_use_native_completion_and_inspection() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(
        dir.path().join("plugin.toml"),
        "abi-version = 3\n[[contributions.snippets]]\nlanguage = 'sample'\npath = 'snippets.toml'\n",
    )?;
    std::fs::write(
        dir.path().join("snippets.toml"),
        r#"[[snippets]]
prefix = 'obj'
body = '{"${1:key}": ${2:value}}$0'
description = 'Native object snippet'
"#,
    )?;
    let path = dir.path().join("document.sample");
    std::fs::write(&path, "obj\n")?;
    let mut config = test_config();
    config.editor.auto_completion = false;
    config.editor.path_completion = false;
    config.plugins.insert(
        "assets".into(),
        plugins::PluginConfig {
            path: dir.path().join("plugin.toml"),
            ..Default::default()
        },
    );
    let syntax = editor_core::syntax::Loader::new(
        toml::from_str(
            "[[language]]\nname = 'sample'\nscope = 'source.sample'\nfile-types = ['sample']\n",
        )?,
        loader::syntax::Resources::default(),
    )?;
    let (sender, mut callbacks) =
        super::helpers::callbacks::unbounded(|blocking, callback| (blocking, callback));
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_lang_loader(syntax)
        .with_file(path, None)
        .with_handler_setup(move |handlers, config| {
            handlers.completions = CompletionHandler::new(sender, config);
        })
        .build()?;
    tokio::time::timeout(Duration::from_secs(5), run_event_loop_until_idle(&mut app)).await?;
    assert_eq!(app.editor.plugin_snippets().len(), 1);
    assert!(matches!(
        app.editor.plugin_diagnostics()[0].engine,
        Some(plugin_api::diagnostics::PluginEngine::Declarative)
    ));
    app.editor.mode = Mode::Insert;
    let (doc, view) = {
        let (view, doc) = view::current!(app.editor);
        doc.set_selection(view.id, editor_core::Selection::point(3));
        (doc.id(), view.id)
    };
    app.editor
        .handlers()
        .completions
        .event(CompletionEvent::ManualTrigger {
            cursor: 3,
            doc,
            view,
        });
    tokio::time::timeout(Duration::from_secs(5), async {
        while app.editor.last_completion.is_none() {
            let (_, callback) = callbacks.recv().await.expect("completion channel open");
            callback(&mut app.editor);
            while let Some(update) = completion::next_update(&mut app.editor) {
                app.handle_editor_event(EditorEvent::Completion(update))
                    .await;
            }
        }
    })
    .await?;
    let inserted = |app: &term::application::Application| {
        let (view, doc) = view::current_ref!(app.editor);
        assert_eq!(doc.text().to_string(), "{\"key\": value}\n");
        let selected = doc.selection(view.id).primary();
        assert_eq!(
            doc.text().slice(selected.from()..selected.to()).to_string(),
            "key"
        );
    };
    let inspected = |app: &term::application::Application| {
        let report = view::doc!(app.editor).text().to_string();
        assert!(report.contains("assets — Ready, declarative, generation"));
        assert!(report.contains("Queued: 0; completed: 0; failed: 0; cancelled: 0"));
    };
    test_key_sequences(
        &mut app,
        vec![
            (Some("<C-n><ret>"), Some(&inserted)),
            (Some("<esc>:plugin-inspect assets<ret>"), Some(&inspected)),
        ],
        false,
    )
    .await?;
    assert_eq!(app.editor.error_revision(), 0);
    Ok(())
}
