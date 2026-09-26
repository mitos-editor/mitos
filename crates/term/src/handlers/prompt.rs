use event::register_hook;
use view::events::DocumentFocusLost;

use crate::job::{self};
use crate::ui;

pub(super) fn register_hooks() {
    register_hook!(move |_event: &mut DocumentFocusLost<'_>| {
        job::dispatch_blocking(move |_, compositor| {
            if compositor.find::<ui::Prompt>().is_some() {
                compositor.remove_type::<ui::Prompt>();
            }
        });
        Ok(())
    });
}
