use std::io;

use ui_core::{graphics::CursorKind, theme};

use crate::terminal::Config;

use super::BackendExt;

pub type TestBackend = ratatui::backend::TestBackend;

impl BackendExt for TestBackend {
    fn claim(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn reconfigure(&mut self, _config: Config) -> io::Result<()> {
        Ok(())
    }

    fn restore(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn set_cursor_kind(&mut self, _kind: CursorKind) -> io::Result<()> {
        Ok(())
    }

    fn start_sync(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn end_sync(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn supports_true_color(&self) -> bool {
        true
    }

    fn get_theme_mode(&self) -> Option<theme::Mode> {
        None
    }

    fn set_background_color(&mut self, _color: Option<theme::Color>) -> io::Result<()> {
        Ok(())
    }
}
