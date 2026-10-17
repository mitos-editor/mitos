//! Terminal diagnostic visibility and mode transitions.

use event::{register_hook, send_blocking};
use view::{document::Mode, events::DiagnosticsDidChange, features::diagnostics::DiagnosticEvent};

pub(super) fn register_hooks() {
    register_hook!(move |event: &mut DiagnosticsDidChange<'_>| {
        if event.editor.mode() != Mode::Insert {
            for (view, _) in event.editor.tree.views_mut() {
                send_blocking(&view.diagnostics_handler.events, DiagnosticEvent::Refresh)
            }
        }
        Ok(())
    });
}
