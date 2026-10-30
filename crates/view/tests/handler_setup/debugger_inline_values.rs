use super::*;
use view::{current_ref, editor::Action, ViewId};

const SOURCE: &str = "fn earlier() { let answer = 0; }\nfn main() {\n    let answer = 42;\n    let text = \"answer\"; // answer\n    object.answer;\n    answer\n}\n";

async fn fixture(
    options: AdapterOptions,
) -> anyhow::Result<(Fixture, DebugAdapterId, AdapterWire, ViewId)> {
    let mut f = Fixture::with_languages(
        SOURCE,
        r#"
        [[language]]
        name = "rust"
        scope = "source.rust"
        roots = []
        file-types = ["words"]
    "#,
    )?;
    let (view, doc) = current_ref!(f.editor);
    let (view, doc, path) = (view.id, doc.id(), doc.path().unwrap().to_owned());
    f.editor.set_document_language(doc, Some("rust"))?;
    assert!(f.editor.document(doc).unwrap().syntax().is_some());
    let (id, wire) = paused_adapter_with_options(
        &mut f,
        AdapterOptions {
            stack_source: Some(path),
            stack_line: Some(6),
            ..options
        },
    )
    .await?;
    Ok((f, id, wire, view))
}

async fn labels(f: &mut Fixture, view: ViewId) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while f.editor.debugger_inline_values(view).is_empty() {
            f.callbacks
                .recv()
                .await
                .context("missing inline-value callback")?(&mut f.editor);
        }
        anyhow::Ok(())
    })
    .await?
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_values_refresh_when_injected_syntax_finishes() -> anyhow::Result<()> {
    for queued in [false, true] {
        let (sender, mut syntax_callbacks) = crate::support::callback_channel();
        let mut f = Fixture::with_handler_setup(
            "<script>answer;</script>\n",
            r#"
            [[language]]
            name = "html"
            scope = "text.html.basic"
            file-types = ["words"]

            [[language]]
            name = "javascript"
            scope = "source.js"
            file-types = ["js"]
            injection-regex = "^(js|javascript)$"
            "#,
            loader::syntax::Resources::default(),
            |_| {},
            move |handlers, _| {
                handlers.syntax = view::features::syntax::SyntaxHandler::new(sender);
            },
        )?;
        let first = tokio::time::timeout(Duration::from_secs(10), syntax_callbacks.recv())
            .await?
            .context("missing root syntax callback")?;
        first(&mut f.editor);
        let (view, doc) = current_ref!(f.editor);
        let (view, doc_id, path) = (view.id, doc.id(), doc.path().unwrap().to_owned());
        assert!(doc.is_syntax_pending() && doc.syntax().is_some());
        let complete = tokio::time::timeout(Duration::from_secs(10), syntax_callbacks.recv())
            .await?
            .context("missing injected syntax callback")?;
        let (_, mut wire) = paused_adapter_with_options(
            &mut f,
            AdapterOptions {
                stack_source: Some(path),
                stack_line: Some(1),
                ..Default::default()
            },
        )
        .await?;

        // The root tree cannot identify the JavaScript local yet. Exercise both
        // a cached empty result and a result queued across syntax completion.
        f.editor.refresh_debugger_inline_values();
        command(&mut wire, "variables").await?;
        let callback = tokio::time::timeout(Duration::from_secs(10), f.callbacks.recv())
            .await?
            .context("missing root-only inline values")?;
        let callback = if queued {
            Some(callback)
        } else {
            callback(&mut f.editor);
            None
        };
        assert!(f.editor.debugger_inline_values(view).is_empty());

        complete(&mut f.editor);
        assert!(!f.editor.document(doc_id).unwrap().is_syntax_pending());
        if let Some(callback) = callback {
            callback(&mut f.editor);
        }
        f.editor.refresh_debugger_inline_values();
        labels(&mut f, view).await?;
        let values = f.editor.debugger_inline_values(view);
        assert_eq!(values.len(), 1);
        assert_eq!(
            (values[0].line, values[0].text.as_str()),
            (0, "answer = 42")
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_values_use_cheap_scopes_and_the_last_code_occurrence_in_the_selected_function(
) -> anyhow::Result<()> {
    let (mut f, id, mut wire, view) = fixture(AdapterOptions::default()).await?;
    f.editor.refresh_debugger_inline_values();
    labels(&mut f, view).await?;
    let values = f.editor.debugger_inline_values(view);
    assert_eq!(values.len(), 2);
    assert_eq!(
        (values[0].line, values[0].text.as_str()),
        (4, "object = Object")
    );
    assert_eq!(
        (values[1].line, values[1].text.as_str()),
        (5, "answer = 42")
    );
    command(&mut wire, "scopes").await?;
    let request = command(&mut wire, "variables").await?;
    assert_eq!(request["arguments"]["variablesReference"], 7);
    f.editor.refresh_debugger_inline_values();
    assert!(super::inspection::barrier(&mut f, &mut wire)
        .await?
        .is_empty());

    f.editor
        .handle_debugger_message(
            id,
            debugger_event("invalidated", json!({"areas":["variables"]})),
        )
        .await;
    assert!(f.editor.debugger_inline_values(view).is_empty());
    f.editor.refresh_debugger_inline_values();
    labels(&mut f, view).await?;
    command(&mut wire, "scopes").await?;
    command(&mut wire, "variables").await?;
    let frames = f.editor.debugger_stack_frames()?;
    f.editor.select_debugger_frame(&frames.context, 1)?;
    assert!(f.editor.debugger_inline_values(view).is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn split_views_share_variable_data_and_refresh_independently() -> anyhow::Result<()> {
    let (mut f, _, mut wire, first) = fixture(AdapterOptions::default()).await?;
    let doc = current_ref!(f.editor).1.id();
    f.editor.switch(doc, Action::VerticalSplit);
    let second = current_ref!(f.editor).0.id;
    f.editor.refresh_debugger_inline_values();
    labels(&mut f, first).await?;
    labels(&mut f, second).await?;
    command(&mut wire, "scopes").await?;
    command(&mut wire, "variables").await?;
    assert!(super::inspection::barrier(&mut f, &mut wire)
        .await?
        .is_empty());
    f.configure(|config| config.debugger.inline_values = false);
    assert!(f.editor.debugger_inline_values(first).is_empty());
    assert!(f.editor.debugger_inline_values(second).is_empty());
    f.editor.refresh_debugger_inline_values();
    f.configure(|config| config.debugger.inline_values = true);
    f.editor.refresh_debugger_inline_values();
    labels(&mut f, first).await?;
    labels(&mut f, second).await?;
    assert!(super::inspection::barrier(&mut f, &mut wire)
        .await?
        .is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_values_prefer_the_innermost_scope_and_skip_scopes_outside_the_paused_location(
) -> anyhow::Result<()> {
    let (mut f, _, mut wire, view) = fixture(AdapterOptions {
        scopes: Some(json!({"scopes":[
            {"name":"outer","variablesReference":7,"expensive":false,"line":1,"endLine":10},
            {"name":"inner","variablesReference":9,"expensive":false,"line":5,"endLine":7},
            {"name":"outside","variablesReference":11,"expensive":false,"line":8,"endLine":10}
        ]})),
        variable_overrides: [(7, "outer"), (9, "inner"), (11, "outside")]
            .map(|(reference, value)| {
                (
                    reference,
                    json!({"variables":[{"name":"answer","value":value,"variablesReference":0}]}),
                )
            })
            .into_iter()
            .collect(),
        ..Default::default()
    })
    .await?;
    f.editor.refresh_debugger_inline_values();
    labels(&mut f, view).await?;
    assert_eq!(
        f.editor.debugger_inline_values(view)[0].text,
        "answer = inner"
    );
    command(&mut wire, "scopes").await?;
    let requests = super::inspection::barrier(&mut f, &mut wire).await?;
    let references: Vec<_> = requests
        .iter()
        .map(|request| request["arguments"]["variablesReference"].as_u64().unwrap())
        .collect();
    assert_eq!(references, [9, 7]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_inline_values_cannot_survive_document_edits_or_view_rebinding() -> anyhow::Result<()>
{
    for rebind in [false, true] {
        let (mut f, _, mut wire, view) = fixture(AdapterOptions::default()).await?;
        f.editor.refresh_debugger_inline_values();
        command(&mut wire, "variables").await?;
        let callback = tokio::time::timeout(Duration::from_secs(10), f.callbacks.recv())
            .await?
            .context("missing inline-value callback")?;
        if rebind {
            f.editor.new_file(Action::Replace);
        } else {
            f.replace("fn main() { answer }");
        }
        callback(&mut f.editor);
        assert!(f.editor.debugger_inline_values(view).is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn resuming_cancels_pending_inline_variable_requests() -> anyhow::Result<()> {
    let (mut f, id, mut wire, view) = fixture(AdapterOptions {
        held_commands: vec!["scopes"],
        ..Default::default()
    })
    .await?;
    f.editor.refresh_debugger_inline_values();
    command(&mut wire, "scopes").await?;
    f.editor
        .handle_debugger_message(id, debugger_event("continued", json!({"threadId":1})))
        .await;
    f.editor.refresh_debugger_inline_values();
    assert!(f.editor.debugger_inline_values(view).is_empty());
    release_adapter_request(&wire).await?;
    assert!(super::inspection::barrier(&mut f, &mut wire)
        .await?
        .is_empty());
    Ok(())
}
