//! Inline completion previews, requests, and document/view lifetime.
mod requests;
mod state;
pub(crate) use requests::register_hooks;
pub use requests::{
    accept, cycle, dismiss, mode_changed, trigger, AppliedInlineCompletion, InlineCompletionHandler,
};
pub(crate) use state::InlineCompletions;
pub use state::{InlineCompletion, InlineCompletionRow};
