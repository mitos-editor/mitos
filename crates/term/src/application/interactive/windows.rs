//! Run a shell in the existing console, with stdout reserved for its result.

use anyhow::{bail, ensure, Context};
use std::{
    fs::File,
    io,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    process::{Output, Stdio},
    ptr,
};
use tokio::io::AsyncReadExt;
use windows_sys::Win32::{
    Foundation::{HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Console::{
            GetConsoleMode, SetConsoleCtrlHandler, SetConsoleMode, CTRL_BREAK_EVENT, CTRL_C_EVENT,
        },
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Threading::{OpenThread, ResumeThread, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME},
    },
};

fn check(success: i32) -> io::Result<()> {
    if success == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: the caller transfers a newly created, valid handle.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

struct Console {
    input: File,
    output: File,
    input_mode: u32,
    output_mode: u32,
}

impl Console {
    fn open() -> io::Result<Self> {
        let input = File::options().read(true).write(true).open("CONIN$")?;
        let output = File::options().read(true).write(true).open("CONOUT$")?;
        let mut input_mode = 0;
        let mut output_mode = 0;
        // SAFETY: both files own live console handles and the mode pointers are writable.
        unsafe {
            check(GetConsoleMode(input.as_raw_handle(), &mut input_mode))?;
            check(GetConsoleMode(output.as_raw_handle(), &mut output_mode))?;
        }
        Ok(Self {
            input,
            output,
            input_mode,
            output_mode,
        })
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        // Children can leave raw input or output modes behind, even on failure.
        // Restore the modes from after Mitos released its terminal, before the
        // backend enables the editor's terminal modes again.
        for (handle, mode) in [
            (self.input.as_raw_handle(), self.input_mode),
            (self.output.as_raw_handle(), self.output_mode),
        ] {
            // SAFETY: this guard still owns both console handles.
            if let Err(error) = check(unsafe { SetConsoleMode(handle, mode) }) {
                log::error!("Could not restore console mode: {error}");
            }
        }
    }
}

// A real handler affects only Mitos. Using SetConsoleCtrlHandler(None, TRUE)
// would make children inherit the flag that ignores Ctrl-c as well.
unsafe extern "system" fn control(event: u32) -> i32 {
    i32::from(matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT))
}

pub(super) struct ControlHandler;

impl ControlHandler {
    pub(super) fn install() -> io::Result<Self> {
        // SAFETY: control has the required ABI, does not access shared state,
        // and remains valid until this guard unregisters it.
        check(unsafe { SetConsoleCtrlHandler(Some(control), 1) })?;
        Ok(Self)
    }
}

impl Drop for ControlHandler {
    fn drop(&mut self) {
        // SAFETY: remove the same static handler registered by install.
        if let Err(error) = check(unsafe { SetConsoleCtrlHandler(Some(control), 0) }) {
            log::error!("Could not remove interactive console handler: {error}");
        }
    }
}

struct Job(OwnedHandle);

impl Job {
    fn new() -> io::Result<Self> {
        // SAFETY: no name or inheritable security attributes are supplied.
        let job = Self(owned(unsafe {
            CreateJobObjectW(ptr::null(), ptr::null())
        })?);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: limits has the layout and size required by this information class.
        check(unsafe {
            SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of_val(&limits) as u32,
            )
        })?;
        Ok(job)
    }

    fn terminate(&self) -> io::Result<()> {
        // SAFETY: the owned job handle is valid. This also closes inherited
        // stdout pipes held by descendants after their parent shell exits.
        check(unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) })
    }
}

fn resume(process: u32) -> anyhow::Result<()> {
    // Stable std::process does not expose the primary thread handle. Before
    // resumption a CREATE_SUSPENDED child has only its primary thread, so find
    // that thread using the documented Tool Help API.
    // SAFETY: the snapshot is transferred immediately to its owning guard.
    let snapshot = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    // SAFETY: entry is initialized with the structure size required by Tool Help.
    check(unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) })?;
    loop {
        if entry.th32OwnerProcessID == process {
            // SAFETY: open the identified child's thread without handle inheritance.
            let thread =
                owned(unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) })?;
            // SAFETY: the child was created suspended and already belongs to our job.
            ensure!(
                unsafe { ResumeThread(thread.as_raw_handle()) } != u32::MAX,
                "Could not resume interactive shell: {}",
                io::Error::last_os_error()
            );
            return Ok(());
        }
        // SAFETY: snapshot and entry remain valid for enumeration.
        if unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } == 0 {
            bail!("Could not find the interactive shell's suspended thread");
        }
    }
}

pub(super) async fn run(mut command: tokio::process::Command) -> anyhow::Result<Output> {
    let console = Console::open().context("Could not open the controlling console")?;
    let job = Job::new().context("Could not create interactive shell job")?;
    command
        .stdin(console.input.try_clone()?)
        .stderr(console.output.try_clone()?)
        .stdout(Stdio::piped())
        .creation_flags(CREATE_SUSPENDED)
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .context("Could not start interactive shell")?;
    // SAFETY: this newly spawned child owns a live process handle. Assignment
    // precedes resumption, so no descendant can escape the job during startup.
    check(unsafe { AssignProcessToJobObject(job.0.as_raw_handle(), child.raw_handle().unwrap()) })
        .context("Could not assign interactive shell to its job")?;
    resume(child.id().unwrap())?;
    let mut stdout = child.stdout.take().unwrap();
    let mut output = Vec::new();
    let (status, _) = tokio::try_join!(
        async {
            let status = child.wait().await?;
            job.terminate()?;
            Ok::<_, io::Error>(status)
        },
        stdout.read_to_end(&mut output),
    )?;
    // Dropping the job on cancellation or error also terminates descendants.
    Ok(Output {
        status,
        stdout: output,
        stderr: Vec::new(),
    })
}
