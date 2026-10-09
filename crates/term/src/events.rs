use event::{events, register_event};
use view::document::Mode;

use crate::commands;
use crate::keymap::MappableCommand;

/// The frontend entry point for a completed command invocation.
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommandOrigin {
    Keymap,
    Prompt,
    Palette,
    Custom,
    Macro,
    Repeat,
    Mouse,
    Programmatic,
}

events! {
    OnModeSwitch<'a, 'cx> { old_mode: Mode, new_mode: Mode, cx: &'a mut commands::Context<'cx> }
    PostInsertChar<'a, 'cx> { c: char, cx: &'a mut commands::Context<'cx> }
    PostCommand<'a, 'cx> { command: & 'a MappableCommand, cx: &'a mut commands::Context<'cx> }
    TerminalFocusGained<'a, 'cx> { cx: &'a mut commands::Context<'cx> }
    TerminalFocusLost<'a, 'cx> { cx: &'a mut commands::Context<'cx> }
}

pub fn register() {
    event::runtime_local! { static REGISTER: std::sync::Once = std::sync::Once::new(); }
    REGISTER.call_once(|| {
        register_event::<OnModeSwitch>();
        register_event::<PostInsertChar>();
        register_event::<PostCommand>();
        register_event::<TerminalFocusGained>();
        register_event::<TerminalFocusLost>();
    });
}
