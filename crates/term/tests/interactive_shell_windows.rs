//! Exercise the Windows frontend in a real console, including on headless CI.
#![cfg(all(windows, not(feature = "integration")))]

#[path = "support/terminal_screen.rs"]
mod terminal_screen;
use terminal_screen::Screen;

use anyhow::ensure;
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, IsTerminal, Read, Write},
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    ptr,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::WAIT_OBJECT_0,
    System::{
        Console::{
            ClosePseudoConsole, CreatePseudoConsole, GetConsoleMode, ResizePseudoConsole,
            SetConsoleMode, COORD, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
            HPCON,
        },
        Pipes::CreatePipe,
        Threading::{
            CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
            InitializeProcThreadAttributeList, OpenProcess, TerminateProcess,
            UpdateProcThreadAttribute, WaitForSingleObject, CREATE_UNICODE_ENVIRONMENT,
            EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTUPINFOEXW,
        },
    },
};

fn check(success: i32) -> io::Result<()> {
    if success == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
    text.as_ref().encode_wide().chain(Some(0)).collect()
}

fn pipe() -> io::Result<(File, File)> {
    let mut read = ptr::null_mut();
    let mut write = ptr::null_mut();
    // SAFETY: CreatePipe initializes both handles on success. The files take ownership.
    check(unsafe { CreatePipe(&mut read, &mut write, ptr::null(), 0) })?;
    Ok(unsafe { (File::from_raw_handle(read), File::from_raw_handle(write)) })
}

struct Console(HPCON);

impl Drop for Console {
    fn drop(&mut self) {
        // SAFETY: this guard owns the pseudoconsole. Its output reader remains
        // alive until ClosePseudoConsole finishes draining console output.
        unsafe { ClosePseudoConsole(self.0) };
    }
}

struct Attributes(Vec<usize>);

impl Attributes {
    fn new(console: HPCON) -> io::Result<Self> {
        let mut size = 0;
        // SAFETY: the first call queries the required allocation size.
        unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size) };
        let mut buffer = vec![0usize; size.div_ceil(size_of::<usize>())];
        // SAFETY: usize storage provides sufficient alignment and space.
        check(unsafe {
            InitializeProcThreadAttributeList(buffer.as_mut_ptr().cast(), 1, 0, &mut size)
        })?;
        let mut attributes = Self(buffer);
        // SAFETY: the pseudoconsole attribute takes the handle value, rather
        // than its address. The console outlives process creation.
        check(unsafe {
            UpdateProcThreadAttribute(
                attributes.0.as_mut_ptr().cast(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                console as *const _,
                size_of::<HPCON>(),
                ptr::null_mut(),
                ptr::null(),
            )
        })?;
        Ok(attributes)
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: this is the initialized attribute list owned by this guard.
        unsafe { DeleteProcThreadAttributeList(self.0.as_mut_ptr().cast()) };
    }
}

struct Editor {
    process: OwnedHandle,
    console: Option<Console>,
    input: Option<File>,
    reader: Option<thread::JoinHandle<()>>,
    receiver: mpsc::Receiver<Vec<u8>>,
    dir: tempfile::TempDir,
    output: String,
    screen: Screen,
}

impl Editor {
    fn new() -> anyhow::Result<Self> {
        Self::with_shell(&["cmd", "/C"])
    }

    fn with_shell(shell: &[&str]) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        std::fs::write(
            dir.path().join("config.toml"),
            format!(
                r#"[editor]
mouse = false
shell = {}
[commands]
":outline" = [":goto %sh_interactive{{%arg{{0}}}}", ":echo completed %{{cursor_line}}"]
":discard" = [":noop %sh_interactive{{%arg{{0}}}}", ":echo completed %{{cursor_line}}"]
":nested" = [":goto %sh_interactive{{echo %sh{{echo 2}}}}", ":echo completed %{{cursor_line}}"]
"#,
                serde_json::to_string(shell)?
            ),
        )?;
        std::fs::write(dir.path().join("sample.txt"), "one\ntwo\nthree\nfour\n")?;
        let (input_read, input) = pipe()?;
        let (mut output_read, output_write) = pipe()?;
        let mut console = 0;
        // SAFETY: the pipes remain live during creation. Console owns the returned handle.
        let result = unsafe {
            CreatePseudoConsole(
                COORD { X: 80, Y: 24 },
                input_read.as_raw_handle(),
                output_write.as_raw_handle(),
                0,
                &mut console,
            )
        };
        ensure!(result >= 0, "CreatePseudoConsole: {result:#x}");
        let console = Console(console);
        drop(input_read);
        drop(output_write);
        let (sender, receiver) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut buffer = [0; 16384];
            while let Ok(count) = output_read.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                let _ = sender.send(buffer[..count].to_vec());
            }
        });
        let mut attributes = Attributes::new(console.0)?;
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.lpAttributeList = attributes.0.as_mut_ptr().cast();
        let mut process = PROCESS_INFORMATION::default();
        let executable = wide(env!("CARGO_BIN_EXE_ms"));
        // Only the executable path needs quoting; all following arguments are
        // fixed relative paths inside this test's private working directory.
        let mut command = wide(format!(
            "\"{}\" --config config.toml --log mitos.log sample.txt",
            env!("CARGO_BIN_EXE_ms")
        ));
        let mut environment: Vec<_> = std::env::vars_os()
            .filter(|(name, _)| {
                !name.eq_ignore_ascii_case(OsStr::new("MITOS_CONFIG"))
                    && !name.eq_ignore_ascii_case(OsStr::new("MITOS_RUNTIME"))
            })
            .collect();
        environment.push(("MITOS_CONFIG".into(), dir.path().as_os_str().to_owned()));
        environment.push((
            "MITOS_RUNTIME".into(),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../runtime")
                .into_os_string(),
        ));
        environment.sort_by_key(|(name, _)| name.to_string_lossy().to_lowercase());
        let mut environment_block = Vec::new();
        for (name, value) in environment {
            environment_block.extend(name.encode_wide());
            environment_block.push(b'=' as u16);
            environment_block.extend(value.encode_wide());
            environment_block.push(0);
        }
        environment_block.push(0);
        // SAFETY: all UTF-16 strings are terminated and live through this call;
        // startup contains a valid pseudoconsole attribute and writable result.
        check(unsafe {
            CreateProcessW(
                executable.as_ptr(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
                environment_block.as_ptr().cast(),
                wide(dir.path()).as_ptr(),
                &startup.StartupInfo,
                &mut process,
            )
        })?;
        // SAFETY: process creation transferred both fresh handles to us.
        let (process, main_thread) = unsafe {
            (
                OwnedHandle::from_raw_handle(process.hProcess),
                OwnedHandle::from_raw_handle(process.hThread),
            )
        };
        drop(main_thread);
        let mut editor = Self {
            process,
            console: Some(console),
            input: Some(input),
            reader: Some(reader),
            receiver,
            dir,
            output: String::new(),
            screen: Screen::default(),
        };
        editor.wait("editor startup", |editor| editor.screen.contains("NOR"))?;
        Ok(editor)
    }

    fn send(&mut self, input: &str) -> anyhow::Result<()> {
        self.input.as_mut().unwrap().write_all(input.as_bytes())?;
        Ok(())
    }

    fn drain(&mut self) -> anyhow::Result<()> {
        while let Ok(bytes) = self.receiver.try_recv() {
            let output = String::from_utf8_lossy(&bytes);
            if output.contains("\x1b[6n") {
                self.send("\x1b[1;1R")?;
            }
            if output.contains("\x1b[c") {
                self.send("\x1b[?1;2c")?;
            }
            self.screen.feed(&output);
            self.output.push_str(&output);
        }
        Ok(())
    }

    fn wait(
        &mut self,
        description: &str,
        mut condition: impl FnMut(&Self) -> bool,
    ) -> anyhow::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            self.drain()?;
            if condition(self) {
                return Ok(());
            }
            let mut exit = 0;
            // SAFETY: the editor process handle and exit pointer are valid.
            check(unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut exit) })?;
            ensure!(
                Instant::now() < deadline && exit == 259,
                "waiting for {description}; exit: {exit}; output: {:?}; log: {}",
                self.output.chars().rev().take(2000).collect::<String>(),
                std::fs::read_to_string(self.dir.path().join("mitos.log")).unwrap_or_default()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn interactive(&mut self, command: &str) -> anyhow::Result<()> {
        self.output.clear();
        self.send(&format!(":outline '{}'\r", command.replace('\'', "''")))
    }

    fn fixture(&mut self, mode: &str) -> anyhow::Result<()> {
        // Normal cases filter libtest's progress output into a selected line.
        // The descendant case must let cmd exit before stdout reaches EOF, so
        // run without a pipeline and discard the expansion's captured value.
        let (command, filter) = if mode == "descendant" {
            ("discard", "")
        } else {
            ("outline", " | findstr /R \"^[0-9]$\"")
        };
        let script = format!(
            "set MITOS_TEST_INTERACTIVE_MODE={mode}&& \"{}\" --exact fixture_process --nocapture --quiet{filter}",
            std::env::current_exe()?.display());
        self.output.clear();
        self.send(&format!(":{command} '{}'\r", script.replace('\'', "''")))?;
        self.wait("interactive child input", |editor| {
            editor.output.contains("CHILD_READY")
        })
    }

    fn assert_line(&mut self, line: &str) -> anyhow::Result<()> {
        self.send(":echo cursor-line: %{cursor_line}\r")?;
        self.wait("editor input and command continuation", |editor| {
            editor.screen.contains_line(&format!("cursor-line: {line}"))
        })
    }

    fn wait_result(&mut self, line: &str) -> anyhow::Result<()> {
        self.wait("interactive result", |editor| {
            editor.screen.contains_line(&format!("completed {line}"))
        })?;
        self.assert_line(line)
    }

    fn child_handle(&self, name: &str) -> anyhow::Result<OwnedHandle> {
        let id = std::fs::read_to_string(self.dir.path().join(name))?.parse()?;
        // SAFETY: request only observation rights; the returned handle is owned.
        let handle = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                id,
            )
        };
        ensure!(
            !handle.is_null(),
            "OpenProcess: {}",
            io::Error::last_os_error()
        );
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        // SAFETY: terminate only this test's editor, then let ConPTY drain while
        // its output reader is still running. Closing the job kills descendants.
        unsafe { TerminateProcess(self.process.as_raw_handle(), 1) };
        drop(self.input.take());
        drop(self.console.take());
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
    }
}

#[test]
fn fixture_process() -> anyhow::Result<()> {
    let Ok(mode) = std::env::var("MITOS_TEST_INTERACTIVE_MODE") else {
        return Ok(());
    };
    std::fs::write(format!("{mode}.pid"), std::process::id().to_string())?;
    if mode == "wait" {
        thread::sleep(Duration::from_secs(120));
        std::process::exit(0);
    }
    ensure!(
        std::io::stdin().is_terminal()
            && std::io::stderr().is_terminal()
            && !std::io::stdout().is_terminal(),
        "incorrect interactive stdio"
    );
    let input = File::options().read(true).write(true).open("CONIN$")?;
    let mut console = File::options().read(true).write(true).open("CONOUT$")?;
    let mut mode_flags = 0;
    // SAFETY: the input console handle and mode pointer are valid.
    check(unsafe { GetConsoleMode(input.as_raw_handle(), &mut mode_flags) })?;
    let cooked = ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT;
    ensure!(
        mode_flags & cooked == cooked,
        "editor did not release console modes"
    );
    if matches!(mode.as_str(), "tree" | "descendant") {
        let _child = std::process::Command::new(std::env::current_exe()?)
            .args(["--exact", "fixture_process", "--nocapture", "--quiet"])
            .env("MITOS_TEST_INTERACTIVE_MODE", "wait")
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while !Path::new("wait.pid").exists() {
            ensure!(Instant::now() < deadline, "descendant did not start");
            thread::sleep(Duration::from_millis(5));
        }
    }
    console.write_all(b"CHILD_READY\r\n")?;
    console.flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    if mode == "modes" {
        // Deliberately leave input raw. The next handoff must still start cooked.
        check(unsafe { SetConsoleMode(input.as_raw_handle(), mode_flags & !cooked) })?;
    }
    println!("\n{}", line.trim());
    std::process::exit(0);
}

#[test]
fn interactive_input_output_resize_and_nested_expansions() -> anyhow::Result<()> {
    let mut editor = Editor::new()?;
    editor.fixture("read")?;
    let before = editor.output.clone();
    // SAFETY: resize this test's live pseudoconsole.
    let result =
        unsafe { ResizePseudoConsole(editor.console.as_ref().unwrap().0, COORD { X: 80, Y: 20 }) };
    ensure!(result >= 0, "ResizePseudoConsole: {result:#x}");
    thread::sleep(Duration::from_millis(150));
    editor.drain()?;
    ensure!(
        editor.output == before,
        "editor drew over interactive child"
    );
    editor.send("3\r")?;
    editor.wait_result("3")?;
    editor.send(":nested\r")?;
    editor.wait_result("2")
}

#[test]
fn failures_ctrl_c_and_child_mode_changes_restore_the_editor() -> anyhow::Result<()> {
    let mut editor = Editor::new()?;
    editor.interactive("exit /b 7")?;
    editor.wait("failed child", |editor| {
        editor.screen.contains("Interactive shell command failed")
    })?;
    editor.assert_line("1")?;
    editor.fixture("read")?;
    editor.send("\x03")?;
    editor.wait("Ctrl-c", |editor| {
        editor.screen.contains("Interactive shell command failed")
    })?;
    editor.assert_line("1")?;
    for mode in ["modes", "read"] {
        editor.fixture(mode)?;
        editor.send("3\r")?;
        editor.wait_result("3")?;
    }
    Ok(())
}

#[test]
fn spawn_failure_restores_input() -> anyhow::Result<()> {
    let mut editor = Editor::with_shell(&["mitos-missing-interactive-shell.exe"])?;
    editor.interactive("echo 4")?;
    editor.wait("spawn failure", |editor| {
        editor.screen.contains("Could not start interactive shell")
    })?;
    editor.assert_line("1")
}

#[test]
fn shell_exit_kills_descendants_holding_stdout_open() -> anyhow::Result<()> {
    let mut editor = Editor::new()?;
    editor.fixture("descendant")?;
    let descendant = editor.child_handle("wait.pid")?;
    editor.send("3\r")?;
    editor.wait_result("1")?;
    // SAFETY: this is an observation handle for the fixture descendant.
    ensure!(
        unsafe { WaitForSingleObject(descendant.as_raw_handle(), 10000) } == WAIT_OBJECT_0,
        "descendant survived shell completion"
    );
    Ok(())
}

#[test]
fn editor_termination_kills_the_interactive_process_tree() -> anyhow::Result<()> {
    let mut editor = Editor::new()?;
    editor.fixture("tree")?;
    let child = editor.child_handle("tree.pid")?;
    let descendant = editor.child_handle("wait.pid")?;
    // SAFETY: terminate only this test's editor; the OS closes its job handle.
    check(unsafe { TerminateProcess(editor.process.as_raw_handle(), 1) })?;
    for handle in [child, descendant] {
        ensure!(
            unsafe { WaitForSingleObject(handle.as_raw_handle(), 10000) } == WAIT_OBJECT_0,
            "interactive process survived editor termination"
        );
    }
    Ok(())
}
