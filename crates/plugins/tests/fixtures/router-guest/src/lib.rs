//! A public-SDK guest driven by host fixture configuration, never host mocks.
use std::cell::RefCell;

use plugin_sdk::editor::{EditorReply, EditorRequest, OpenDisposition, ViewTarget};
use plugin_sdk::{component, export_plugin, Action, Event, Request, Response, TextEdit};
use serde::Deserialize;

#[derive(Deserialize, Default)]
struct Config {
    #[serde(default)]
    routes: Vec<Route>,
    before_shutdown: Option<String>,
    #[serde(default)]
    require_initialized: bool,
}
#[derive(Deserialize)]
struct Route {
    event: String,
    filter: Option<String>,
    #[serde(default)]
    expected: Vec<String>,
    #[serde(default)]
    response: Response,
    #[serde(default)]
    once: bool,
    #[serde(default)]
    reads: Vec<Read>,
    #[serde(default)]
    requests: Vec<ServiceCase>,
    operation: Option<String>,
}
#[derive(Deserialize)]
struct ServiceCase {
    request: EditorRequest,
    #[serde(default)]
    expected: Vec<String>,
    error_code: Option<plugin_sdk::ErrorCode>,
}
#[derive(Deserialize)]
struct Read {
    start: u64,
    end: u64,
    expected: Option<String>,
    error_code: Option<plugin_sdk::ErrorCode>,
}
#[derive(Default)]
struct State {
    matches: Vec<usize>,
    seen_before_shutdown: bool,
    initialized: bool,
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }

fn failure(message: &str) -> Response {
    Response {
        error: Some(message.into()),
        ..Response::default()
    }
}
fn handle(mut request: Request) -> Response {
    let config: Config = match serde_json::from_value(std::mem::take(&mut request.config)) {
        Ok(config) => config,
        Err(error) => return failure(&format!("invalid router config: {error}")),
    };
    // Expected fragments must not find themselves inside fixture configuration.
    let encoded = serde_json::to_string(&request).unwrap();
    let event = serde_json::to_value(request.event)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    STATE.with_borrow_mut(|state| {
        state.matches.resize(config.routes.len(), 0);
        if request.event == Event::Init {
            state.initialized = true;
        }
        if config.require_initialized && request.event == Event::Command && !state.initialized {
            return failure("plugin command arrived before initialization");
        }
        if request.event == Event::Shutdown
            && config.before_shutdown.is_some()
            && !state.seen_before_shutdown
        {
            return failure("guest event metadata or shutdown ordering mismatch");
        }
        for (index, route) in config.routes.iter().enumerate() {
            if route.event != event
                || route
                    .filter
                    .as_ref()
                    .is_some_and(|filter| !encoded.contains(filter))
            {
                continue;
            }
            if route
                .expected
                .iter()
                .any(|expected| !encoded.contains(expected))
            {
                return failure("unexpected event metadata");
            }
            if route.once && state.matches[index] != 0 {
                return failure("duplicate event");
            }
            state.matches[index] += 1;
            if config.before_shutdown.as_deref() == Some(&route.event) {
                state.seen_before_shutdown = true;
            }
            for read in &route.reads {
                let Some(doc) = request.editor.document.as_ref() else {
                    return failure("read requires document");
                };
                let result = component::read_document(doc.id, doc.version, read.start, read.end);
                if let Some(expected) = &read.expected {
                    if result.as_ref().ok() != Some(expected) {
                        return failure("region text mismatch");
                    }
                } else if let Some(code) = read.error_code {
                    if result.err().map(|error| error.code) != Some(code) {
                        return failure("region failure mismatch");
                    }
                } else if result.is_err() {
                    return failure("region read failed");
                }
            }
            for service in &route.requests {
                match component::editor_request(service.request.clone()) {
                    Ok(reply) if service.error_code.is_none() => {
                        let encoded = serde_json::to_string(&reply).unwrap();
                        if service
                            .expected
                            .iter()
                            .any(|fragment| !encoded.contains(fragment))
                        {
                            return failure("editor service reply mismatch");
                        }
                    }
                    Err(error) if Some(error.code) == service.error_code => (),
                    _ => return failure("editor service outcome mismatch"),
                }
            }
            match route.operation.as_deref() {
                Some("trap") => panic!("deliberate fixture trap"),
                Some("loop") => loop {
                    std::hint::spin_loop();
                },
                Some("large-actions") => {
                    return Response {
                        actions: (0..257)
                            .map(|_| Action::Status {
                                message: "too many".into(),
                            })
                            .collect(),
                        error: None,
                    }
                }
                Some("uppercase") => return uppercase(&request),
                Some("scratch") => return scratch(&request),
                _ => (),
            }
            return route.response.clone();
        }
        Response::default()
    })
}
fn scratch(request: &Request) -> Response {
    let origin = request.editor.view.as_ref().map(|view| ViewTarget {
        view: view.id,
        document: view.document,
        binding_revision: view.binding_revision,
        version: request.editor.document.as_ref().unwrap().version,
        selection_revision: view.selection_revision,
    });
    let result = component::editor_request(EditorRequest::Scratch {
        name: "Guest scratch".into(),
        text: "éß\n".into(),
        language: None,
        origin,
        action: OpenDisposition::Replace,
    });
    let target = match result {
        Ok(EditorReply::View { target }) => target,
        Err(error) => return failure(&error.message),
        _ => return failure("scratch did not return a view"),
    };
    if component::read_document(target.document, target.version, 0, 2).as_deref() != Ok("éß") {
        return failure("scratch read failed");
    }
    Response {
        actions: vec![
            Action::Edit {
                document: target.document,
                version: target.version,
                edits: vec![TextEdit {
                    start: 0,
                    end: 2,
                    text: "ÉSS".into(),
                }],
            },
            Action::Status {
                message: "scratch edited".into(),
            },
        ],
        error: None,
    }
}
fn uppercase(request: &Request) -> Response {
    let Some(doc) = request.editor.document.as_ref() else {
        return failure("uppercase needs document");
    };
    let Some(view) = request.editor.view.as_ref() else {
        return failure("uppercase needs view");
    };
    let mut ordered: Vec<_> = view.selections.iter().enumerate().collect();
    ordered.sort_by_key(|(_, range)| range.anchor.min(range.head));
    let mut ranges = view.selections.clone();
    let mut edits = Vec::new();
    let mut shift = 0_i64;
    let mut previous_end = 0;
    for (index, range) in ordered {
        let (start, end) = (range.anchor.min(range.head), range.anchor.max(range.head));
        if start < previous_end || end as u64 > doc.char_count {
            return failure("invalid selections");
        }
        let text = match component::read_document(doc.id, doc.version, start as u64, end as u64) {
            Ok(text) => text,
            Err(error) => return failure(&error.message),
        };
        let uppercase = text.to_uppercase();
        let new_start = (start as i64 + shift) as usize;
        let length = uppercase.chars().count();
        ranges[index] = if range.anchor <= range.head {
            plugin_sdk::SelectionRange {
                anchor: new_start,
                head: new_start + length,
            }
        } else {
            plugin_sdk::SelectionRange {
                anchor: new_start + length,
                head: new_start,
            }
        };
        shift += length as i64 - (end - start) as i64;
        previous_end = end;
        if uppercase != text {
            edits.push(TextEdit {
                start,
                end,
                text: uppercase,
            });
        }
    }
    if edits.is_empty() {
        return Response::default();
    }
    Response {
        actions: vec![
            Action::Edit {
                document: doc.id,
                version: doc.version,
                edits,
            },
            Action::SetSelection {
                document: doc.id,
                version: doc.version,
                view: view.id,
                binding_revision: view.binding_revision,
                selection_revision: view.selection_revision,
                ranges,
                primary: view.primary,
            },
        ],
        error: None,
    }
}
export_plugin!(handle);
