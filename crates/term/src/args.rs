use anyhow::Result;
use editor_core::{pos_at_coords, Position, Range, RopeSlice};
use indexmap::IndexMap;
use std::{
    ops,
    path::{Path, PathBuf},
};
use stdx::rope::{RopeExt, LINE_TYPE};
use view::tree::Layout;

#[derive(Default)]
pub struct Args {
    pub display_help: bool,
    pub display_version: bool,
    pub health: bool,
    pub health_arg: Option<String>,
    pub health_options: crate::health::Options,
    pub load_tutor: bool,
    pub fetch_grammars: bool,
    pub build_grammars: bool,
    pub strict: bool,
    pub split: Option<Layout>,
    pub verbosity: u64,
    pub log_file: Option<PathBuf>,
    pub config_file: Option<PathBuf>,
    pub migrate_config: Option<PathBuf>,
    pub stdout: bool,
    pub files: IndexMap<PathBuf, Vec<FileSelection>>,
    pub stdin_position: Position,
    pub working_directory: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FileSelection {
    Point(Position),
    Lines(ops::Range<usize>),
}

impl Default for FileSelection {
    fn default() -> Self {
        Self::Point(Position::default())
    }
}

impl FileSelection {
    pub(crate) fn into_range(self, text: RopeSlice<'_>) -> Range {
        match self {
            Self::Point(position) => Range::point(pos_at_coords(text, position, true)),
            Self::Lines(lines) => {
                let head = pos_at_coords(text, Position::new(lines.start, 0), true);
                let anchor = text.line_to_char_idx(lines.end.min(text.len_lines(LINE_TYPE)));
                Range::new(anchor, head)
            }
        }
    }

    fn set_row(&mut self, row: usize) {
        match self {
            Self::Point(position) => position.row = row,
            Self::Lines(_) => *self = Self::Point(Position::new(row, 0)),
        }
    }
}

impl Args {
    pub fn parse_args() -> Result<Args> {
        Self::parse_args_from(std::env::args())
    }

    #[allow(clippy::too_many_lines)]
    fn parse_args_from(argv: impl IntoIterator<Item = String>) -> Result<Args> {
        let mut args = Args::default();
        let mut argv = argv.into_iter().peekable();
        let mut line_number = 0;

        let mut insert_file = |arg: &str| {
            let (filename, selection) = parse_file_selection(arg);

            // Before setting the working directory, resolve all the paths in args.files
            let filename = stdx::path::canonicalize(filename);

            args.files.entry(filename).or_default().push(selection);
        };

        argv.next(); // skip the program, we don't care about that

        while let Some(arg) = argv.next() {
            match arg.as_str() {
                "--" => break, // stop parsing at this point treat the remaining as files
                "--version" => args.display_version = true,
                "--help" => args.display_help = true,
                "--strict" => args.strict = true,
                "--summary" => args.health_options.summary = true,
                "--no-color" => args.health_options.no_color = true,
                "--tutor" => args.load_tutor = true,
                "--vsplit" => match args.split {
                    Some(_) => anyhow::bail!("can only set a split once of a specific type"),
                    None => args.split = Some(Layout::Vertical),
                },
                "--hsplit" => match args.split {
                    Some(_) => anyhow::bail!("can only set a split once of a specific type"),
                    None => args.split = Some(Layout::Horizontal),
                },
                "--health" => {
                    args.health = true;
                    args.health_arg = argv.next_if(|opt| !opt.starts_with('-'));
                }
                "--migrate-config" => match argv.next().filter(|arg| !arg.starts_with("--")) {
                    Some(path) => args.migrate_config = Some(path.into()),
                    None => anyhow::bail!("--migrate-config must specify a config file"),
                },
                "--stdout" => args.stdout = true,
                "-g" | "--grammar" => match argv.next().as_deref() {
                    Some("fetch") => args.fetch_grammars = true,
                    Some("build") => args.build_grammars = true,
                    _ => {
                        anyhow::bail!("--grammar must be followed by either 'fetch' or 'build'")
                    }
                },
                "-c" | "--config" => match argv.next().as_deref() {
                    Some(path) => args.config_file = Some(path.into()),
                    None => anyhow::bail!("--config must specify a path to read"),
                },
                "--log" => match argv.next().as_deref() {
                    Some(path) => args.log_file = Some(path.into()),
                    None => anyhow::bail!("--log must specify a path to write"),
                },
                "-w" | "--working-dir" => match argv.next().as_deref() {
                    Some(path) => {
                        args.working_directory = if Path::new(path).is_dir() {
                            Some(PathBuf::from(path))
                        } else {
                            anyhow::bail!(
                                "--working-dir specified does not exist or is not a directory"
                            )
                        }
                    }
                    None => {
                        anyhow::bail!("--working-dir must specify an initial working directory")
                    }
                },
                arg if arg.starts_with("--") => {
                    anyhow::bail!("unexpected double dash argument: {}", arg)
                }
                arg if arg.starts_with('-') => {
                    let arg = arg.get(1..).unwrap().chars();
                    for chr in arg {
                        match chr {
                            'v' => args.verbosity += 1,
                            'V' => args.display_version = true,
                            'h' => args.display_help = true,
                            _ => anyhow::bail!("unexpected short arg {}", chr),
                        }
                    }
                }
                arg if args.health && args.health_arg.is_none() => {
                    args.health_arg = Some(arg.to_owned())
                }
                "+" => line_number = usize::MAX,
                arg if arg.starts_with('+') => {
                    match arg[1..].parse::<usize>() {
                        Ok(n) => line_number = n.saturating_sub(1),
                        _ => insert_file(arg),
                    };
                }
                arg => insert_file(arg),
            }
        }

        // push the remaining args, if any to the files
        for arg in argv {
            insert_file(&arg);
        }

        if line_number != 0 {
            if let Some(first_selection) = args
                .files
                .first_mut()
                .and_then(|(_, selections)| selections.first_mut())
            {
                first_selection.set_row(line_number);
            } else {
                args.stdin_position.row = line_number;
            }
        }

        anyhow::ensure!(
            args.migrate_config.is_some() == args.stdout,
            "Use --migrate-config <file> --stdout to export configuration without rewriting it"
        );
        anyhow::ensure!(
            args.health || !(args.health_options.summary || args.health_options.no_color),
            "--summary and --no-color require --health"
        );
        Ok(args)
    }
}

fn parse_file_selection(s: &str) -> (PathBuf, FileSelection) {
    if let Some((path, selection)) = split_path_line_range(s).filter(|_| !Path::new(s).exists()) {
        return (path, selection);
    }
    let (path, position) = parse_file(s);
    (path, FileSelection::Point(position))
}

fn split_path_line_range(s: &str) -> Option<(PathBuf, FileSelection)> {
    let (path, lines) = s.trim_end_matches(':').rsplit_once(':')?;
    let (start, end) = lines.split_once('-')?;
    // The 1-based inclusive end is also the zero-based index of the next line.
    let start = start.parse::<usize>().ok()?.checked_sub(1)?;
    let end_exclusive = end.parse().ok()?;
    (start < end_exclusive).then(|| (path.into(), FileSelection::Lines(start..end_exclusive)))
}

/// Parse arg into [`PathBuf`] and position.
pub(crate) fn parse_file(s: &str) -> (PathBuf, Position) {
    let def = || (PathBuf::from(s), Position::default());
    if Path::new(s).exists() {
        return def();
    }
    split_path_row_col(s)
        .or_else(|| split_path_row(s))
        .unwrap_or_else(def)
}

/// Split file.rs:10:2 into [`PathBuf`], row and col.
///
/// Does not validate if file.rs is a file or directory.
fn split_path_row_col(s: &str) -> Option<(PathBuf, Position)> {
    let mut s = s.trim_end_matches(':').rsplitn(3, ':');
    let col: usize = s.next()?.parse().ok()?;
    let row: usize = s.next()?.parse().ok()?;
    let path = s.next()?.into();
    let pos = Position::new(row.saturating_sub(1), col.saturating_sub(1));
    Some((path, pos))
}

/// Split file.rs:10 into [`PathBuf`] and row.
///
/// Does not validate if file.rs is a file or directory.
fn split_path_row(s: &str) -> Option<(PathBuf, Position)> {
    let (path, row) = s.trim_end_matches(':').rsplit_once(':')?;
    let row: usize = row.parse().ok()?;
    let path = path.into();
    let pos = Position::new(row.saturating_sub(1), 0);
    Some((path, pos))
}

#[cfg(test)]
mod tests {
    use super::{parse_file_selection, Args, FileSelection};
    use crate::{application::TestApplication, config::Config};
    use editor_core::Position;
    use std::path::PathBuf;

    #[test]
    fn health_options_work_before_and_after_the_category() {
        for arguments in [
            vec!["ms", "--health", "rust", "--summary", "--no-color"],
            vec!["ms", "--summary", "--no-color", "--health", "rust"],
            vec!["ms", "--health", "--summary", "--no-color", "rust"],
        ] {
            let args = Args::parse_args_from(arguments.into_iter().map(str::to_owned)).unwrap();
            assert!(args.health && args.health_options.summary && args.health_options.no_color);
            assert_eq!(args.health_arg.as_deref(), Some("rust"));
            assert!(args.files.is_empty());
        }
        for option in ["--summary", "--no-color"] {
            let args =
                Args::parse_args_from(["ms", "--health", option].map(str::to_owned)).unwrap();
            assert!(args.health_arg.is_none());
            assert!(Args::parse_args_from(["ms", option].map(str::to_owned)).is_err());
        }
    }

    #[test]
    fn line_positions_apply_to_stdin_without_file_arguments() {
        for (argument, row) in [("+10", 9), ("+", usize::MAX), ("+0", 0)] {
            let args = Args::parse_args_from(["ms", argument].map(str::to_owned)).unwrap();
            assert!(args.files.is_empty());
            assert_eq!(args.stdin_position.row, row);
        }
        for file in ["file.test:2:4", "file.test:2-4"] {
            let args = Args::parse_args_from(["ms", "+10", file].map(str::to_owned)).unwrap();
            let col = if file == "file.test:2:4" { 3 } else { 0 };
            assert_eq!(
                args.files.first().unwrap().1[0],
                FileSelection::Point(Position::new(9, col))
            );
            assert_eq!(args.stdin_position.row, 0);
        }
    }

    #[test]
    fn invalid_line_ranges_are_literal_paths() {
        for path in ["file.test:0-2", "file.test:4-2", "file.test:2-"] {
            assert_eq!(
                parse_file_selection(path),
                (PathBuf::from(path), FileSelection::default())
            );
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn existing_path_takes_precedence_over_line_range() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("file.test:2-4");
        std::fs::write(&path, "")?;
        assert_eq!(
            parse_file_selection(path.to_str().unwrap()),
            (path, FileSelection::default())
        );
        Ok(())
    }

    #[tokio::test]
    async fn line_ranges_select_whole_lines_at_startup() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let first = dir.path().join("first.txt");
        let second = dir.path().join("second.txt");
        std::fs::write(&first, "one\r\ntwó\r\nthree\r\nfour\r\n")?;
        std::fs::write(&second, "one\nlast")?;
        let args = Args::parse_args_from([
            "ms".into(),
            format!("{}:2-3", first.display()),
            format!("{}:1:2", first.display()),
            format!("{}:2-99", second.display()),
        ])?;
        let mut config = Config::default();
        config.editor.files.watch.enable = false;
        let mut app = TestApplication::new(
            args,
            config,
            language::syntax::Loader::new(
                toml::from_str("language = []")?,
                loader::syntax::Resources::default(),
            )?,
            workspace::trust::WorkspaceTrust::fully_trusted(loader::workspace_trust_dir()),
        )?;

        let (view, doc) = current_ref!(app.editor);
        let text = doc.text().slice(..);
        let selection = doc.selection(view.id);
        assert_eq!(selection.len(), 2);
        assert_eq!(selection.primary().fragment(text), "twó\r\nthree\r\n");
        assert_eq!(selection.primary().cursor(text), 5);
        assert_eq!(selection.iter().next().unwrap().cursor(text), 1);

        let doc = app
            .editor
            .document_by_path(stdx::path::canonicalize(second))
            .unwrap();
        let text = doc.text().slice(..);
        let range = doc.selection(view.id).primary();
        assert_eq!(range.fragment(text), "last");
        assert_eq!(range.cursor(text), 4);
        assert!(app.close().await.is_empty());
        Ok(())
    }
}
