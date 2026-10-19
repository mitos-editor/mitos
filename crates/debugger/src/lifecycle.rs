//! Adapter lifecycle and configuration progress, independent of thread execution.

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
enum Phase {
    #[default]
    Starting,
    Active,
    Disconnecting,
    Terminated,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
enum Configuration {
    #[default]
    Idle,
    Synchronizing,
    Completing,
}

/// Startup and shutdown are distinct from running/stopped thread state.
#[derive(Default)]
pub struct SessionLifecycle {
    phase: Phase,
    configuration: Configuration,
}

impl SessionLifecycle {
    pub fn is_starting(&self) -> bool {
        self.phase == Phase::Starting
    }

    pub fn is_disconnecting(&self) -> bool {
        matches!(self.phase, Phase::Disconnecting | Phase::Terminated)
    }

    pub fn is_configuring(&self) -> bool {
        self.configuration != Configuration::Idle
    }

    pub fn startup_finished(&mut self) {
        if self.phase == Phase::Starting {
            self.phase = Phase::Active;
        }
    }

    pub fn begin_configuration(&mut self) {
        self.configuration = Configuration::Synchronizing;
    }

    pub fn needs_configuration_done(&self) -> bool {
        self.configuration == Configuration::Synchronizing
    }

    pub fn configuration_sent(&mut self) {
        self.configuration = Configuration::Completing;
    }

    pub fn configuration_finished(&mut self) {
        self.configuration = Configuration::Idle;
    }

    pub fn begin_disconnect(&mut self) {
        self.phase = Phase::Disconnecting;
    }

    /// A terminated execution may be replaced on the same adapter connection.
    /// Returns whether the adapter may request a restart. Explicit user shutdown wins.
    pub fn adapter_terminated(&mut self) -> bool {
        let restart_allowed = self.phase != Phase::Disconnecting;
        if restart_allowed {
            self.phase = Phase::Terminated;
        }
        self.configuration = Configuration::Idle;
        restart_allowed
    }

    /// An adapter may initialize a replacement execution while old cleanup is queued.
    /// Explicit user shutdown, however, must not be undone by a late event.
    pub fn accept_initialization(&mut self) -> bool {
        match self.phase {
            Phase::Disconnecting => false,
            Phase::Terminated => {
                self.phase = Phase::Active;
                true
            }
            _ => true,
        }
    }

    /// Both a rejected shutdown and an adapter-requested relaunch retain the connection.
    pub fn connection_retained(&mut self) {
        self.phase = Phase::Active;
    }

    pub fn ensure_command_available(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.is_starting(),
            "Selected debugging session is still starting"
        );
        anyhow::ensure!(
            !self.is_disconnecting(),
            "Debugger session is disconnecting"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_and_configuration_complete_independently() {
        let mut lifecycle = SessionLifecycle::default();
        lifecycle.begin_configuration();
        lifecycle.configuration_sent();
        assert!(!lifecycle.needs_configuration_done());
        lifecycle.configuration_finished();
        assert!(lifecycle.is_starting());
        assert!(lifecycle.ensure_command_available().is_err());
        lifecycle.startup_finished();
        assert!(lifecycle.ensure_command_available().is_ok());

        lifecycle.begin_configuration();
        lifecycle.configuration_sent();
        assert!(lifecycle.is_configuring());
        lifecycle.configuration_finished();
        assert!(!lifecycle.is_configuring());
    }

    #[test]
    fn failed_shutdown_restores_command_availability() {
        let mut lifecycle = SessionLifecycle::default();
        lifecycle.startup_finished();
        lifecycle.begin_disconnect();
        assert!(lifecycle.ensure_command_available().is_err());
        lifecycle.startup_finished();
        assert!(lifecycle.is_disconnecting());
        assert!(!lifecycle.adapter_terminated());
        assert!(!lifecycle.accept_initialization());
        lifecycle.connection_retained();
        assert!(lifecycle.ensure_command_available().is_ok());
        assert!(lifecycle.adapter_terminated());
        assert!(lifecycle.accept_initialization());
        assert!(!lifecycle.is_disconnecting());
    }
}
