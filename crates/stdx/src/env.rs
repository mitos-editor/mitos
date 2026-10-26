//! Functions for working with the host environment.
use std::{
    borrow::Cow,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    sync::{LazyLock, RwLock},
};

// We keep the CWD as a static so that we can access it in places where we don't have access to the Editor
static CWD: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Get the current working directory.
/// This information is managed internally as the call to std::env::current_dir
/// might fail if the cwd has been deleted.
pub fn current_working_dir() -> PathBuf {
    if let Some(path) = &*CWD.read().unwrap() {
        return path.clone();
    }

    // implementation of crossplatform pwd -L
    // we want pwd -L so that symlinked directories are handled correctly
    let pwd = std::env::var_os("PWD");
    #[cfg(windows)]
    let pwd = pwd.or_else(|| std::env::var_os("CD"));

    let mut cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(_) => return fallback_working_dir(pwd),
    };

    if let Some(pwd) = pwd.map(PathBuf::from)
        && pwd.canonicalize().ok().as_ref() == Some(&cwd)
    {
        cwd = pwd;
    }
    let mut dst = CWD.write().unwrap();
    *dst = Some(cwd.clone());

    cwd
}

fn fallback_working_dir(pwd: Option<OsString>) -> PathBuf {
    if let Some(pwd) = pwd.map(PathBuf::from) {
        if pwd.is_absolute() {
            return pwd;
        }
    }
    panic!(
        "Couldn't determine the current working directory: the cwd is gone and \
         PWD is unset or relative. Pass an initial directory with `--working-dir`."
    )
}

/// Update the current working directory.
pub fn set_current_working_dir(path: impl AsRef<Path>) -> std::io::Result<Option<PathBuf>> {
    let path = crate::path::canonicalize(path);
    std::env::set_current_dir(&path)?;
    let mut cwd = CWD.write().unwrap();

    Ok(cwd.replace(path))
}

/// Checks if the given environment variable is set.
pub fn env_var_is_set(env_var_name: &str) -> bool {
    std::env::var_os(env_var_name).is_some()
}

/// Checks if a binary with the given name exists.
pub fn binary_exists<T: AsRef<OsStr>>(binary_name: T) -> bool {
    which::which(binary_name).is_ok()
}

/// Attempts to find a binary of the given name. See [which](https://linux.die.net/man/1/which).
pub fn which<T: AsRef<OsStr>>(
    binary_name: T,
) -> Result<std::path::PathBuf, ExecutableNotFoundError> {
    let binary_name = binary_name.as_ref();
    which::which(binary_name).map_err(|err| ExecutableNotFoundError {
        command: binary_name.to_string_lossy().into_owned(),
        inner: err,
    })
}

fn find_brace_end(src: &[u8]) -> Option<usize> {
    use regex_automata::meta::Regex;

    static REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::builder().build("[{}]").unwrap());
    let mut depth = 0;
    for mat in REGEX.find_iter(src) {
        let pos = mat.start();
        match src[pos] {
            b'{' => depth += 1,
            b'}' if depth == 0 => return Some(pos),
            b'}' => depth -= 1,
            _ => unreachable!(),
        }
    }
    None
}

fn expand_impl(src: &OsStr, mut resolve: impl FnMut(&OsStr) -> Option<OsString>) -> Cow<'_, OsStr> {
    use regex_automata::meta::Regex;

    static REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::builder()
            .build_many(&[
                r"\$\{([^\}:]+):-",
                r"\$\{([^\}:]+):=",
                r"\$\{([^\}-]+)-",
                r"\$\{([^\}=]+)=",
                r"\$\{([^\}]+)",
                r"\$(\w+)",
            ])
            .unwrap()
    });

    let bytes = src.as_encoded_bytes();
    let mut res = Vec::with_capacity(bytes.len());
    let mut pos = 0;
    for captures in REGEX.captures_iter(bytes) {
        let mat = captures.get_match().unwrap();
        let pattern_id = mat.pattern().as_usize();
        let mut range = mat.range();
        // A pattern may match multiple times on a single variable, for example `${HOME:-$HOME}`:
        // `${HOME:-` matches and also the default value (`$HOME`). Skip past any variables which
        // have already been expanded.
        if range.start < pos {
            continue;
        }
        let var = &bytes[captures.get_group(1).unwrap().range()];
        let default = if pattern_id != 5 {
            let Some(bracket_pos) = find_brace_end(&bytes[range.end..]) else {
                break;
            };
            let default = &bytes[range.end..range.end + bracket_pos];
            range.end += bracket_pos + 1;
            default
        } else {
            &[]
        };
        // safety: this is a codepoint aligned substring of an osstr (always valid)
        let var = unsafe { OsStr::from_encoded_bytes_unchecked(var) };
        let expansion = resolve(var);
        let expansion = match &expansion {
            Some(val) => {
                if val.is_empty() && pattern_id < 2 {
                    default
                } else {
                    val.as_encoded_bytes()
                }
            }
            None => default,
        };
        res.extend_from_slice(&bytes[pos..range.start]);
        pos = range.end;
        res.extend_from_slice(expansion);
    }
    if pos == 0 {
        src.into()
    } else {
        res.extend_from_slice(&bytes[pos..]);
        // safety: this is a composition of valid osstr (and codepoint aligned slices which are also valid)
        unsafe { OsString::from_encoded_bytes_unchecked(res) }.into()
    }
}

/// performs substitution of enviorment variables. Supports the following (POSIX) syntax:
///
/// * `$<var>`, `${<var>}`
/// * `${<var>:-<default>}`, `${<var>-<default>}`
/// * `${<var>:=<default>}`, `${<var>=default}`
///
pub fn expand<S: AsRef<OsStr> + ?Sized>(src: &S) -> Cow<'_, OsStr> {
    expand_impl(src.as_ref(), |var| std::env::var_os(var))
}

#[derive(Debug)]
pub struct ExecutableNotFoundError {
    command: String,
    inner: which::Error,
}

impl std::fmt::Display for ExecutableNotFoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "command '{}' not found: {}", self.command, self.inner)
    }
}

impl std::error::Error for ExecutableNotFoundError {}

#[cfg(test)]
mod tests {
    #[test]
    fn fallback_working_dir_prefers_absolute_pwd() {
        let pwd = PathBuf::from(if cfg!(windows) {
            r"C:\some\path"
        } else {
            "/some/path"
        });
        assert_eq!(
            fallback_working_dir(Some(pwd.clone().into_os_string())),
            pwd
        );
    }
    #[test]
    #[should_panic]
    fn fallback_working_dir_panics_on_relative_pwd() {
        // A relative PWD is useless as a working directory. There is nothing
        // sensible to return, so we panic instead of inventing a directory.
        fallback_working_dir(Some(OsString::from("relative/path")));
    }

    #[test]
    #[should_panic]
    fn fallback_working_dir_panics_without_pwd() {
        fallback_working_dir(None);
    }

    use std::{
        ffi::{OsStr, OsString},
        path::PathBuf,
    };

    use super::{current_working_dir, expand_impl, fallback_working_dir, set_current_working_dir};

    #[test]
    fn current_dir_is_set() {
        // Changing the process directory and its cache must not race with path tests.
        const CHILD: &str = "MITOS_STDX_TEST_CURRENT_DIR";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "env::tests::current_dir_is_set", "--nocapture"])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "isolated current-directory test failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let new_path = dunce::canonicalize(dir.path()).unwrap();
        let cwd = current_working_dir();
        assert_ne!(cwd, new_path);

        set_current_working_dir(&new_path).expect("Couldn't set new path");

        assert_eq!(current_working_dir(), new_path);
        set_current_working_dir(&cwd).expect("Couldn't restore original path");
    }

    macro_rules! assert_env_expand {
        ($env: expr, $lhs: expr, $rhs: expr) => {
            assert_eq!(&*expand_impl($lhs.as_ref(), $env), OsStr::new($rhs));
        };
    }

    /// paths that should work on all platforms
    #[test]
    fn test_env_expand() {
        let env = |var: &OsStr| -> Option<OsString> {
            match var.to_str().unwrap() {
                "FOO" => Some("foo".into()),
                "EMPTY" => Some("".into()),
                _ => None,
            }
        };
        assert_env_expand!(env, "pass_trough", "pass_trough");
        assert_env_expand!(env, "$FOO", "foo");
        assert_env_expand!(env, "bar/$FOO/baz", "bar/foo/baz");
        assert_env_expand!(env, "bar/${FOO}/baz", "bar/foo/baz");
        assert_env_expand!(env, "baz/${BAR:-bar}/foo", "baz/bar/foo");
        assert_env_expand!(env, "baz/${FOO:-$FOO}/foo", "baz/foo/foo");
        assert_env_expand!(env, "baz/${BAR:=bar}/foo", "baz/bar/foo");
        assert_env_expand!(env, "baz/${BAR-bar}/foo", "baz/bar/foo");
        assert_env_expand!(env, "baz/${BAR=bar}/foo", "baz/bar/foo");
        assert_env_expand!(env, "baz/${EMPTY:-bar}/foo", "baz/bar/foo");
        assert_env_expand!(env, "baz/${EMPTY:=bar}/foo", "baz/bar/foo");
        assert_env_expand!(env, "baz/${EMPTY-bar}/foo", "baz//foo");
        assert_env_expand!(env, "baz/${EMPTY=bar}/foo", "baz//foo");
    }
}
