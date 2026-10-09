//! Reload the WASM instances owned by this editor.

use crate::{compositor, ui::PromptEvent};
use command_line::Args;

pub(super) fn reload(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event == PromptEvent::Validate
        && cx
            .editor
            .reload_plugins(&cx.config.current.plugins, &loader::config_dir())
    {
        cx.editor.set_status("Plugins reloaded");
    }
    Ok(())
}
