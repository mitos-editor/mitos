//! Mitos's thin terminal-rendering layer over Ratatui.
//!
//! Most familiar names are re-exported directly from Ratatui through modules
//! such as [`layout`], [`style`], [`text`], and [`widgets`]. Mitos-specific code
//! lives at the boundaries Ratatui intentionally does not own: terminal session
//! setup in [`backend`], editor-aware buffer writes in [`buffer::BufferExt`],
//! and [`table`] row preparation for start truncation.
//!
//! New rendering code should use Ratatui primitives directly unless the editor
//! needs behavior that is shared across several components.

pub mod backend;
pub mod panel;
pub mod scrollbar;
pub mod table;
pub mod terminal;

/// Ratatui buffer types plus editor-specific write operations.
pub mod buffer {
    pub use ratatui::buffer::*;
    pub use ui_core::buffer::BufferExt;
}

/// Ratatui layout primitives.
pub mod layout {
    pub use ratatui::layout::*;
}

/// Ratatui symbol sets.
pub mod symbols {
    pub use ratatui::symbols::*;
}

/// Ratatui styling primitives.
pub mod style {
    pub use ratatui::style::*;
}

/// Ratatui styled text primitives.
pub mod text {
    pub use ratatui::text::*;
}

/// Ratatui widgets plus editor table row preparation values.
pub mod widgets {
    pub use crate::table::{Cell, Row};
    pub use ratatui::widgets::{
        Block, BorderType, Borders, HighlightSpacing, Padding, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, StatefulWidget, Table, TableState, Tabs, Widget,
        Wrap,
    };
}

pub use ratatui::{Frame, Terminal, TerminalOptions, Viewport};
