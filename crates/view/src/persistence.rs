//! Editor state kept between sessions: file positions, `:` and search history,
//! and the default yank register. Configured by `[editor.persistence]`.
//!
//! Each kind of state is one small JSON file in the state directory. Files are read
//! at startup and written at exit. Writes merge with the file on disk, so several
//! editors that exit one after another keep each other's entries, and go through a
//! temporary file and a rename, so an interrupted write never leaves a broken file.
//! A missing or unreadable file only loses that kind of state.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
};

use editor_config::PersistenceConfig;
use editor_core::{Range, Selection};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{register::Registers, view::ViewPosition};

const FILE_POSITIONS: &str = "file-positions.json";
const COMMAND_HISTORY: &str = "command-history.json";
const SEARCH_HISTORY: &str = "search-history.json";
const CLIPBOARD: &str = "clipboard.json";

/// Where a file was left: its selection and its scroll position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilePosition {
    pub path: PathBuf,
    /// `(anchor, head)` character offsets of each range.
    pub ranges: Vec<(usize, usize)>,
    pub primary: usize,
    pub anchor: usize,
    pub horizontal_offset: usize,
    pub vertical_offset: usize,
}

impl FilePosition {
    pub fn new(path: PathBuf, selection: &Selection, view: ViewPosition) -> Self {
        Self {
            path,
            ranges: selection.iter().map(|r| (r.anchor, r.head)).collect(),
            primary: selection.primary_index(),
            anchor: view.anchor,
            horizontal_offset: view.horizontal_offset,
            vertical_offset: view.vertical_offset,
        }
    }

    /// The saved selection if it still fits a text of `len` characters.
    pub fn selection(&self, len: usize) -> Option<Selection> {
        if self.ranges.is_empty()
            || self.primary >= self.ranges.len()
            || self.ranges.iter().any(|&(a, h)| a.max(h) > len)
        {
            return None;
        }
        let ranges = self.ranges.iter().map(|&(a, h)| Range::new(a, h)).collect();
        Some(Selection::new(ranges, self.primary))
    }

    pub fn view_position(&self) -> ViewPosition {
        ViewPosition {
            anchor: self.anchor,
            horizontal_offset: self.horizontal_offset,
            vertical_offset: self.vertical_offset,
        }
    }
}

/// File positions known to this editor, plus the ones it changed this session.
#[derive(Debug, Default)]
pub struct Persistence {
    dir: Option<PathBuf>,
    positions: HashMap<PathBuf, FilePosition>,
    /// Positions recorded this session, oldest first, merged into the file at exit.
    recorded: Vec<FilePosition>,
    exclusions: Option<globset::GlobSet>,
}

impl Persistence {
    /// Read the enabled state from `dir` into `registers`, keeping file positions here.
    pub fn load(dir: PathBuf, config: &PersistenceConfig, registers: &mut Registers) -> Self {
        let mut persistence = Self {
            exclusions: exclusions(&config.old_files_exclusions),
            ..Self::default()
        };
        if config.old_files {
            persistence.positions = read::<Vec<FilePosition>>(&dir.join(FILE_POSITIONS))
                .into_iter()
                .map(|position| (position.path.clone(), position))
                .collect();
        }
        let history = |name: &str, register: char, registers: &mut Registers| {
            let mut entries: Vec<String> = read(&dir.join(name));
            if !entries.is_empty() {
                // Files hold the oldest entry first; registers take the newest first.
                entries.reverse();
                let _ = registers.write(register, entries);
            }
        };
        if config.commands {
            history(COMMAND_HISTORY, ':', registers);
        }
        if config.search {
            history(SEARCH_HISTORY, '/', registers);
        }
        if let Some(register) = clipboard_register(config) {
            history(CLIPBOARD, register, registers);
        }
        persistence.dir = Some(dir);
        persistence
    }

    fn excluded(&self, path: &Path) -> bool {
        self.exclusions
            .as_ref()
            .is_some_and(|set| set.is_match(path))
    }

    /// Where `path` was left, unless its pattern is excluded.
    pub fn position(&self, path: &Path) -> Option<&FilePosition> {
        (!self.excluded(path)).then(|| self.positions.get(path))?
    }

    /// Remember where a file was left, for reopening it later.
    pub fn record(&mut self, position: FilePosition) {
        if self.dir.is_none() || self.excluded(&position.path) {
            return;
        }
        self.recorded.retain(|old| old.path != position.path);
        self.positions
            .insert(position.path.clone(), position.clone());
        self.recorded.push(position);
    }

    /// Write the enabled state. Does nothing unless [`Self::load`] ran.
    pub fn save(&mut self, config: &PersistenceConfig, registers: &Registers) -> io::Result<()> {
        let Some(dir) = self.dir.clone() else {
            return Ok(());
        };
        fs::create_dir_all(&dir)?;
        if config.old_files {
            let path = dir.join(FILE_POSITIONS);
            let mut positions: Vec<FilePosition> = read(&path);
            for position in self.recorded.drain(..) {
                positions.retain(|old| old.path != position.path);
                positions.push(position);
            }
            keep_newest(&mut positions, config.old_files_trim);
            write(&path, &positions)?;
        }
        let history = |name: &str, register: char, limit: usize| {
            let path = dir.join(name);
            let mut entries: Vec<String> = read(&path);
            for entry in register_values(registers, register) {
                entries.retain(|old| old != &entry);
                entries.push(entry);
            }
            keep_newest(&mut entries, limit);
            write(&path, &entries)
        };
        if config.commands {
            history(COMMAND_HISTORY, ':', config.commands_trim)?;
        }
        if config.search {
            history(SEARCH_HISTORY, '/', config.search_trim)?;
        }
        if let Some(register) = clipboard_register(config) {
            // The clipboard is one value set: the last editor to exit wins.
            write(&dir.join(CLIPBOARD), &register_values(registers, register))?;
        }
        Ok(())
    }
}

/// The `"` register; the `*` and `+` registers are kept by the system clipboard.
fn clipboard_register(config: &PersistenceConfig) -> Option<char> {
    config.clipboard.then_some('"')
}

/// Register values oldest first, as the state files store them.
fn register_values(registers: &Registers, register: char) -> Vec<String> {
    let mut values: Vec<String> = registers
        .read(register)
        .map(|values| values.map(|value| value.into_owned()).collect())
        .unwrap_or_default();
    values.reverse();
    values
}

fn keep_newest<T>(entries: &mut Vec<T>, limit: usize) {
    let excess = entries.len().saturating_sub(limit);
    entries.drain(..excess);
}

fn exclusions(patterns: &[String]) -> Option<globset::GlobSet> {
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        match globset::Glob::new(pattern) {
            Ok(glob) => {
                builder.add(glob);
            }
            Err(error) => log::warn!("ignoring persistence exclusion {pattern:?}: {error}"),
        }
    }
    builder.build().ok()
}

fn read<T: DeserializeOwned + Default>(path: &Path) -> T {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            log::warn!("ignoring unreadable state file {}: {error}", path.display());
            T::default()
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => T::default(),
        Err(error) => {
            log::warn!("cannot read state file {}: {error}", path.display());
            T::default()
        }
    }
}

fn write<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut file, value)?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registers() -> Registers {
        Registers::new(crate::clipboard::Clipboard::native(Box::new(
            arc_swap::access::Constant(clipboard::ClipboardProvider::None),
        )))
    }

    /// Push `values` oldest first, as a session would.
    fn push(registers: &mut Registers, register: char, values: &[&str]) {
        for value in values {
            registers.push(register, (*value).into()).unwrap();
        }
    }

    fn enabled() -> PersistenceConfig {
        PersistenceConfig {
            old_files: true,
            commands: true,
            search: true,
            clipboard: true,
            ..Default::default()
        }
    }

    fn position(path: &str, cursor: usize) -> FilePosition {
        FilePosition::new(
            path.into(),
            &Selection::point(cursor),
            ViewPosition::default(),
        )
    }

    #[test]
    fn state_round_trips_through_the_state_directory() {
        let dir = tempfile::tempdir().unwrap();
        let config = enabled();
        let mut registers = registers();
        let mut first = Persistence::load(dir.path().into(), &config, &mut registers);
        first.record(position("/src/a.rs", 4));
        push(&mut registers, ':', &["w", "q"]);
        push(&mut registers, '/', &["needle"]);
        push(&mut registers, '"', &["one", "two"]);
        first.save(&config, &registers).unwrap();

        let mut registers = self::registers();
        let second = Persistence::load(dir.path().into(), &config, &mut registers);
        assert_eq!(
            second.position(Path::new("/src/a.rs")),
            Some(&position("/src/a.rs", 4))
        );
        assert_eq!(register_values(&registers, ':'), ["w", "q"]);
        assert_eq!(register_values(&registers, '/'), ["needle"]);
        assert_eq!(register_values(&registers, '"'), ["one", "two"]);
    }

    #[test]
    fn saving_merges_with_other_editors_and_keeps_the_newest_entries() {
        let dir = tempfile::tempdir().unwrap();
        let config = PersistenceConfig {
            old_files_trim: 2,
            commands_trim: 3,
            ..enabled()
        };
        let mut registers_a = registers();
        let mut registers_b = registers();
        let mut a = Persistence::load(dir.path().into(), &config, &mut registers_a);
        let mut b = Persistence::load(dir.path().into(), &config, &mut registers_b);
        a.record(position("/a", 1));
        b.record(position("/b", 2));
        b.record(position("/c", 3));
        push(&mut registers_a, ':', &["one", "two"]);
        push(&mut registers_b, ':', &["two", "three"]);
        a.save(&config, &registers_a).unwrap();
        b.save(&config, &registers_b).unwrap();

        let mut registers = self::registers();
        let loaded = Persistence::load(dir.path().into(), &config, &mut registers);
        // `/a` was saved first, so trimming to two drops it.
        assert!(loaded.position(Path::new("/a")).is_none());
        assert!(loaded.position(Path::new("/b")).is_some());
        assert!(loaded.position(Path::new("/c")).is_some());
        assert_eq!(register_values(&registers, ':'), ["one", "two", "three"]);
    }

    #[test]
    fn disabled_state_is_neither_read_nor_written() {
        let dir = tempfile::tempdir().unwrap();
        let mut registers = registers();
        let mut persistence = Persistence::load(
            dir.path().into(),
            &PersistenceConfig::default(),
            &mut registers,
        );
        persistence.record(position("/a", 1));
        push(&mut registers, ':', &["q"]);
        persistence
            .save(&PersistenceConfig::default(), &registers)
            .unwrap();
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        // Without `load`, nothing is recorded or written at all.
        let mut unloaded = Persistence::default();
        unloaded.record(position("/a", 1));
        unloaded.save(&enabled(), &registers).unwrap();
        assert!(unloaded.position(Path::new("/a")).is_none());
    }

    #[test]
    fn excluded_and_broken_state_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(COMMAND_HISTORY), "not json").unwrap();
        let config = enabled();
        let mut registers = registers();
        let mut persistence = Persistence::load(dir.path().into(), &config, &mut registers);
        assert!(registers.read(':').is_none());
        persistence.record(position("/repo/.git/COMMIT_EDITMSG", 1));
        assert!(persistence
            .position(Path::new("/repo/.git/COMMIT_EDITMSG"))
            .is_none());
    }

    #[test]
    fn saved_selections_must_fit_the_current_text() {
        let saved = FilePosition::new(
            "/a".into(),
            &Selection::new(
                [Range::new(1, 3), Range::new(5, 8)].into_iter().collect(),
                1,
            ),
            ViewPosition::default(),
        );
        assert_eq!(saved.selection(8).unwrap().primary(), Range::new(5, 8));
        assert!(saved.selection(7).is_none());
    }
}
