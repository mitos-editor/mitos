// The backend emits cursor-addressed changes, so a status message need not
// appear contiguously in the byte stream. Track the cells needed for assertions.
pub(super) struct Screen {
    cells: [[char; 80]; 24],
    row: usize,
    col: usize,
    pending: String,
}

impl Default for Screen {
    fn default() -> Self {
        Self {
            cells: [[' '; 80]; 24],
            row: 0,
            col: 0,
            pending: String::new(),
        }
    }
}

impl Screen {
    pub(super) fn contains(&self, text: &str) -> bool {
        self.cells
            .iter()
            .any(|row| row.iter().collect::<String>().contains(text))
    }

    pub(super) fn contains_line(&self, text: &str) -> bool {
        self.cells
            .iter()
            .any(|row| row.iter().collect::<String>().trim_end() == text)
    }

    fn line_feed(&mut self) {
        if self.row == 23 {
            self.cells.rotate_left(1);
            self.cells[23].fill(' ');
        } else {
            self.row += 1;
        }
    }

    pub(super) fn feed(&mut self, text: &str) {
        self.pending.push_str(text);
        let mut input = std::mem::take(&mut self.pending);
        while !input.is_empty() {
            if input.starts_with("\x1b[") {
                let Some(end) = input[2..]
                    .find(|ch: char| ('@'..='~').contains(&ch))
                    .map(|index| index + 2)
                else {
                    break;
                };
                let parameters = &input[2..end];
                match input.as_bytes()[end] {
                    b'H' | b'f' => {
                        let mut values = parameters
                            .split(';')
                            .map(|value| value.parse::<usize>().unwrap_or(1).max(1) - 1);
                        self.row = values.next().unwrap_or(0).min(23);
                        self.col = values.next().unwrap_or(0).min(79);
                    }
                    b'J' if parameters == "2" => self.cells = [[' '; 80]; 24],
                    b'K' => self.cells[self.row][self.col..].fill(' '),
                    _ => {}
                }
                input.drain(..=end);
            } else if input.starts_with("\x1b]") {
                let Some(end) = input
                    .find('\x07')
                    .map(|index| index + 1)
                    .or_else(|| input.find("\x1b\\").map(|index| index + 2))
                else {
                    break;
                };
                input.drain(..end);
            } else if input.starts_with('\x1b') {
                if input.len() < 2 {
                    break;
                }
                input.drain(..2);
            } else {
                let ch = input.remove(0);
                match ch {
                    '\r' => self.col = 0,
                    '\n' => self.line_feed(),
                    ch if !ch.is_control() => {
                        // ConPTY can emit entire rows without cursor commands
                        // or newlines, relying on the terminal's delayed wrap.
                        if self.col == 80 {
                            self.col = 0;
                            self.line_feed();
                        }
                        self.cells[self.row][self.col] = ch;
                        self.col += 1;
                    }
                    _ => {}
                }
            }
        }
        self.pending = input;
    }
}

#[test]
fn conpty_wrapped_rows_preserve_status_text() {
    let mut screen = Screen::default();
    // ConPTY's startup repaint streams full rows, then the status line. There
    // need not be a CR/LF or cursor-position command between those rows.
    screen.feed(&" ".repeat(80 * 22));
    screen.feed(" NOR   sample.txt");
    assert!(screen.contains("NOR"));
    screen.feed("\x1b[24;1Hcompleted 3");
    assert!(screen.contains_line("completed 3"));
}
