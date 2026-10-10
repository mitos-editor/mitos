use std::{cell::RefCell, collections::BTreeMap, path::PathBuf};

use plugin_sdk::{
    component::{self, Job},
    editor::{EditorRequest, OpenDisposition, ViewTarget},
    export_plugin,
    ui::{UiKind, UiOpenAction, UiOrigin, UiOutcome, UiResponse, UiRow, UiValue},
    Action, Event, JobOutput, JobPoll, JobRequest, Request, Response, TextEdit,
};

const MAX_RECENT: usize = 32;
const MAX_RESULTS: usize = 256;

#[derive(Default)]
struct State {
    recent: Vec<String>,
    next_request: u64,
    dialogs: BTreeMap<u64, Dialog>,
    pending: Option<Pending>,
}
enum Dialog {
    Recent {
        paths: Vec<String>,
        origin: Option<UiOrigin>,
    },
    SearchPrompt {
        root_path: String,
        origin: Option<UiOrigin>,
    },
    SearchResults {
        paths: Vec<(String, u64, u64)>,
        origin: Option<UiOrigin>,
    },
}
enum Pending {
    Search {
        job: Job,
        root_path: String,
        origin: Option<UiOrigin>,
        rows: Vec<UiRow>,
        paths: Vec<(String, u64, u64)>,
        truncated: bool,
    },
    Format {
        job: Job,
        document: u64,
        version: i32,
        char_count: usize,
    },
}
impl Pending {
    fn job(&self) -> &Job {
        match self {
            Self::Search { job, .. } | Self::Format { job, .. } => job,
        }
    }
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }

fn origin(request: &Request) -> Option<UiOrigin> {
    let doc = request.editor.document.as_ref()?;
    let view = request.editor.view.as_ref()?;
    Some(UiOrigin {
        view: view.id,
        document: doc.id,
        binding_revision: view.binding_revision,
        version: doc.version,
        selection_revision: view.selection_revision,
    })
}
fn status(message: impl Into<String>) -> Response {
    Response {
        actions: vec![Action::Status {
            message: message.into(),
        }],
        error: None,
    }
}
fn fail(message: impl Into<String>) -> Response {
    Response {
        error: Some(message.into()),
        ..Response::default()
    }
}
fn show(state: &mut State, dialog: Dialog, kind: UiKind, origin: Option<UiOrigin>) -> Response {
    if state.dialogs.len() >= 8 {
        return fail("Too many outstanding workflow dialogs");
    }
    state.next_request = state.next_request.wrapping_add(1);
    let request = state.next_request;
    state.dialogs.insert(request, dialog);
    Response {
        actions: vec![Action::ShowUi {
            request,
            origin,
            kind,
        }],
        error: None,
    }
}
fn clipped(text: &str, chars: usize) -> String {
    text.chars().take(chars).collect()
}
fn navigate(
    path: String,
    line: u64,
    column: u64,
    action: UiOpenAction,
    origin: Option<UiOrigin>,
) -> Response {
    let action = match action {
        UiOpenAction::Replace => OpenDisposition::Replace,
        UiOpenAction::HorizontalSplit => OpenDisposition::HorizontalSplit,
        UiOpenAction::VerticalSplit => OpenDisposition::VerticalSplit,
    };
    let origin = origin.map(|origin| ViewTarget {
        view: origin.view,
        document: origin.document,
        binding_revision: origin.binding_revision,
        version: origin.version,
        selection_revision: origin.selection_revision,
    });
    match component::editor_request(EditorRequest::OpenAt {
        origin,
        path,
        line,
        column,
        action,
    }) {
        Ok(_) => Response::default(),
        Err(error) => fail(error.message),
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn handle(request: Request) -> Response {
    STATE.with_borrow_mut(|state| match request.event {
        Event::Init => {
            match component::storage_read("recent-files/v1") {
                Ok(Some(value)) => {
                    if let Ok(paths) = serde_json::from_str::<Vec<String>>(&value) {
                        state.recent = paths
                            .into_iter()
                            .filter(|path| path.len() <= 1024)
                            .take(MAX_RECENT)
                            .collect();
                    }
                }
                Ok(None) => (),
                Err(error) => return fail(error.message),
            }
            Response::default()
        }
        Event::DocumentOpened => {
            if let Some(path) = request
                .editor
                .document
                .as_ref()
                .and_then(|doc| doc.path.as_ref())
            {
                if path.len() <= 1024 {
                    state.recent.retain(|previous| previous != path);
                    state.recent.insert(0, path.clone());
                    state.recent.truncate(MAX_RECENT);
                    if let Err(error) = component::storage_write(
                        "recent-files/v1",
                        &serde_json::to_string(&state.recent).unwrap(),
                    ) {
                        return fail(error.message);
                    }
                }
            }
            Response::default()
        }
        Event::Command => {
            let origin = origin(&request);
            match request.command.as_deref() {
                Some("recent") => {
                    let paths = state.recent.clone();
                    let rows = paths
                        .iter()
                        .enumerate()
                        .map(|(index, path)| UiRow {
                            id: index.to_string(),
                            label: path.clone(),
                            description: String::new(),
                            preview: None,
                            location: None,
                        })
                        .collect();
                    show(
                        state,
                        Dialog::Recent { paths, origin },
                        UiKind::Picker {
                            title: "Recent files".into(),
                            rows,
                        },
                        origin,
                    )
                }
                Some("search") => {
                    let Some(root_path) = request
                        .config
                        .get("root-path")
                        .and_then(|value| value.as_str())
                        .filter(|path| !path.is_empty() && path.len() <= 4096)
                    else {
                        return fail("Set config.root-path to the first granted read root");
                    };
                    show(
                        state,
                        Dialog::SearchPrompt {
                            root_path: root_path.into(),
                            origin,
                        },
                        UiKind::Prompt {
                            title: "Search project (literal text)".into(),
                            initial: request.args.first().cloned().unwrap_or_default(),
                        },
                        origin,
                    )
                }
                Some("format") => {
                    if state.pending.is_some() {
                        return fail("Cancel the current workflow job first");
                    }
                    let Some(doc) = request.editor.document.as_ref() else {
                        return fail("No document to format");
                    };
                    if doc.byte_count > 1024 * 1024 {
                        return fail("Example formatter supports buffers up to 1 MiB");
                    }
                    let text =
                        match component::read_document(doc.id, doc.version, 0, doc.char_count) {
                            Ok(text) => text,
                            Err(error) => return fail(error.message),
                        };
                    let tool = request
                        .config
                        .get("formatter")
                        .and_then(|value| value.as_str())
                        .unwrap_or("rustfmt");
                    let job = match component::start_job(JobRequest::Process {
                        command: tool.into(),
                        args: vec![
                            "--emit".into(),
                            "stdout".into(),
                            "--edition".into(),
                            "2024".into(),
                        ],
                        input: text,
                        root: 0,
                        timeout_milliseconds: JobRequest::DEFAULT_PROCESS_TIMEOUT_MILLISECONDS,
                    }) {
                        Ok(job) => job,
                        Err(error) => return fail(error.message),
                    };
                    state.pending = Some(Pending::Format {
                        job,
                        document: doc.id,
                        version: doc.version,
                        char_count: doc.char_count as usize,
                    });
                    status("Formatting started; wait for completion before saving")
                }
                Some("cancel") => {
                    if let Some(pending) = state.pending.take() {
                        if let Err(error) = pending.job().cancel() {
                            return fail(error.message);
                        }
                    }
                    status("Workflow job cancelled")
                }
                _ => fail("Unknown workflow command"),
            }
        }
        Event::UiResult => {
            let data = request.data.get("response").cloned().unwrap_or_default();
            let response: UiResponse = match serde_json::from_value(data) {
                Ok(response) => response,
                Err(error) => return fail(format!("Invalid UI result: {error}")),
            };
            let Some(dialog) = state.dialogs.remove(&response.identity.request) else {
                return Response::default();
            };
            let value = match response.outcome {
                UiOutcome::Accepted { value } => value,
                UiOutcome::Cancelled { .. } => return status("Workflow dialog cancelled"),
                UiOutcome::Failed { error } => return fail(error.message),
            };
            match (dialog, value) {
                (Dialog::Recent { paths, origin }, UiValue::Picker { row, action }) => row
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| paths.get(index).cloned())
                    .map_or_else(
                        || fail("Unknown recent file"),
                        |path| navigate(path, 0, 0, action, origin),
                    ),
                (Dialog::SearchPrompt { root_path, origin }, UiValue::Prompt { text }) => {
                    if state.pending.is_some() {
                        return fail("Cancel the current workflow job first");
                    }
                    let job = match component::start_job(JobRequest::Search {
                        root: 0,
                        query: text,
                    }) {
                        Ok(job) => job,
                        Err(error) => return fail(error.message),
                    };
                    state.pending = Some(Pending::Search {
                        job,
                        root_path,
                        origin,
                        rows: vec![],
                        paths: vec![],
                        truncated: false,
                    });
                    status("Search started; :workflows.cancel stops it")
                }
                (Dialog::SearchResults { paths, origin }, UiValue::Picker { row, action }) => row
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| paths.get(index).cloned())
                    .map_or_else(
                        || fail("Unknown search result"),
                        |(path, line, column)| navigate(path, line, column, action, origin),
                    ),
                _ => fail("UI result does not match its workflow dialog"),
            }
        }
        Event::JobReady => {
            let Some(mut pending) = state.pending.take() else {
                return Response::default();
            };
            let id = match pending.job().id() {
                Ok(id) => id,
                Err(error) => return fail(error.message),
            };
            if request.data.get("job").and_then(|value| value.as_u64()) != Some(id) {
                state.pending = Some(pending);
                return Response::default();
            }
            // The native queue has at most eight chunks. Drain it until pending;
            // readiness is coalesced, so one event can describe several chunks.
            for _ in 0..16 {
                let output = match pending.job().poll() {
                    Ok(output) => output,
                    Err(error) => return fail(error.message),
                };
                match output {
                    JobPoll::Pending => {
                        state.pending = Some(pending);
                        return Response::default();
                    }
                    JobPoll::Failed { error } => return fail(error.message),
                    JobPoll::Finished => {
                        return match pending {
                            Pending::Search {
                                root_path: _,
                                origin,
                                rows,
                                paths,
                                truncated,
                                ..
                            } => show(
                                state,
                                Dialog::SearchResults { paths, origin },
                                UiKind::Picker {
                                    title: if truncated {
                                        "Project search (results limited)"
                                    } else {
                                        "Project search"
                                    }
                                    .into(),
                                    rows,
                                },
                                origin,
                            ),
                            Pending::Format { .. } => fail("Formatter ended without output"),
                        };
                    }
                    JobPoll::Ready { output } => match (&mut pending, output) {
                        (
                            Pending::Search {
                                root_path,
                                rows,
                                paths,
                                truncated,
                                ..
                            },
                            JobOutput::Search {
                                matches,
                                truncated: limited,
                            },
                        ) => {
                            *truncated |= limited;
                            for found in matches {
                                if rows.len() == MAX_RESULTS {
                                    *truncated = true;
                                    continue;
                                }
                                rows.push(UiRow {
                                    id: rows.len().to_string(),
                                    label: clipped(&found.path, 256),
                                    description: format!("{}:{}", found.line + 1, found.column + 1),
                                    preview: Some(clipped(&found.text, 512)),
                                    location: None,
                                });
                                paths.push((
                                    PathBuf::from(&*root_path)
                                        .join(found.path)
                                        .to_string_lossy()
                                        .into_owned(),
                                    found.line,
                                    found.column,
                                ));
                            }
                        }
                        (
                            Pending::Format {
                                document,
                                version,
                                char_count,
                                ..
                            },
                            JobOutput::Process {
                                status: code,
                                stdout,
                                stderr,
                            },
                        ) => {
                            if code != 0 {
                                return fail(format!(
                                    "Formatter exited {code}: {}",
                                    clipped(&stderr, 512)
                                ));
                            }
                            return Response {
                                actions: vec![
                                    Action::Edit {
                                        document: *document,
                                        version: *version,
                                        edits: vec![TextEdit {
                                            start: 0,
                                            end: *char_count,
                                            text: stdout,
                                        }],
                                    },
                                    Action::Status {
                                        message: "Formatting completed; save when ready".into(),
                                    },
                                ],
                                error: None,
                            };
                        }
                        _ => return fail("Unexpected workflow job output"),
                    },
                }
            }
            state.pending = Some(pending);
            Response::default()
        }
        Event::Shutdown => {
            state.pending.take();
            Response::default()
        }
        _ => Response::default(),
    })
}
export_plugin!(handle);
