use plugin_sdk::{
    component, export_plugin,
    ui::{UiKind, UiRow},
    Action, Event, Request, Response, TextEdit,
};

fn handle(request: Request) -> Response {
    if request.event != Event::Command {
        return Response::default();
    }
    let actions = match request.command.as_deref() {
        Some("read-roots") => match component::read_roots() {
            Ok(roots) => roots
                .into_iter()
                .map(|root| Action::Status {
                    message: format!("{}|{}|{}", root.index, root.path, root.configured_path),
                })
                .collect(),
            Err(error) => {
                return Response {
                    error: Some(error.message),
                    ..Response::default()
                }
            }
        },
        Some("uppercase") => {
            let document = request.editor.document.unwrap();
            let text = match component::read_document(document.id, document.version, 0, 6) {
                Ok(text) => text,
                Err(error) => {
                    return Response {
                        error: Some(error.message),
                        ..Response::default()
                    }
                }
            };
            vec![Action::Edit {
                document: document.id,
                version: document.version,
                edits: vec![TextEdit {
                    start: 0,
                    end: 6,
                    text: text.to_uppercase(),
                }],
            }]
        }
        Some("ui") => vec![
            Action::ShowUi {
                request: 1,
                origin: None,
                kind: UiKind::Prompt {
                    title: "SDK prompt".into(),
                    initial: "Initial".into(),
                },
            },
            Action::ShowUi {
                request: 2,
                origin: None,
                kind: UiKind::Picker {
                    title: "SDK picker".into(),
                    rows: vec![UiRow {
                        id: "row".into(),
                        label: "Label".into(),
                        description: "Description".into(),
                        preview: Some("Preview".into()),
                        location: None,
                    }],
                },
            },
        ],
        _ => vec![Action::Status {
            message: "sdk-ready".into(),
        }],
    };
    Response {
        actions,
        error: None,
    }
}
export_plugin!(handle);
