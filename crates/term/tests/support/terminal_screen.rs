//! Screen assertions for native PTY and ConPTY frontend tests.

#[derive(Default)]
pub(super) struct Screen(vt100::Parser);

impl Screen {
    pub(super) fn contains(&self, text: &str) -> bool {
        self.0.screen().rows(0, 80).any(|row| row.contains(text))
    }

    pub(super) fn contains_line(&self, text: &str) -> bool {
        self.0
            .screen()
            .rows(0, 80)
            .any(|row| row.trim_end() == text)
    }

    pub(super) fn resize(&mut self, rows: u16, cols: u16) {
        self.0
            .set_size(rows.try_into().unwrap(), cols.try_into().unwrap());
    }

    pub(super) fn feed(&mut self, bytes: &[u8]) {
        self.0.process(bytes);
    }
}
