//! A lazy variable browser. Retained parent pickers preserve their query and cursor.

use super::{picker::PickerCallbackResult, Picker, PickerColumn};
use crate::{
    compositor::{Component, Context, Cursor, Event, EventResult},
    job::Callback,
};
use event::TaskController;
use std::{cell::Cell, rc::Rc, sync::Arc};
use tui::buffer::Buffer;
use view::{debugger::DebuggerVariables, graphics::Rect, Editor};

#[derive(Clone, Copy)]
enum Action {
    Back,
    Expand(usize),
}

struct Row {
    action: Action,
    key: Option<String>,
    name: String,
    value: String,
}

struct Page {
    data: DebuggerVariables,
    picker: Picker<Row, ()>,
}

pub(crate) struct Variables {
    current: Page,
    parents: Vec<Page>,
    action: Rc<Cell<Option<Action>>>,
    load: TaskController,
    refreshing: bool,
    refresh_failed: bool,
}

impl Page {
    fn rows(data: &DebuggerVariables, parent: bool) -> Vec<Row> {
        let mut rows = Vec::new();
        if parent {
            rows.push(Row {
                action: Action::Back,
                key: None,
                name: ".. (back)".into(),
                value: String::new(),
            });
        }
        rows.extend(data.entries().iter().enumerate().map(|(index, entry)| {
            let marker = if entry.is_expandable() { "▸ " } else { "  " };
            let expensive = if entry.expensive { " [expensive]" } else { "" };
            let ty = entry
                .ty
                .as_deref()
                .map(|ty| format!(": {ty}"))
                .unwrap_or_default();
            Row {
                action: Action::Expand(index),
                key: Some(entry.name.clone()),
                name: format!("{marker}{}{ty}{expensive}", entry.name),
                value: entry.value.clone().unwrap_or_default(),
            }
        }));
        rows
    }

    fn refresh(&mut self, data: DebuggerVariables, parent: bool) {
        self.picker
            .refresh_options(Self::rows(&data, parent), Arc::new(()));
        self.data = data;
    }

    fn new(
        editor: &Editor,
        data: DebuggerVariables,
        parent: bool,
        action: Rc<Cell<Option<Action>>>,
    ) -> Self {
        let rows = Self::rows(&data, parent);
        let picker = Picker::new_with_callback_result(
            editor.redraw().clone(),
            [
                PickerColumn::new("name", |row: &Row, _| row.name.as_str().into()),
                PickerColumn::new("value", |row: &Row, _| row.value.as_str().into()),
            ],
            0,
            rows,
            (),
            move |_, row, _| {
                action.set(Some(row.action));
                PickerCallbackResult::KeepOpen
            },
        )
        .with_item_key(|row| row.key.clone());
        Self { data, picker }
    }
}

impl Variables {
    pub fn new(editor: &Editor, data: DebuggerVariables) -> Self {
        let action = Rc::new(Cell::new(None));
        Self {
            current: Page::new(editor, data, false, action.clone()),
            parents: Vec::new(),
            action,
            load: TaskController::new(),
            refreshing: false,
            refresh_failed: false,
        }
    }

    fn push(&mut self, editor: &Editor, data: DebuggerVariables) {
        let page = Page::new(editor, data, true, self.action.clone());
        self.parents
            .push(std::mem::replace(&mut self.current, page));
    }

    fn refresh(&mut self, cx: &mut Context) {
        if self.refreshing || self.refresh_failed {
            return;
        }
        let task = match cx.editor.debugger_refresh_variables(&self.current.data) {
            Ok(task) => task,
            Err(error) => {
                self.refresh_failed = true;
                cx.editor.set_error(|| error.to_string());
                return;
            }
        };
        self.refreshing = true;
        let load = self.load.restart();
        cx.jobs.callback(async move {
            let update = tokio::select! {
                biased;
                _ = load.canceled() => return Ok(Callback::Editor(Box::new(|_| {}))),
                update = task.run() => update,
            };
            Ok(Callback::EditorCompositor(Box::new(
                move |editor, compositor| {
                    if load.is_canceled() {
                        return;
                    }
                    let Some(browser) = compositor.find_id::<Variables>("dap-variables") else {
                        return;
                    };
                    browser.refreshing = false;
                    match update.and_then(|update| update.into_result(editor)) {
                        Some(Ok(mut pages)) => {
                            let current = pages.pop().expect("refresh always returns scopes");
                            if pages.len() < browser.parents.len() {
                                browser.current = browser.parents.remove(pages.len());
                                browser.parents.truncate(pages.len());
                            }
                            for (index, (page, data)) in
                                browser.parents.iter_mut().zip(pages).enumerate()
                            {
                                page.refresh(data, index != 0);
                            }
                            browser
                                .current
                                .refresh(current, !browser.parents.is_empty());
                        }
                        Some(Err(error)) => {
                            browser.refresh_failed = true;
                            editor.set_error(|| {
                                format!("Could not refresh debugger values: {error}")
                            });
                        }
                        None => {} // A newer invalidation will be retried on the next render.
                    }
                    editor.redraw().request();
                },
            )))
        });
    }

    fn expand(&mut self, cx: &mut Context, index: usize) {
        let page = &self.current.data;
        if !page.entries()[index].is_expandable() {
            return;
        }
        let task = match cx.editor.debugger_variable_children(page, index) {
            Ok(task) => task,
            Err(error) => {
                cx.editor.set_error(|| error.to_string());
                return;
            }
        };
        let load = self.load.restart();
        cx.jobs.callback(async move {
            let update = tokio::select! {
                biased;
                _ = load.canceled() => return Ok(Callback::Editor(Box::new(|_| {}))),
                update = task.run() => update,
            };
            Ok(Callback::EditorCompositor(Box::new(
                move |editor, compositor| {
                    if load.is_canceled() {
                        return;
                    }
                    let Some(browser) = compositor.find_id::<Variables>("dap-variables") else {
                        return;
                    };
                    let Some(update) = update else {
                        return;
                    };
                    match update.into_result(editor) {
                        Some(Ok(data)) => browser.push(editor, data),
                        Some(Err(error)) => {
                            editor.set_error(|| format!("Debugger request failed: {error}"))
                        }
                        None => {}
                    }
                },
            )))
        });
    }
}

impl Component for Variables {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        if !self.current.data.can_refresh(cx.editor) {
            self.load.cancel();
            return EventResult::Consumed(Some(Box::new(|compositor, _| {
                compositor.remove("dap-variables");
            })));
        }
        if !self.current.data.is_current(cx.editor) {
            self.refresh_failed = false;
            self.refresh(cx);
            return EventResult::Consumed(None);
        }
        let result = self.current.picker.handle_event(event, cx);

        if matches!(&result, EventResult::Consumed(Some(_))) {
            self.load.cancel();
        }
        match self.action.take() {
            Some(Action::Back) => {
                self.load.cancel();
                if let Some(parent) = self.parents.pop() {
                    self.current = parent;
                }
            }
            Some(Action::Expand(index)) => self.expand(cx, index),
            None => {}
        }
        result
    }

    fn render(&mut self, area: Rect, surface: &mut Buffer, cx: &mut Context) {
        if self.current.data.can_refresh(cx.editor) {
            if !self.current.data.is_current(cx.editor) {
                self.refresh(cx);
            }
            let picker = &mut self.current.picker;
            picker.required_size((area.width, area.height));
            picker.render(area, surface, cx);
        } else {
            super::Popup::new(
                "dap-variables",
                super::Text::new(
                    "Debugger values are no longer current. Press any key to close.".into(),
                ),
            )
            .render(area, surface, cx);
        }
    }

    fn has_pending_keys(&self) -> bool {
        self.current.picker.has_pending_keys()
    }
    #[cfg(feature = "integration")]
    fn has_pending_render(&mut self) -> bool {
        self.current.picker.has_pending_render()
    }
    fn owns_cursor(&self) -> bool {
        self.current.picker.owns_cursor()
    }
    fn cursor(&self, area: Rect, editor: &Editor) -> Cursor {
        if self.current.data.can_refresh(editor) {
            self.current.picker.cursor(area, editor)
        } else {
            Cursor::Hidden
        }
    }
    fn id(&self) -> Option<&'static str> {
        Some("dap-variables")
    }
}
