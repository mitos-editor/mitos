//! A frame picker that extends its loaded prefix without losing the query or selection.
use super::{picker::PickerCallbackResult, Picker, PickerColumn};
use crate::{
    compositor::{Component, Context, Cursor, Event, EventResult},
    job::Callback,
};
use event::TaskController;
use std::{cell::Cell, rc::Rc, sync::Arc};
use tui::buffer::Buffer;
use view::{debugger::DebuggerFrames, graphics::Rect, Editor};

#[derive(Clone, Copy)]
enum Action {
    Frame(usize),
    More,
}

struct Row {
    action: Action,
    id: Option<usize>,
    name: String,
    frame: Option<dap::StackFrame>,
}

pub(crate) struct Frames {
    data: DebuggerFrames,
    picker: Picker<Row, ()>,
    action: Rc<Cell<Option<Action>>>,
    load: TaskController,
}

impl Frames {
    fn rows(data: &DebuggerFrames) -> Vec<Row> {
        let mut rows: Vec<_> = data
            .frames
            .iter()
            .map(|(index, frame)| Row {
                action: Action::Frame(*index),
                id: Some(frame.id),
                name: format!("{} ({})", frame.name, data.thread_state),
                frame: Some(frame.clone()),
            })
            .collect();
        if data.has_more {
            rows.push(Row {
                action: Action::More,
                id: None,
                name: "Load more…".into(),
                frame: None,
            });
        }
        rows
    }

    pub fn new(editor: &Editor, data: DebuggerFrames) -> Self {
        let action = Rc::new(Cell::new(None));
        let selected = action.clone();
        let preview = data.context.clone();
        let picker = Picker::new_with_callback_result(
            editor.redraw().clone(),
            [PickerColumn::new("frame", |row: &Row, _| {
                row.name.as_str().into()
            })],
            0,
            Self::rows(&data),
            (),
            move |_, row, _| {
                selected.set(Some(row.action));
                PickerCallbackResult::KeepOpen
            },
        )
        .with_item_key(|row| row.id)
        .with_preview(move |editor, row| {
            if !preview.is_current(editor) {
                return None;
            }
            let frame = row.frame.as_ref()?;
            let path = view::debugger::source::local_path(frame)?;
            Some((
                path.into(),
                Some(view::debugger::source::frame_line_range(frame)?),
            ))
        });
        Self {
            data,
            picker,
            action,
            load: TaskController::new(),
        }
    }

    fn more(&mut self, cx: &mut Context) {
        let task = match cx.editor.debugger_more_stack_frames(&self.data) {
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
                    let Some(picker) = compositor.find_id::<Frames>("dap-frames") else {
                        return;
                    };
                    match update.and_then(|update| update.into_result(editor)) {
                        Some(Ok(page)) => {
                            if let Some(data) = page.apply(editor) {
                                picker
                                    .picker
                                    .refresh_options(Self::rows(&data), Arc::new(()));
                                picker.data = data;
                            }
                        }
                        Some(Err(error)) => {
                            editor.set_error(|| format!("Could not load stack frames: {error}"))
                        }
                        None => {}
                    }
                },
            )))
        });
    }
}

impl Component for Frames {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        if !self.data.context.is_current(cx.editor) {
            self.load.cancel();
            return EventResult::Consumed(Some(Box::new(|compositor, _| {
                compositor.remove("dap-frames");
            })));
        }
        let result = self.picker.handle_event(event, cx);
        if matches!(&result, EventResult::Consumed(Some(_))) {
            self.load.cancel();
        }
        match self.action.take() {
            Some(Action::More) => self.more(cx),
            Some(Action::Frame(index)) => {
                self.load.cancel();
                if let Err(error) = cx.editor.select_debugger_frame(&self.data.context, index) {
                    cx.editor.set_error(|| error.to_string());
                }
                return EventResult::Consumed(Some(Box::new(|compositor, _| {
                    compositor.remove("dap-frames");
                })));
            }
            None => {}
        }
        result
    }
    fn render(&mut self, area: Rect, surface: &mut Buffer, cx: &mut Context) {
        if self.data.context.is_current(cx.editor) {
            self.picker.required_size((area.width, area.height));
            self.picker.render(area, surface, cx);
        } else {
            super::Popup::new(
                "dap-frames",
                super::Text::new(
                    "Debugger frames are no longer current. Press any key to close.".into(),
                ),
            )
            .render(area, surface, cx);
        }
    }
    fn has_pending_keys(&self) -> bool {
        self.picker.has_pending_keys()
    }
    #[cfg(feature = "integration")]
    fn has_pending_render(&mut self) -> bool {
        self.picker.has_pending_render()
    }
    fn owns_cursor(&self) -> bool {
        self.picker.owns_cursor()
    }
    fn cursor(&self, area: Rect, editor: &Editor) -> Cursor {
        if self.data.context.is_current(editor) {
            self.picker.cursor(area, editor)
        } else {
            Cursor::Hidden
        }
    }
    fn id(&self) -> Option<&'static str> {
        Some("dap-frames")
    }
}
