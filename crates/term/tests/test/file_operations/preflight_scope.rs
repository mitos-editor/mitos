use lsp_client::Url;
use serde_json::json;
use view::editor::Action;

use super::fixture::Fixture;

#[tokio::test(flavor = "multi_thread")]
async fn unrelated_buffer_edits_and_lifecycle_changes_allow_preflight() -> anyhow::Result<()> {
    for change in ["edit", "close", "rename", "open"] {
        let mut fixture = Fixture::new().await?;
        let id = fixture.app.editor.open(&fixture.dependent, Action::Load)?;
        let uri = Url::from_file_path(&fixture.source).unwrap();
        fixture
            .request_rename(json!({"changes": {uri.as_str(): [{"range": {
            "start":{"line":0,"character":0}, "end":{"line":0,"character":0}
        }, "newText":"prepared "}]}}))
            .await?;
        if change == "open" {
            let path = fixture.dir.path().join("unrelated.txt");
            std::fs::write(&path, "unrelated\n")?;
            fixture.app.editor.open(&path, Action::Load)?;
        } else {
            fixture.change(id, change)?;
        }
        fixture.complete_rename(true).await?;
        let doc = fixture
            .app
            .editor
            .document_by_path(stdx::path::canonicalize(&fixture.target))
            .unwrap();
        assert_eq!(doc.text(), "prepared source\n");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn returned_text_edits_require_unchanged_document_identities_and_contents(
) -> anyhow::Result<()> {
    for form in ["changes", "documentChanges", "operations"] {
        for change in ["edit", "close", "rename", "reopen", "open"] {
            let mut fixture = Fixture::new().await?;
            let id = if change == "open" {
                None
            } else {
                Some(fixture.app.editor.open(&fixture.dependent, Action::Load)?)
            };
            let uri = Url::from_file_path(&fixture.dependent).unwrap();
            let edits = json!([{"range": {"start":{"line":0,"character":0},
                "end":{"line":0,"character":0}}, "newText":"prepared "}]);
            let auxiliary = fixture.dir.path().join("auxiliary.txt");
            let document_edit = json!({"textDocument":{"uri":uri,"version":null},"edits":edits});
            let edit = match form {
                "changes" => json!({"changes":{uri.as_str():edits}}),
                "documentChanges" => json!({"documentChanges":[document_edit]}),
                "operations" => json!({"documentChanges":[
                    {"kind":"create","uri":Url::from_file_path(&auxiliary).unwrap()},
                    document_edit
                ]}),
                _ => unreachable!(),
            };
            fixture.request_rename(edit).await?;
            if let Some(id) = id {
                fixture.change(id, change)?;
            } else {
                fixture.app.editor.open(&fixture.dependent, Action::Load)?;
            }
            fixture.complete_rename(false).await?;
            assert!(!auxiliary.exists());
            assert_eq!(std::fs::read_to_string(&fixture.dependent)?, "dependent\n");
            if change == "edit" {
                assert_eq!(
                    fixture.app.editor.document(id.unwrap()).unwrap().text(),
                    "typed dependent\n"
                );
            }
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn returned_directory_operations_protect_child_documents() -> anyhow::Result<()> {
    for kind in ["delete", "rename"] {
        let mut fixture = Fixture::new().await?;
        let directory = fixture.dir.path().join("directory");
        std::fs::create_dir(&directory)?;
        let child = directory.join("child.txt");
        std::fs::write(&child, "child\n")?;
        let id = fixture.app.editor.open(&child, Action::Load)?;
        let uri = Url::from_file_path(&directory).unwrap();
        let moved = fixture.dir.path().join("moved");
        fixture
            .request_rename(json!({"documentChanges":[{"kind":kind,
            "uri":uri,"oldUri":uri,"newUri":Url::from_file_path(&moved).unwrap(),
            "options":{"recursive":true}}]}))
            .await?;
        fixture.change(id, "edit")?;
        fixture.complete_rename(false).await?;
        assert_eq!(std::fs::read_to_string(&child)?, "child\n");
        assert!(!moved.exists());
    }
    Ok(())
}
