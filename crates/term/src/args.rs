use anyhow::Result;
use editor_core::Position;
use indexmap::IndexMap;
use std::path::{Path, PathBuf};
use view::tree::Layout;

#[derive(Default)]
pub struct Args {
    pub display_help: bool,
    pub display_version: bool,
    pub health: bool,
    pub health_arg: Option<String>,
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
    pub files: IndexMap<PathBuf, Vec<Position>>,
    pub stdin_position: Position,
    pub working_directory: Option<PathBuf>,
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

        let mut insert_file_with_position = |file_with_position: &str| {
            let (filename, position) = parse_file(file_with_position);

            // Before setting the working directory, resolve all the paths in args.files
            let filename = stdx::path::canonicalize(filename);

            args.files
                .entry(filename)
                .and_modify(|positions| positions.push(position))
                .or_insert_with(|| vec![position]);
        };

        argv.next(); // skip the program, we don't care about that

        while let Some(arg) = argv.next() {
            match arg.as_str() {
                "--" => break, // stop parsing at this point treat the remaining as files
                "--version" => args.display_version = true,
                "--help" => args.display_help = true,
                "--strict" => args.strict = true,
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
                "+" => line_number = usize::MAX,
                arg if arg.starts_with('+') => {
                    match arg[1..].parse::<usize>() {
                        Ok(n) => line_number = n.saturating_sub(1),
                        _ => insert_file_with_position(arg),
                    };
                }
                arg => insert_file_with_position(arg),
            }
        }

        // push the remaining args, if any to the files
        for arg in argv {
            insert_file_with_position(&arg);
        }

        if line_number != 0 {
            if let Some(first_position) = args
                .files
                .first_mut()
                .and_then(|(_, positions)| positions.first_mut())
            {
                first_position.row = line_number;
            } else {
                args.stdin_position.row = line_number;
            }
        }

        anyhow::ensure!(
            args.migrate_config.is_some() == args.stdout,
            "Use --migrate-config <file> --stdout to export configuration without rewriting it"
        );
        Ok(args)
    }
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
    use super::Args;

    #[test]
    fn line_positions_apply_to_stdin_without_file_arguments() {
        for (argument, row) in [("+10", 9), ("+", usize::MAX), ("+0", 0)] {
            let args = Args::parse_args_from(["ms", argument].map(str::to_owned)).unwrap();
            assert!(args.files.is_empty());
            assert_eq!(args.stdin_position.row, row);
        }
        let args = Args::parse_args_from(["ms", "+10", "file.test"].map(str::to_owned)).unwrap();
        assert_eq!(args.files.first().unwrap().1[0].row, 9);
        assert_eq!(args.stdin_position.row, 0);
    }
}
