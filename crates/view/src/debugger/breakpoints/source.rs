//! Document source capabilities keep adapter breakpoints inside their original session.
use super::*;
use crate::debugger::DebuggerSource;

impl Editor {
    /// Markers for a live document source, including adapter-discovered breakpoints.
    pub fn debugger_breakpoint_locations(
        &self,
        source: &DebuggerSource,
    ) -> Vec<BreakpointLocation> {
        if !source.is_current(self) {
            return Vec::new();
        }
        let bindings = match source.adapter() {
            Some((id, _)) => self.debugger.get_session(id),
            None => self.debugger.get_active_session(),
        }
        .and_then(|session| session.breakpoints.as_ref())
        .map(|session| &session.bindings);
        self.debugger
            .breakpoints
            .locations_for(source.breakpoint_source(), bindings)
            .collect()
    }

    pub fn debugger_breakpoint_at_line(
        &self,
        source: &DebuggerSource,
        line: usize,
    ) -> Option<Breakpoint> {
        self.debugger_breakpoint_locations(source)
            .into_iter()
            .find(|location| location.line == line)
            .and_then(|location| location.definition)
    }

    pub fn toggle_source_breakpoints(
        &mut self,
        source: &DebuggerSource,
        lines: &[usize],
    ) -> Result<()> {
        anyhow::ensure!(
            source.is_current(self),
            "Debugger source is no longer available"
        );
        if let Some(path) = source.local_path() {
            return self.toggle_breakpoints(path.to_owned(), lines);
        }
        let (id, _) = source
            .adapter()
            .context("Debugger source has no owning session")?;
        let key = source.breakpoint_source();
        let mut lines = lines.to_vec();
        lines.sort_unstable();
        lines.dedup();
        for line in lines {
            if let Some(existing) = self.debugger_breakpoint_at_line(source, line) {
                self.debugger
                    .breakpoints
                    .remove_for(&key, existing.identity());
                if let Some(state) = self.debugger.sessions[id].breakpoints.as_mut() {
                    state.remove_binding(existing.identity());
                }
            } else {
                self.debugger.breakpoints.insert_for(key.clone(), line);
            }
        }
        self.sync_debugger_source_breakpoints(id, key, false)
    }

    pub fn set_source_breakpoint_property(
        &mut self,
        source: &DebuggerSource,
        identity: BreakpointId,
        property: BreakpointProperty,
        value: Option<String>,
    ) -> Result<()> {
        anyhow::ensure!(
            source.is_current(self),
            "Debugger source is no longer available"
        );
        if let Some(path) = source.local_path() {
            return self.set_breakpoint_property(path, identity, property, value);
        }
        let (id, _) = source
            .adapter()
            .context("Debugger source has no owning session")?;
        let key = source.breakpoint_source();
        self.debugger.breakpoints.set_property_for(
            &key,
            identity,
            property,
            value,
            self.debugger.sessions[id].capabilities(),
        )?;
        self.sync_debugger_source_breakpoints(id, key, false)
    }
}
