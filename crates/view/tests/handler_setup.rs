//! Shared services run through the public view API without a terminal or compositor.
mod support;
use support::Fixture;

#[path = "handler_setup/clipboard.rs"]
mod clipboard;
#[path = "handler_setup/completion.rs"]
mod completion;
#[path = "handler_setup/configuration.rs"]
mod configuration;
#[path = "handler_setup/file_watching.rs"]
mod file_watching;
#[path = "handler_setup/plugin_services.rs"]
mod plugin_services;
#[path = "handler_setup/plugins.rs"]
mod plugins;
#[path = "handler_setup/snippets.rs"]
mod snippets;
#[path = "handler_setup/word_completion.rs"]
mod word_completion;
#[path = "handler_setup/workspace_trust.rs"]
mod workspace_trust;

#[path = "handler_setup/auto_save.rs"]
mod auto_save;
