//! Adapter identity shared by the transport and its session registry.

use std::fmt;

slotmap::new_key_type! {
    /// Generational identifier for a debug adapter allocated by its session owner.
    pub struct DebugAdapterId;
}

impl fmt::Display for DebugAdapterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}
