//! Inline completion previews, requests, and document/view lifetime.
mod requests;
mod state;
pub use requests::{
    accept, cycle, dismiss, trigger, AppliedInlineCompletion, InlineCompletionHandler,
};
pub(crate) use requests::{mode_changed, register_hooks};
pub(crate) use state::InlineCompletions;
pub use state::{InlineCompletion, InlineCompletionRow};
