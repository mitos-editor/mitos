//! Terminal configuration schema, layer composition, keymaps, and trusted loading.
mod keymaps;
mod migration;
mod schema;
mod sources;

pub use crate::terminal_launcher::TerminalLauncherConfig;
pub use keymaps::Keys;
pub use migration::{ConfigMigration, MigrationScope};
pub use schema::{
    Config, ConfigEvent, ConfigRaw, Context, EditorSettings, KeyboardConfig, TerminalConfig,
};
pub use sources::ConfigLoadError;

#[cfg(test)]
mod tests;
