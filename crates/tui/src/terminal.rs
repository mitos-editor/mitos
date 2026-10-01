//! Mitos-specific terminal session configuration.

use ui_core::terminal::KittyKeyboardProtocolConfig;

/// Terminal capabilities and modes requested for one application session.
#[derive(Debug)]
pub struct Config {
    /// Whether claiming the terminal should enable mouse event reporting.
    pub enable_mouse_capture: bool,
    /// Emit extended underline escape sequences even when capability detection
    /// does not advertise them.
    pub force_enable_extended_underlines: bool,
    /// Policy for negotiating the Kitty keyboard protocol.
    pub kitty_keyboard_protocol: KittyKeyboardProtocolConfig,
}

/// The active cursor's presentation, independent of its terminal position.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    /// No cursor is visible and no input-method anchor is requested.
    #[default]
    Hidden,
    /// The component draws the cursor in the buffer. Keep the terminal cursor
    /// invisible but positioned for input methods.
    Software(ratatui::layout::Position),
    /// The terminal draws the cursor with the requested shape.
    Native(ratatui::layout::Position, ui_core::graphics::CursorKind),
}

/// Render the buffer, then apply its single active cursor. Cursor shape is set
/// before Ratatui shows it; software cursors only update the hidden IME anchor.
pub fn draw_with_cursor<B>(
    terminal: &mut ratatui::Terminal<B>,
    render: impl FnOnce(&mut ratatui::buffer::Buffer) -> Cursor,
) -> std::io::Result<()>
where
    B: crate::backend::Backend + crate::backend::BackendExt,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    terminal.autoresize().map_err(std::io::Error::other)?;
    let buffer = terminal.current_buffer_mut();
    let mut cursor = render(buffer);
    let position = match cursor {
        Cursor::Hidden => None,
        Cursor::Software(pos) | Cursor::Native(pos, _) => Some(pos),
    };
    if position.is_some_and(|pos| !buffer.area.contains(pos)) {
        cursor = Cursor::Hidden;
    }
    let native_position = if let Cursor::Native(pos, kind) = cursor {
        terminal.backend_mut().set_cursor_kind(kind)?;
        Some(pos)
    } else {
        None
    };
    terminal
        .apply_buffer_with_cursor(native_position)
        .map_err(std::io::Error::other)?;
    if let Cursor::Software(pos) = cursor {
        terminal
            .set_cursor_position(pos)
            .map_err(std::io::Error::other)?;
        terminal
            .backend_mut()
            .flush()
            .map_err(std::io::Error::other)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Backend, TestBackend};
    use ratatui::{layout::Position, Terminal};
    use ui_core::graphics::CursorKind;

    #[test]
    fn cursor_visibility_and_position_follow_each_frame() {
        let mut terminal = Terminal::new(TestBackend::new(10, 3)).unwrap();
        for (cursor, visible, position) in [
            (Cursor::Software((1, 0).into()), false, (1, 0)),
            (Cursor::Native((2, 0).into(), CursorKind::Bar), true, (2, 0)),
            (
                Cursor::Native((3, 0).into(), CursorKind::Underline),
                true,
                (3, 0),
            ),
            (Cursor::Software((4, 0).into()), false, (4, 0)),
            (Cursor::Software((5, 0).into()), false, (5, 0)),
        ] {
            draw_with_cursor(&mut terminal, |_| cursor).unwrap();
            assert_eq!(terminal.backend().cursor_visible(), visible);
            assert_eq!(
                terminal.backend_mut().get_cursor_position().unwrap(),
                Position::from(position)
            );
        }
        draw_with_cursor(&mut terminal, |_| Cursor::Hidden).unwrap();
        assert!(!terminal.backend().cursor_visible());
    }

    #[test]
    fn resize_hides_cursors_outside_the_viewport() {
        let mut terminal = Terminal::new(TestBackend::new(10, 3)).unwrap();
        let native = Cursor::Native((8, 2).into(), CursorKind::Block);
        draw_with_cursor(&mut terminal, |_| native).unwrap();
        assert!(terminal.backend().cursor_visible());
        terminal.backend_mut().resize(4, 1);
        for cursor in [native, Cursor::Software((8, 2).into())] {
            draw_with_cursor(&mut terminal, |_| cursor).unwrap();
            assert!(!terminal.backend().cursor_visible());
        }
        draw_with_cursor(&mut terminal, |_| {
            Cursor::Native((1, 0).into(), CursorKind::Bar)
        })
        .unwrap();
        assert!(terminal.backend().cursor_visible());
    }
}
