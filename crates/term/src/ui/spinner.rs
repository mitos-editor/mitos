use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use lsp_client::LanguageServerId;

const FRAMES: [&str; 8] = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
const FRAME_INTERVAL_MS: u128 = 80;

pub(super) fn frame(elapsed: Duration) -> &'static str {
    FRAMES[(elapsed.as_millis() / FRAME_INTERVAL_MS % FRAMES.len() as u128) as usize]
}

#[derive(Default, Debug)]
pub struct ProgressSpinners {
    inner: HashMap<LanguageServerId, Instant>,
}

impl ProgressSpinners {
    pub fn synchronize(&mut self, servers: impl Iterator<Item = LanguageServerId>) {
        let active: std::collections::HashSet<_> = servers.collect();
        self.inner.retain(|id, _| active.contains(id));
        for id in active {
            self.inner.entry(id).or_insert_with(Instant::now);
        }
    }

    pub fn frame(&self, id: LanguageServerId) -> Option<&'static str> {
        Some(frame(self.inner.get(&id)?.elapsed()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn synchronization_keeps_active_start_times_and_removes_finished_servers() {
        let server = LanguageServerId::default();
        let started = Instant::now() - Duration::from_secs(1);
        let mut spinners = ProgressSpinners {
            inner: HashMap::from([(server, started)]),
        };

        spinners.synchronize([server].into_iter());
        assert_eq!(spinners.inner[&server], started);
        assert!(spinners.frame(server).is_some());

        spinners.synchronize(std::iter::empty());
        assert_eq!(spinners.frame(server), None);

        spinners.synchronize([server].into_iter());
        assert!(spinners.inner[&server] > started);
    }
}
