//! Sequential presentation of server-defined forms, including recursive lists.

use std::{cell::RefCell, rc::Rc, sync::Arc, time::Duration};

use event::{cancelable_future, TaskController};
use lsp_client::lsp::{self, FormFieldType};
use serde_json::{json, Value};
use tui::{
    buffer::Buffer,
    widgets::{Paragraph, Widget},
};
use view::{
    features::interactive::{validate_value, FormRequest},
    graphics::Rect,
    Editor,
};

use crate::{
    compositor::{Component, Compositor, Context, Cursor, Event, EventResult},
    job::Jobs,
    ui::{
        self,
        overlay::{overlaid, Overlay},
        picker::PickerCallbackResult,
        Picker, Prompt, PromptEvent,
    },
};

type State = Rc<RefCell<Form>>;
type ValueCallback = Box<dyn FnOnce(&mut Editor, &mut Compositor, &Jobs, Value)>;

struct Form {
    request: Option<FormRequest>,
    fields: Vec<lsp::FormField>,
    answers: Vec<lsp::FormAnswer>,
    index: usize,
}

pub(crate) fn show(
    editor: &mut Editor,
    compositor: &mut Compositor,
    jobs: &Jobs,
    request: FormRequest,
) {
    compositor.remove_type::<Input>();
    if !request.is_current(editor) {
        return;
    }
    let state = Rc::new(RefCell::new(Form {
        fields: request.fields().to_vec(),
        answers: request.answers().to_vec(),
        request: Some(request),
        index: 0,
    }));
    next_field(editor, compositor, jobs, state);
}

/// Remove an expired dialog before drawing; pending RPCs are canceled by the model.
pub(crate) fn clear_stale(editor: &Editor, compositor: &mut Compositor) {
    if compositor
        .find::<Input>()
        .is_some_and(|input| !input.is_current(editor))
    {
        compositor.remove_type::<Input>();
    }
}

fn next_field(editor: &mut Editor, compositor: &mut Compositor, jobs: &Jobs, state: State) {
    let mut form = state.borrow_mut();
    if !form
        .request
        .as_ref()
        .is_some_and(|request| request.is_current(editor))
    {
        return;
    }
    while let Some(field) = form.fields.get(form.index).cloned() {
        form.index += 1;
        let answer = form.answers.iter().find(|answer| answer.id == field.id);
        let valid = answer.is_some_and(|answer| {
            (answer.value.is_null() && !field.required)
                || validate_value(&field.field_type, &answer.value).is_ok()
        });
        if valid && field.error.as_ref().is_none_or(String::is_empty) {
            continue;
        }
        let default = answer.map(|answer| answer.value.clone()).or(field.default);
        let label = match field.error.filter(|error| !error.is_empty()) {
            Some(error) => format!("{} ({error})", field.description),
            None => field.description,
        };
        let id = field.id;
        drop(form);
        let next = state.clone();
        ask_value(
            editor,
            compositor,
            jobs,
            state,
            label,
            field.field_type,
            field.required,
            default,
            Box::new(move |editor, compositor, jobs, value| {
                let mut form = next.borrow_mut();
                form.answers.retain(|answer| answer.id != id);
                form.answers.push(lsp::FormAnswer { id, value });
                drop(form);
                next_field(editor, compositor, jobs, next);
            }),
        );
        return;
    }
    let request = form.request.take().unwrap();
    let mut answers = std::mem::take(&mut form.answers);
    answers.retain(|answer| form.fields.iter().any(|field| field.id == answer.id));
    drop(form);
    request.submit(editor, answers);
}

#[derive(Clone)]
struct Choice {
    label: String,
    value: Value,
}

enum Outcome {
    Text(String),
    Choice(Value),
    Abort,
}
enum InputKind {
    Text(Box<Prompt>),
    Choice(Box<Overlay<Picker<Choice, ()>>>),
}

/// Wrap native components so closing a picker never stores a live form in picker history.
struct Input {
    inner: InputKind,
    state: State,
    callback: Option<ValueCallback>,
    outcome: Rc<RefCell<Option<Outcome>>>,
    kind: FormFieldType,
    required: bool,
    default: Option<Value>,
    label: String,
    identity: Arc<()>,
    query: Option<String>,
    search: TaskController,
    completed: bool,
}

impl Drop for Input {
    fn drop(&mut self) {
        if !self.completed {
            self.state.borrow_mut().request.take();
        }
    }
}

impl Input {
    fn is_current(&self, editor: &Editor) -> bool {
        self.state
            .borrow()
            .request
            .as_ref()
            .is_some_and(|request| request.is_current(editor))
    }

    fn search(&mut self, editor: &Editor, jobs: &Jobs) {
        let FormFieldType::LazyEnum { source, config } = &self.kind else {
            return;
        };
        let InputKind::Choice(picker) = &mut self.inner else {
            return;
        };
        let query = picker.content.primary_query().to_string();
        if self.query.as_ref() == Some(&query) {
            return;
        }
        self.query = Some(query.clone());
        let default = query.is_empty().then(|| self.default.clone()).flatten();
        let handle = self.search.restart();
        // Remove old suggestions immediately: they belong to a different query.
        picker.content.refresh_options(Vec::new(), Arc::new(()));
        let params = lsp::InteractiveListEnumParams {
            source: source.clone(),
            config: config.clone(),
            query,
        };
        let state = self.state.borrow();
        let Some(request) = state.request.as_ref() else {
            return;
        };
        let identity = self.identity.clone();
        // Construct the RPC after the debounce rather than while the user is typing.
        let server_request = request.list_enum(editor, params);
        let Some(server_request) = server_request else {
            return;
        };
        let jobs = jobs.sender();
        let optional = !self.required;
        tokio::spawn(async move {
            let result = cancelable_future(
                async {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    server_request.await
                },
                handle.clone(),
            )
            .await;
            let Some(Some(result)) = result else {
                return;
            };
            jobs.send(move |editor, compositor| {
                if handle.is_canceled() {
                    return;
                }
                let Some(input) = compositor.find::<Input>() else {
                    return;
                };
                if !Arc::ptr_eq(&input.identity, &identity) || !input.is_current(editor) {
                    return;
                }
                match result {
                    Ok(entries) => {
                        let mut choices = enum_choices(entries);
                        if optional {
                            choices.push(skip_choice());
                        }
                        if let Some(default) = default {
                            if default.is_string()
                                && !choices.iter().any(|choice| choice.value == default)
                            {
                                choices.insert(
                                    0,
                                    Choice {
                                        label: default.as_str().unwrap().into(),
                                        value: default.clone(),
                                    },
                                );
                            }
                            choices.sort_by_key(|choice| choice.value != default);
                        }
                        if let InputKind::Choice(picker) = &mut input.inner {
                            picker.content.refresh_options(choices, Arc::new(()));
                        }
                    }
                    Err(error) => editor.set_error(|| format!("Failed to search choices: {error}")),
                }
            })
            .await;
        });
    }
}

impl Component for Input {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        if !self.is_current(cx.editor) {
            return EventResult::Consumed(Some(Box::new(|compositor, _| {
                compositor.remove_type::<Input>();
            })));
        }
        let result = match &mut self.inner {
            InputKind::Text(prompt) => prompt.handle_event(event, cx),
            InputKind::Choice(picker) => picker.handle_event(event, cx),
        };
        let outcome = self.outcome.borrow_mut().take();
        let value = match outcome {
            Some(Outcome::Text(text)) => {
                let form = self.state.borrow();
                let base = form.request.as_ref().unwrap().base_dir();
                match parse_text(&self.kind, &text, self.required, base) {
                    Ok(value) => Some(value),
                    Err(error) => {
                        cx.editor.set_error(|| error);
                        return EventResult::Consumed(None);
                    }
                }
            }
            Some(Outcome::Choice(value)) => Some(value),
            Some(Outcome::Abort) => None,
            None => {
                self.search(cx.editor, cx.jobs);
                // Native picker close callbacks pop its containing component.
                // Replace them so forms cannot be reopened from picker history.
                if matches!(result, EventResult::Consumed(Some(_))) {
                    // Enter while a search has no matching rows keeps the question open.
                    return EventResult::Consumed(None);
                }
                return result;
            }
        };
        if let Some(value) = value {
            self.completed = true;
            let callback = self.callback.take().unwrap();
            EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                compositor.remove_type::<Input>();
                callback(cx.editor, compositor, cx.jobs, value);
            })))
        } else {
            EventResult::Consumed(Some(Box::new(|compositor, _| {
                compositor.remove_type::<Input>();
            })))
        }
    }

    fn render(&mut self, area: Rect, frame: &mut Buffer, cx: &mut Context) {
        match &mut self.inner {
            InputKind::Text(prompt) => prompt.render(area, frame, cx),
            InputKind::Choice(picker) => {
                Paragraph::new(self.label.as_str())
                    .style(cx.editor.theme.get("ui.text"))
                    .render(Rect { height: 1, ..area }, frame);
                picker.render(
                    Rect {
                        y: area.y + 1,
                        height: area.height.saturating_sub(1),
                        ..area
                    },
                    frame,
                    cx,
                );
            }
        }
    }
    fn has_pending_keys(&self) -> bool {
        match &self.inner {
            InputKind::Text(prompt) => prompt.has_pending_keys(),
            InputKind::Choice(picker) => picker.has_pending_keys(),
        }
    }
    fn owns_cursor(&self) -> bool {
        true
    }
    fn cursor(&self, area: Rect, editor: &Editor) -> Cursor {
        match &self.inner {
            InputKind::Text(prompt) => prompt.cursor(area, editor),
            InputKind::Choice(picker) => picker.cursor(
                Rect {
                    y: area.y + 1,
                    height: area.height.saturating_sub(1),
                    ..area
                },
                editor,
            ),
        }
    }
    fn required_size(&mut self, size: (u16, u16)) -> Option<(u16, u16)> {
        match &mut self.inner {
            InputKind::Text(prompt) => prompt.required_size(size),
            InputKind::Choice(picker) => picker.required_size((size.0, size.1.saturating_sub(1))),
        }
    }
}

fn skip_choice() -> Choice {
    Choice {
        label: "Skip (leave unanswered)".into(),
        value: Value::Null,
    }
}
fn enum_choices(entries: Vec<lsp::FormEnumEntry>) -> Vec<Choice> {
    entries
        .into_iter()
        .map(|entry| Choice {
            label: if entry.description.is_empty() {
                entry.value.clone()
            } else {
                entry.description
            },
            value: Value::String(entry.value),
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn ask_value(
    editor: &mut Editor,
    compositor: &mut Compositor,
    jobs: &Jobs,
    state: State,
    label: String,
    kind: FormFieldType,
    required: bool,
    default: Option<Value>,
    callback: ValueCallback,
) {
    if let FormFieldType::List { element_type } = kind {
        let values = default
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        ask_list(
            editor,
            compositor,
            jobs,
            state,
            label,
            *element_type,
            required,
            values,
            callback,
        );
        return;
    }
    let choices = match &kind {
        FormFieldType::Bool => Some(vec![
            Choice {
                label: "Yes".into(),
                value: Value::Bool(true),
            },
            Choice {
                label: "No".into(),
                value: Value::Bool(false),
            },
        ]),
        FormFieldType::Enum { entries, .. } => Some(enum_choices(entries.clone())),
        FormFieldType::LazyEnum { .. } => Some(
            default
                .as_ref()
                .filter(|value| value.is_string())
                .map(|value| {
                    vec![Choice {
                        label: value.as_str().unwrap().into(),
                        value: value.clone(),
                    }]
                })
                .unwrap_or_default(),
        ),
        _ => None,
    };
    let outcome = Rc::new(RefCell::new(None));
    let output = outcome.clone();
    let inner = if let Some(mut choices) = choices {
        if !required {
            choices.push(skip_choice());
        }
        if let Some(default) = &default {
            choices.sort_by_key(|choice| choice.value != *default);
        }
        choice_input(
            editor,
            choices,
            output,
            matches!(kind, FormFieldType::LazyEnum { .. }),
        )
    } else {
        let is_file = matches!(kind, FormFieldType::File { .. });
        let mut line = default
            .as_ref()
            .map(|value| {
                value.as_str().map(str::to_owned).unwrap_or_else(|| {
                    if value.is_null() {
                        String::new()
                    } else {
                        value.to_string()
                    }
                })
            })
            .unwrap_or_default();
        if is_file
            && let Ok(url) = lsp::Url::parse(&line)
            && let Ok(path) = url.to_file_path()
        {
            line = path.to_string_lossy().into_owned();
        }
        let base = state
            .borrow()
            .request
            .as_ref()
            .unwrap()
            .base_dir()
            .to_owned();
        let prompt = Prompt::new(
            format!("{label}{}: ", if required { "" } else { " (optional)" }).into(),
            None,
            move |editor, text| {
                if is_file {
                    let path = std::path::Path::new(text);
                    if path.is_relative() && !text.starts_with('~') {
                        let absolute = base.join(path).to_string_lossy().into_owned();
                        let prefix = absolute.len().saturating_sub(text.len());
                        ui::completers::filename(editor, &absolute)
                            .into_iter()
                            .map(|(range, text)| ((range.start.saturating_sub(prefix)..), text))
                            .collect()
                    } else {
                        ui::completers::filename(editor, text)
                    }
                } else {
                    vec![]
                }
            },
            move |_, text, event| {
                *output.borrow_mut() = match event {
                    PromptEvent::Validate => Some(Outcome::Text(text.into())),
                    PromptEvent::Abort => Some(Outcome::Abort),
                    PromptEvent::Update => None,
                };
            },
        )
        .with_line(line, editor);
        InputKind::Text(Box::new(prompt))
    };
    let mut input = Input {
        inner,
        state,
        callback: Some(callback),
        outcome,
        kind,
        required,
        default,
        label,
        identity: Arc::new(()),
        query: None,
        search: TaskController::new(),
        completed: false,
    };
    input.search(editor, jobs);
    compositor.push(Box::new(input));
}

fn choice_input(
    editor: &Editor,
    choices: Vec<Choice>,
    output: Rc<RefCell<Option<Outcome>>>,
    remote: bool,
) -> InputKind {
    let aborted = output.clone();
    let column = ui::PickerColumn::new("option", |choice: &Choice, _| choice.label.as_str().into());
    // The server owns dynamic search semantics. Do not fuzzy-filter its results
    // again: a semantic result's description need not contain the query.
    let columns = if remote {
        vec![ui::PickerColumn::hidden("query"), column]
    } else {
        vec![column]
    };
    let picker = Picker::new_with_callback_result(
        editor.redraw().clone(),
        columns,
        0,
        choices,
        (),
        move |_, choice, _| {
            *output.borrow_mut() = Some(Outcome::Choice(choice.value.clone()));
            PickerCallbackResult::Close
        },
    )
    .with_cancel_callback(move |_| {
        *aborted.borrow_mut() = Some(Outcome::Abort);
    });
    InputKind::Choice(Box::new(overlaid(picker)))
}

#[allow(clippy::too_many_arguments)]
fn ask_list(
    editor: &mut Editor,
    compositor: &mut Compositor,
    _jobs: &Jobs,
    state: State,
    label: String,
    element: FormFieldType,
    required: bool,
    values: Vec<Value>,
    callback: ValueCallback,
) {
    let mut choices = vec![Choice {
        label: "Add item".into(),
        value: json!({"add": true}),
    }];
    for (index, value) in values.iter().enumerate() {
        choices.push(Choice {
            label: format!("Edit item {}: {value}", index + 1),
            value: json!({"edit": index}),
        });
        choices.push(Choice {
            label: format!("Remove item {}: {value}", index + 1),
            value: json!({"remove": index}),
        });
    }
    choices.push(Choice {
        label: format!("Done ({} items)", values.len()),
        value: json!({"done": true}),
    });
    if !required {
        choices.push(skip_choice());
    }
    let outcome = Rc::new(RefCell::new(None));
    let inner = choice_input(editor, choices, outcome.clone(), false);
    let next = state.clone();
    let next_label = label.clone();
    let continuation: ValueCallback = Box::new(move |editor, compositor, jobs, action| {
        let mut values = values;
        if action.is_null() {
            callback(editor, compositor, jobs, Value::Null);
        } else if action.get("done").is_some() {
            callback(editor, compositor, jobs, Value::Array(values));
        } else if let Some(index) = action.get("remove").and_then(Value::as_u64) {
            values.remove(index as usize);
            ask_list(
                editor, compositor, jobs, next, next_label, element, required, values, callback,
            );
        } else {
            let index = action
                .get("edit")
                .and_then(Value::as_u64)
                .map(|n| n as usize);
            let default = index.map(|index| values[index].clone());
            let item_label = format!("{next_label}: item {}", index.unwrap_or(values.len()) + 1);
            let next_element = element.clone();
            ask_value(
                editor,
                compositor,
                jobs,
                next.clone(),
                item_label,
                element,
                true,
                default,
                Box::new(move |editor, compositor, jobs, value| {
                    if let Some(index) = index {
                        values[index] = value;
                    } else {
                        values.push(value);
                    }
                    ask_list(
                        editor,
                        compositor,
                        jobs,
                        next,
                        next_label,
                        next_element,
                        required,
                        values,
                        callback,
                    );
                }),
            );
        }
    });
    compositor.push(Box::new(Input {
        inner,
        state,
        callback: Some(continuation),
        outcome,
        kind: FormFieldType::List {
            element_type: Box::new(FormFieldType::Unknown),
        },
        required,
        label,
        default: None,
        identity: Arc::new(()),
        query: None,
        search: TaskController::new(),
        completed: false,
    }));
}

fn parse_text(
    kind: &FormFieldType,
    input: &str,
    required: bool,
    base: &std::path::Path,
) -> Result<Value, String> {
    if input.is_empty() && !required {
        return Ok(Value::Null);
    }
    let value = match kind {
        FormFieldType::String => Value::String(input.into()),
        FormFieldType::Number => {
            let value: Value = serde_json::from_str(input).map_err(|_| "Enter a number")?;
            if !value.is_number() {
                return Err("Enter a number".into());
            }
            value
        }
        FormFieldType::File { .. } => {
            if input.is_empty() {
                return Err("Enter a file or directory".into());
            }
            let url = if input.starts_with("file:") {
                lsp::Url::parse(input).map_err(|err| err.to_string())?
            } else {
                let expanded = stdx::path::expand_tilde(std::path::Path::new(input));
                let path = base.join(expanded.as_ref());
                lsp::Url::from_file_path(path).map_err(|_| "Expected an absolute path")?
            };
            Value::String(url.to_string())
        }
        _ => return Err("Unsupported input type".into()),
    };
    validate_value(kind, &value)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn numbers_and_optional_answers_keep_their_types() {
        assert_eq!(
            parse_text(&FormFieldType::Number, "-1.25", true, Path::new("/")),
            Ok(json!(-1.25))
        );
        for invalid in ["NaN", "true", "[]", "\"2\"", "1e999"] {
            assert!(parse_text(&FormFieldType::Number, invalid, true, Path::new("/")).is_err());
        }
        assert_eq!(
            parse_text(&FormFieldType::String, "", false, Path::new("/")),
            Ok(Value::Null)
        );
        assert_eq!(
            parse_text(&FormFieldType::String, "", true, Path::new("/")),
            Ok(json!(""))
        );
    }

    #[test]
    fn file_inputs_resolve_relative_paths_and_enforce_constraints() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("existing.go"), "").unwrap();
        let existing = FormFieldType::File {
            existence: Some(2),
            file_type: None,
            filters: vec!["go".into()],
        };
        assert!(parse_text(&existing, "existing.go", true, dir.path()).is_ok());
        assert!(parse_text(&existing, "missing.go", true, dir.path()).is_err());
        assert!(parse_text(&existing, ".", true, dir.path()).is_err());
        let new = FormFieldType::File {
            existence: Some(1),
            file_type: Some(1),
            filters: vec!["go".into()],
        };
        assert!(parse_text(&new, "existing.go", true, dir.path()).is_err());
        assert!(parse_text(&new, "new.txt", true, dir.path()).is_err());
        let value = parse_text(&new, "new space-界.go", true, dir.path()).unwrap();
        assert_eq!(
            lsp::Url::parse(value.as_str().unwrap())
                .unwrap()
                .to_file_path()
                .unwrap(),
            dir.path().join("new space-界.go")
        );
        let directory = FormFieldType::File {
            existence: Some(2),
            file_type: Some(2),
            filters: vec![],
        };
        assert!(parse_text(&directory, ".", true, dir.path()).is_ok());
        assert!(parse_text(&directory, "existing.go", true, dir.path()).is_err());
    }
}
