//! Command palette construction and reopening the last picker.

use crate::{
    commands::{catalog, context::Context, mappable::MappableCommand},
    compositor::{self, Compositor},
    ui::{self, overlay::overlaid, Picker},
};
use std::borrow::Cow;
use tui::{text::Span, widgets::Cell};
use view::{document::Mode, theme::Style};

pub(super) struct CommandPaletteData {
    keymap: crate::keymap::ReverseKeymap,
    command_style: Style,
    binding_style: Style,
}

pub(super) fn command_palette_name<'a>(
    item: &'a MappableCommand,
    data: &CommandPaletteData,
) -> Cell<'a> {
    let name: Cow<'a, str> = match item {
        MappableCommand::Typable { name, .. } => format!(":{name}").into(),
        MappableCommand::Static { name, .. } => (*name).into(),
        MappableCommand::Macro { .. } => {
            unreachable!("macros aren't included in the command palette")
        }
    };

    Span::styled(name, data.command_style).into()
}

pub(super) fn command_palette_bindings<'a>(
    item: &MappableCommand,
    data: &CommandPaletteData,
) -> Cell<'a> {
    let bindings = data
        .keymap
        .get(item.name())
        .map(|bindings| {
            bindings.iter().fold(String::new(), |mut acc, bind| {
                if !acc.is_empty() {
                    acc.push(' ');
                }
                for key in bind {
                    acc.push_str(&key.key_sequence_format());
                }
                acc
            })
        })
        .unwrap_or_default();

    Span::styled(bindings, data.binding_style).into()
}

pub fn command_palette(cx: &mut Context) {
    let register = cx.register;
    let count = cx.count;

    cx.callback.push(Box::new(
        move |compositor: &mut Compositor, cx: &mut compositor::Context| {
            let keymap = compositor.find::<ui::EditorView>().unwrap().keymaps.map()
                [&cx.editor.mode]
                .reverse_map();
            let data = CommandPaletteData {
                keymap,
                command_style: cx.editor.theme.get("constant"),
                binding_style: cx.editor.theme.get("markup.raw.inline"),
            };

            let commands = MappableCommand::STATIC_COMMAND_LIST.iter().cloned().chain(
                catalog::TYPABLE_COMMAND_LIST
                    .iter()
                    .map(|cmd| MappableCommand::Typable {
                        name: cmd.name.to_owned(),
                        args: String::new(),
                        doc: cmd.doc.to_owned(),
                    }),
            );

            let columns = [
                ui::PickerColumn::new("name", command_palette_name),
                ui::PickerColumn::new("bindings", command_palette_bindings),
                ui::PickerColumn::new("doc", |item: &MappableCommand, _| item.doc().into()),
            ];

            let picker = Picker::new(columns, 0, commands, data, move |cx, command, _action| {
                let mut ctx = Context {
                    config: cx.config,
                    register,
                    count,
                    editor: cx.editor,
                    callback: Vec::new(),
                    on_next_key_callback: None,
                    jobs: cx.jobs,
                };
                let focus = view!(ctx.editor).id;

                command.execute(&mut ctx);

                if ctx.editor.tree.contains(focus) {
                    let config = ctx.editor.config();
                    let mode = ctx.editor.mode();
                    let view = view_mut!(ctx.editor, focus);
                    let doc = doc_mut!(ctx.editor, &view.doc);

                    view.ensure_cursor_in_view(doc, config.scrolloff);

                    if mode != Mode::Insert {
                        doc.append_changes_to_history(view);
                    }
                }
            });
            compositor.push(Box::new(overlaid(picker)));
        },
    ));
}

pub(super) fn last_picker(cx: &mut Context) {
    // TODO: last picker does not seem to work well with buffer_picker
    cx.callback.push(Box::new(|compositor, cx| {
        match compositor.last_picker.take() {
            Some(picker) => {
                compositor.push(picker);
            }
            _ => cx.editor.set_error(|| "no last picker"),
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use ui_core::input::KeyEvent;
    #[test]
    fn command_palette_styles_names_and_bindings() {
        let command_style = Style::default().fg(view::graphics::Color::Blue);
        let binding_style = Style::default().fg(view::graphics::Color::Magenta);
        let command = MappableCommand::Typable {
            name: "write".into(),
            args: String::new(),
            doc: String::new(),
        };
        let keys = vec![
            "space".parse::<KeyEvent>().unwrap(),
            "?".parse::<KeyEvent>().unwrap(),
        ];
        let data = CommandPaletteData {
            keymap: HashMap::from([("write".into(), vec![keys])]),
            command_style,
            binding_style,
        };

        let name = command_palette_name(&command, &data);
        let name = &name.content.lines[0].spans[0];
        assert_eq!(name.content, ":write");
        assert_eq!(name.style, tui::style::Style::from(command_style));

        let bindings = command_palette_bindings(&command, &data);
        let bindings = &bindings.content.lines[0].spans[0];
        assert_eq!(bindings.content, "<space>?");
        assert_eq!(bindings.style, tui::style::Style::from(binding_style));
    }
}
