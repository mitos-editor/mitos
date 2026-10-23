//! Frontend messages for shared snippet navigation.
use crate::commands::context::Context;
use editor_core::movement::Direction;
use view::editor::SnippetNavigation;

pub(super) fn goto_next_tabstop(cx: &mut Context) {
    navigate(cx, Direction::Forward);
}
pub(super) fn goto_prev_tabstop(cx: &mut Context) {
    navigate(cx, Direction::Backward);
}
fn navigate(cx: &mut Context, direction: Direction) {
    if cx.editor.move_snippet_tabstop(direction) == SnippetNavigation::Inactive {
        cx.editor.set_error(|| "no snippet is currently active");
    }
}

pub(super) fn next_snippet_choice(cx: &mut Context) {
    cycle_choice(cx, Direction::Forward);
}
pub(super) fn prev_snippet_choice(cx: &mut Context) {
    cycle_choice(cx, Direction::Backward);
}
fn cycle_choice(cx: &mut Context, direction: Direction) {
    match cx.editor.cycle_snippet_choice(direction) {
        Ok(true) => {}
        Ok(false) => cx
            .editor
            .set_error(|| "no snippet choice is currently active"),
        Err(error) => cx.editor.set_error(|| error.to_string()),
    }
}

fn picker(
    editor: &mut view::Editor,
) -> anyhow::Result<crate::ui::Picker<view::features::snippets::SnippetCandidate, ()>> {
    use crate::ui::{picker::PickerCallbackResult, Picker, PickerColumn};
    use view::features::snippets::SnippetCandidate;
    let candidates = editor.snippet_candidates();
    anyhow::ensure!(
        !candidates.is_empty(),
        "no snippets are defined for this language"
    );
    Ok(Picker::new_with_callback_result(
        editor.redraw().clone(),
        [
            PickerColumn::new("name", |item: &SnippetCandidate, _| {
                item.snippet.name.as_str().into()
            }),
            PickerColumn::new("prefix", |item: &SnippetCandidate, _| {
                item.snippet.prefixes.join(", ").into()
            }),
            PickerColumn::new("description", |item: &SnippetCandidate, _| {
                item.snippet.description.as_str().into()
            }),
            PickerColumn::new("source", |item: &SnippetCandidate, _| {
                item.snippet.source.as_deref().unwrap_or("").into()
            }),
        ],
        0,
        candidates,
        (),
        |cx, candidate, _| match cx.editor.insert_user_snippet(candidate) {
            Ok(()) => PickerCallbackResult::Close,
            Err(error) => {
                cx.editor.set_error(|| error.to_string());
                PickerCallbackResult::Close
            }
        },
    )
    .with_title("Snippets"))
}

pub(super) fn snippet_picker(cx: &mut Context) {
    match picker(cx.editor) {
        Ok(picker) => cx.push_layer(Box::new(crate::ui::overlay::overlaid(picker))),
        Err(error) => cx.editor.set_error(|| error.to_string()),
    }
}

pub(super) mod typed {
    use crate::{compositor, ui::PromptEvent};
    use command_line::Args;
    use std::path::Path;
    use view::features::snippets::SnippetSource;

    pub fn insert(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event != PromptEvent::Validate {
            return Ok(());
        }
        let name = args.first().ok_or_else(|| {
            anyhow::anyhow!("snippet name is required; use snippet_picker to browse snippets")
        })?;
        let candidate = cx
            .editor
            .snippet_candidates()
            .into_iter()
            .find(|candidate| candidate.snippet.name == name)
            .ok_or_else(|| {
                anyhow::anyhow!("no snippet named {name:?} is available for this language")
            })?;
        cx.editor.insert_user_snippet(&candidate)
    }

    pub fn reload(cx: &mut compositor::Context, _: Args, event: PromptEvent) -> anyhow::Result<()> {
        if event == PromptEvent::Validate {
            cx.editor.reload_snippets()?;
            cx.editor.set_status("Snippet definitions reloaded");
        }
        Ok(())
    }

    pub fn open(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event == PromptEvent::Validate {
            cx.editor
                .open_snippet_config(SnippetSource::User, args.first().map(Path::new))?;
        }
        Ok(())
    }

    pub fn open_workspace(
        cx: &mut compositor::Context,
        args: Args,
        event: PromptEvent,
    ) -> anyhow::Result<()> {
        if event == PromptEvent::Validate {
            cx.editor
                .open_snippet_config(SnippetSource::Workspace, args.first().map(Path::new))?;
        }
        Ok(())
    }
}
