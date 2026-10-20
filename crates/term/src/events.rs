use event::{events, register_event};
use view::document::Mode;

use crate::commands;

events! {
    OnModeSwitch<'a, 'cx> { old_mode: Mode, new_mode: Mode, cx: &'a mut commands::Context<'cx> }
}

pub fn register() {
    event::runtime_local! { static REGISTER: std::sync::Once = std::sync::Once::new(); }
    REGISTER.call_once(|| {
        register_event::<OnModeSwitch>();
    });
}
