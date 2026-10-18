//! Coordinates in formatted text, after wrapping and virtual annotations.

use std::ops::{Add, AddAssign, Sub};

/// A zero-based display row and terminal-cell column within formatted text.
///
/// Rows include soft wraps and virtual lines. These coordinates are relative to
/// the layout anchor, or to the viewport when explicitly translated by a view;
/// they are not document line and character coordinates.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DisplayPosition {
    pub row: usize,
    pub col: usize,
}

impl DisplayPosition {
    pub const fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

impl AddAssign for DisplayPosition {
    fn add_assign(&mut self, rhs: Self) {
        self.row += rhs.row;
        self.col += rhs.col;
    }
}

impl Add for DisplayPosition {
    type Output = Self;

    fn add(mut self, rhs: Self) -> Self {
        self += rhs;
        self
    }
}

impl Sub for DisplayPosition {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self::new(self.row - rhs.row, self.col - rhs.col)
    }
}
