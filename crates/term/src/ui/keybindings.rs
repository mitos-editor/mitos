//! Searchable bindings and a key inspector that never executes captured input.

use crate::{
    compositor::{self, Component, Context, DrawContext, Event, EventResult},
    keymap::inspect::{self, Definition, Source},
    ui::{overlay::overlaid, picker::PickerCallbackResult, Picker, PickerColumn, Popup, Text},
};
use tui::buffer::Buffer;
use ui_core::input::KeyEvent;
use view::graphics::Rect;

const INSPECTOR_ID: &str = "key-inspector";

fn defaults() -> Vec<Definition> {
    let config = crate::config::Config::default();
    let mut bindings = inspect::components(&config.component_keys);
    for (mode, keys) in &config.keys {
        bindings.extend(inspect::editor(&format!("keys.{mode}"), keys));
    }
    bindings
}

fn details(
    contexts: &[inspect::Context],
    defaults: &[Definition],
    sources: &[(Source, Definition)],
    keys: &[KeyEvent],
) -> String {
    let mut text = format!("Keys: {}\n", inspect::sequence(keys));
    if let Some(resolved) = inspect::resolve(contexts, keys) {
        text.push_str(&format!(
            "Context: {}\nAction: {}\n",
            resolved.context.name, resolved.action
        ));
        if !resolved.context.pending.is_empty() {
            text.push_str(&format!(
                "Pending: {}\n",
                inspect::sequence(&resolved.context.pending)
            ));
        }
        if let Some(binding) = resolved.definition {
            text.push_str(&format!("{}\n\n", binding.description));
            let mut original = binding.clone();
            original.keys = resolved
                .context
                .origin_prefix
                .iter()
                .chain(&binding.keys)
                .copied()
                .collect();
            let origins = inspect::origins(&original, sources, defaults);
            text.push_str(&format!("Source: {}\n", origins[0]));
            for origin in &origins[1..] {
                text.push_str(&format!("Shadowed: {origin}\n"));
            }
        } else {
            text.push_str("Source: runtime input handling\n");
        }
        for context in contexts {
            let path: Vec<_> = context.pending.iter().chain(keys).copied().collect();
            if let Some(binding) = context
                .bindings
                .iter()
                .find(|binding| binding.keys == path && binding.action == "unbound")
            {
                text.push_str(&format!(
                    "Unbound: {}\n",
                    inspect::origins(binding, sources, defaults)[0]
                ));
            }
            if std::ptr::eq(context, resolved.context) {
                continue;
            }
            if let Some(binding) = context.bindings.iter().find(|binding| binding.keys == path) {
                text.push_str(&format!(
                    "Shadowed context: {} — {}\n",
                    context.name, binding.action
                ));
            }
        }
        if resolved.prefix {
            text.push_str("\nWaiting for another key.");
        }
    } else {
        text.push_str("No active binding.\n");
    }
    text
}

#[derive(Clone)]
struct Row {
    context: String,
    keys: String,
    action: String,
    source: String,
    state: &'static str,
    description: String,
    details: String,
}

pub(crate) fn show(compositor: &mut compositor::Compositor, cx: &mut Context) {
    let contexts = compositor.keymap_contexts(cx);
    let defaults = defaults();
    let mut rows = Vec::new();
    for context in &contexts {
        for binding in &context.bindings {
            let Some(input) = binding.keys.strip_prefix(context.pending.as_slice()) else {
                continue;
            };
            if input.is_empty() {
                continue;
            }
            let resolution = inspect::resolve(&contexts, input);
            let state = if binding.action == "unbound" {
                "unbound"
            } else if resolution.is_some_and(|resolved| std::ptr::eq(resolved.context, context)) {
                if binding.prefix {
                    "prefix"
                } else {
                    "active"
                }
            } else {
                "shadowed"
            };
            let mut original = binding.clone();
            original.keys = context
                .origin_prefix
                .iter()
                .chain(&binding.keys)
                .copied()
                .collect();
            rows.push(Row {
                context: context.name.clone(),
                keys: inspect::sequence(input),
                action: binding.action.clone(),
                source: inspect::origins(
                    &original,
                    &cx.config.current.keybinding_sources,
                    &defaults,
                )[0]
                .clone(),
                state,
                description: binding.description.clone(),
                details: details(
                    &contexts,
                    &defaults,
                    &cx.config.current.keybinding_sources,
                    input,
                ),
            });
        }
    }
    rows.sort_by(|a, b| a.context.cmp(&b.context).then(a.keys.cmp(&b.keys)));
    let picker = Picker::new_with_callback_result(
        cx.editor.redraw().clone(),
        [
            PickerColumn::new("keys", |row: &Row, _: &()| row.keys.as_str().into()),
            PickerColumn::new("action", |row: &Row, _: &()| row.action.as_str().into()),
            PickerColumn::new("context", |row: &Row, _: &()| row.context.as_str().into()),
            PickerColumn::new("source", |row: &Row, _: &()| row.source.as_str().into()),
            PickerColumn::new("state", |row: &Row, _: &()| row.state.into()),
            PickerColumn::new("description", |row: &Row, _: &()| {
                row.description.as_str().into()
            }),
        ],
        1,
        rows,
        (),
        |_, row, _| {
            let details = row.details.clone();
            PickerCallbackResult::CloseWithCallback(Box::new(move |compositor, _| {
                compositor.push(Box::new(Popup::new(
                    "keybinding-details",
                    Text::new(details),
                )));
            }))
        },
    )
    .with_title("Keybindings — type to filter; Ctrl-x i inspects a key");
    compositor.push(Box::new(overlaid(picker)));
}

pub(crate) fn inspect(compositor: &mut compositor::Compositor, cx: &mut Context) {
    let inspector = Inspector {
        contexts: compositor.keymap_contexts(cx),
        defaults: defaults(),
        sources: cx.config.current.keybinding_sources.clone(),
        input: Vec::new(),
        complete: false,
        text: Text::new(
            "Press a key or sequence to inspect it. Captured keys are not executed.".to_owned(),
        ),
    };
    compositor.push(Box::new(Popup::new(INSPECTOR_ID, inspector)));
}

struct Inspector {
    contexts: Vec<inspect::Context>,
    defaults: Vec<Definition>,
    sources: Vec<(Source, Definition)>,
    input: Vec<KeyEvent>,
    complete: bool,
    text: Text,
}

impl Component for Inspector {
    fn handle_event(&mut self, event: &Event, _: &mut Context) -> EventResult {
        let Event::Key(key) = *event else {
            return EventResult::Consumed(None);
        };
        if self.complete && (key == crate::key!(Esc) || key == crate::ctrl!('c')) {
            return EventResult::Consumed(Some(Box::new(|compositor, _| {
                compositor.remove(INSPECTOR_ID);
            })));
        }
        if self.complete {
            self.input.clear();
        }
        self.input.push(key);
        self.complete =
            inspect::resolve(&self.contexts, &self.input).is_none_or(|resolved| !resolved.prefix);
        let mut text = details(&self.contexts, &self.defaults, &self.sources, &self.input);
        if self.complete {
            text.push_str("\nEscape closes; another key starts a new inspection.");
        }
        self.text = Text::new(text);
        EventResult::Consumed(None)
    }

    fn render(&mut self, area: Rect, surface: &mut Buffer, cx: &mut DrawContext) {
        self.text.render(area, surface, cx);
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        self.text.required_size(viewport)
    }
}
