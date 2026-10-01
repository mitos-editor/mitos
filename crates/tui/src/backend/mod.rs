//! Provides interface for controlling the terminal

use std::io;

use crate::terminal::Config;
use ui_core::{
    graphics::CursorKind,
    theme::{Color, Mode},
};

#[cfg(all(feature = "termina", not(windows)))]
mod termina;
#[cfg(all(feature = "termina", not(windows)))]
pub use self::termina::TerminaBackend;

#[cfg(all(feature = "termina", windows))]
mod crossterm;
#[cfg(all(feature = "termina", windows))]
pub use self::crossterm::CrosstermBackend;

mod test;
pub use self::test::TestBackend;

pub use ratatui::backend::Backend;

/// Terminal-session operations which are intentionally outside Ratatui's rendering backend.
pub trait BackendExt {
    /// Claims the terminal for TUI use.
    fn claim(&mut self) -> Result<(), io::Error>;
    /// Updates terminal configuration while the backend owns the terminal.
    fn reconfigure(&mut self, config: Config) -> Result<(), io::Error>;
    /// Restores the terminal to its normal state, undoing [`Self::claim`].
    fn restore(&mut self) -> Result<(), io::Error>;
    /// Sets the cursor shape without changing its visibility.
    fn set_cursor_kind(&mut self, kind: CursorKind) -> Result<(), io::Error>;
    /// Begins a synchronized-output frame (if the terminal supports it), so the
    /// draw and cursor updates between `start_sync` and `end_sync` present as one
    /// frame instead of flickering.
    fn start_sync(&mut self) -> Result<(), io::Error>;
    /// Ends the synchronized-output frame opened by `start_sync`.
    fn end_sync(&mut self) -> Result<(), io::Error>;
    /// Returns whether the backend can emit 24-bit foreground/background color.
    fn supports_true_color(&self) -> bool;
    /// Returns the terminal-reported light/dark theme, when supported.
    fn get_theme_mode(&self) -> Option<Mode>;
    /// Changes the terminal's default background color for the session.
    fn set_background_color(&mut self, color: Option<Color>) -> io::Result<()>;
}
