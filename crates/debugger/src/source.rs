//! Editor-lifetime identities for adapter sources; adapter references may be reused.

/// Identity of one version of an adapter-provided source, independent of its wire reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceId(u64);

/// Allocate identities across all sessions owned by an editor.
#[derive(Default)]
pub struct SourceIds(u64);

impl SourceIds {
    pub fn allocate(&mut self) -> SourceId {
        self.0 = self
            .0
            .checked_add(1)
            .expect("debugger source identities exhausted");
        SourceId(self.0)
    }
}
