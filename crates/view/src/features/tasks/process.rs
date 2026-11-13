//! Streaming task processes, with cancellation and ownership of descendants.

use std::{
    process::{ExitStatus, Stdio},
    time::Duration,
};

use anyhow::Context;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::mpsc,
    time::{sleep, Instant},
};

const READ_SIZE: usize = 8 * 1024;
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stream {
    Stdout,
    Stderr,
}

#[derive(Debug)]
pub(super) struct Chunk {
    pub stream: Stream,
    pub bytes: Vec<u8>,
}

async fn read_stream(
    mut reader: impl AsyncRead + Unpin,
    stream: Stream,
    output: mpsc::Sender<Chunk>,
) -> anyhow::Result<()> {
    let mut bytes = [0; READ_SIZE];
    loop {
        let count = reader
            .read(&mut bytes)
            .await
            .context("Could not read task output")?;
        if count == 0 {
            return Ok(());
        }
        // Await each send before reading again: memory stays bounded, and bytes
        // from this pipe cannot be reordered around a partial UTF-8 character.
        output
            .send(Chunk {
                stream,
                bytes: bytes[..count].to_vec(),
            })
            .await
            .map_err(|_| anyhow::anyhow!("Task output receiver closed"))?;
    }
}

pub(super) async fn run(
    mut command: Command,
    cancel: event::TaskHandle,
    output: mpsc::Sender<Chunk>,
) -> anyhow::Result<Option<ExitStatus>> {
    if cancel.is_canceled() {
        return Ok(None);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut tree = platform::ProcessTree::prepare(&mut command)?;
    let mut child = command.spawn().context("Could not start task")?;
    tree.attach(&child)?;
    let stdout = child.stdout.take().expect("task stdout was piped");
    let stderr = child.stderr.take().expect("task stderr was piped");
    let readers = async {
        tokio::try_join!(
            read_stream(stdout, Stream::Stdout, output.clone()),
            read_stream(stderr, Stream::Stderr, output),
        )?;
        Ok::<_, anyhow::Error>(())
    };
    tokio::pin!(readers);
    let deadline = sleep(DRAIN_TIMEOUT);
    tokio::pin!(deadline);
    let mut status = None;
    let mut output_closed = false;
    let result = loop {
        tokio::select! {
            biased;
            // This also interrupts readers blocked on channel backpressure.
            _ = cancel.canceled() => break Ok(None),
            result = child.wait(), if status.is_none() => {
                match result.context("Could not wait for task") {
                    Ok(exit) => status = Some(exit),
                    Err(error) => break Err(error),
                }
                // A successful shell may leave background children holding its
                // pipes open. They belong to this invocation and stop with it.
                if let Err(error) = tree.terminate() {
                    break Err(error.into());
                }
                if output_closed {
                    break Ok(status);
                }
                deadline.as_mut().reset(Instant::now() + DRAIN_TIMEOUT);
            }
            result = &mut readers, if !output_closed => {
                if let Err(error) = result {
                    break Err(error);
                }
                output_closed = true;
                if status.is_some() {
                    break Ok(status);
                }
            }
            _ = &mut deadline, if status.is_some() => break Ok(status),
        }
    };

    // Keep cleanup within this future. Dropping it instead still runs the tree
    // guard and Child's kill_on_drop guard, including during editor shutdown.
    let cleanup = tree.terminate();
    let reap = if status.is_none() {
        let _ = child.start_kill();
        child.wait().await.map(|_| ())
    } else {
        Ok(())
    };
    let result = result?;
    cleanup.context("Could not terminate task descendants")?;
    reap.context("Could not reap task")?;
    Ok(result)
}

#[cfg(unix)]
mod platform {
    use std::{io, os::unix::process::CommandExt};

    use tokio::process::{Child, Command};

    pub(super) struct ProcessTree(Option<libc::pid_t>);

    impl ProcessTree {
        pub(super) fn prepare(command: &mut Command) -> io::Result<Self> {
            command.as_std_mut().process_group(0);
            Ok(Self(None))
        }

        pub(super) fn attach(&mut self, child: &Child) -> io::Result<()> {
            self.0 = child.id().map(|id| id as libc::pid_t);
            Ok(())
        }

        pub(super) fn terminate(&mut self) -> io::Result<()> {
            if let Some(group) = self.0.take() {
                // SAFETY: group is the positive ID of our own child's private
                // process group. A negative argument targets that group only.
                if unsafe { libc::kill(-group, libc::SIGKILL) } != 0 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        return Err(error);
                    }
                }
            }
            Ok(())
        }
    }

    impl Drop for ProcessTree {
        fn drop(&mut self) {
            if let Err(error) = self.terminate() {
                log::error!("Could not terminate task process group: {error}");
            }
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::{
        io,
        mem::size_of,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        ptr,
    };

    use tokio::process::{Child, Command};
    use windows_sys::Win32::{
        Foundation::{HANDLE, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD,
                THREADENTRY32,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
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

    pub(super) struct ProcessTree(Option<OwnedHandle>);

    impl ProcessTree {
        pub(super) fn prepare(command: &mut Command) -> io::Result<Self> {
            // SAFETY: no name or inheritable security attributes are supplied.
            let job = owned(unsafe { CreateJobObjectW(ptr::null(), ptr::null()) })?;
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: limits has the required layout and remains valid for this call.
            check(unsafe {
                SetInformationJobObject(
                    job.as_raw_handle(),
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    size_of_val(&limits) as u32,
                )
            })?;
            command.creation_flags(CREATE_SUSPENDED);
            Ok(Self(Some(job)))
        }

        pub(super) fn attach(&mut self, child: &Child) -> io::Result<()> {
            // Assignment precedes resumption, so no descendant can escape the
            // job during startup. Dropping the job kills every owned process.
            // SAFETY: both the job and newly spawned child's handles are live.
            check(unsafe {
                AssignProcessToJobObject(
                    self.0.as_ref().unwrap().as_raw_handle(),
                    child.raw_handle().unwrap(),
                )
            })?;
            resume(child.id().unwrap())
        }

        pub(super) fn terminate(&mut self) -> io::Result<()> {
            drop(self.0.take());
            Ok(())
        }
    }

    fn resume(process: u32) -> io::Result<()> {
        // Stable std::process does not expose the primary thread handle. A
        // CREATE_SUSPENDED process has only its primary thread at this point.
        // SAFETY: the snapshot is transferred immediately to its owning guard.
        let snapshot = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        // SAFETY: entry contains the structure size required by Tool Help.
        check(unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) })?;
        loop {
            if entry.th32OwnerProcessID == process {
                // SAFETY: open this child's primary thread without inheritance.
                let thread =
                    owned(unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) })?;
                // SAFETY: the child is suspended and already belongs to our job.
                if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
                return Ok(());
            }
            // SAFETY: snapshot and entry remain valid for enumeration.
            if unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } == 0 {
                return Err(io::Error::other(
                    "Could not find the task's suspended thread",
                ));
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use std::io;
    use tokio::process::{Child, Command};

    pub(super) struct ProcessTree;

    impl ProcessTree {
        pub(super) fn prepare(_: &mut Command) -> io::Result<Self> {
            Ok(Self)
        }

        pub(super) fn attach(&mut self, _: &Child) -> io::Result<()> {
            Ok(())
        }

        pub(super) fn terminate(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use event::TaskController;
    use tokio::time::timeout;

    use super::*;

    const FIXTURE: &str = "features::tasks::process::tests::fixture_process";
    const MODE: &str = "MITOS_TEST_TASK_PROCESS_MODE";

    fn command(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", FIXTURE, "--nocapture"])
            .env(MODE, mode);
        command
    }

    #[test]
    #[allow(
        clippy::zombie_processes,
        reason = "This subprocess fixture leaves a descendant for the task owner to terminate"
    )]
    fn fixture_process() {
        let Ok(mode) = std::env::var(MODE) else {
            return;
        };
        match mode.as_str() {
            "stream" => {
                println!("task-ready");
                std::io::stdout().flush().unwrap();
                std::thread::sleep(Duration::from_secs(60));
            }
            "bulk" => {
                std::io::stdout()
                    .write_all("界".repeat(40_000).as_bytes())
                    .unwrap();
                std::io::stderr()
                    .write_all("é".repeat(40_000).as_bytes())
                    .unwrap();
            }
            "background" | "family" => {
                let child = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", FIXTURE, "--nocapture"])
                    .env(MODE, "stream")
                    .spawn()
                    .unwrap();
                println!("descendant:{}", child.id());
                std::io::stdout().flush().unwrap();
                if mode == "background" {
                    std::process::exit(7);
                }
                std::thread::sleep(Duration::from_secs(60));
            }
            _ => panic!("unknown fixture mode"),
        }
        std::process::exit(0);
    }

    #[tokio::test]
    async fn output_streams_before_exit_and_cancel_interrupts_backpressure() {
        let mut controller = TaskController::new();
        let handle = controller.restart();
        let (send, mut receive) = mpsc::channel(1);
        let task = tokio::spawn(run(command("stream"), handle, send));
        let mut text = Vec::new();
        timeout(Duration::from_secs(10), async {
            while !String::from_utf8_lossy(&text).contains("task-ready") {
                text.extend(receive.recv().await.unwrap().bytes);
            }
        })
        .await
        .unwrap();
        assert!(!task.is_finished());
        controller.cancel();
        assert!(timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .is_none());

        let handle = controller.restart();
        let (send, receive) = mpsc::channel(1);
        let task = tokio::spawn(run(command("bulk"), handle, send));
        timeout(Duration::from_secs(10), async {
            while receive.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        controller.cancel();
        assert!(timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn both_pipes_keep_their_byte_order_and_reads_are_bounded() {
        let mut controller = TaskController::new();
        let (send, mut receive) = mpsc::channel::<Chunk>(2);
        let collect = async {
            let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
            while let Some(chunk) = receive.recv().await {
                assert!(chunk.bytes.len() <= READ_SIZE);
                match chunk.stream {
                    Stream::Stdout => stdout.extend(chunk.bytes),
                    Stream::Stderr => stderr.extend(chunk.bytes),
                }
            }
            (stdout, stderr)
        };
        let (result, (stdout, stderr)) = timeout(Duration::from_secs(10), async {
            tokio::join!(run(command("bulk"), controller.restart(), send), collect)
        })
        .await
        .unwrap();
        assert!(result.unwrap().unwrap().success());
        assert!(String::from_utf8(stdout)
            .unwrap()
            .ends_with(&"界".repeat(40_000)));
        assert_eq!(String::from_utf8(stderr).unwrap(), "é".repeat(40_000));
    }

    #[tokio::test]
    async fn shell_exit_does_not_wait_for_background_descendants_holding_pipes() {
        let mut controller = TaskController::new();
        let (send, mut receive) = mpsc::channel::<Chunk>(8);
        let collect = async {
            let mut bytes = Vec::new();
            while let Some(chunk) = receive.recv().await {
                bytes.extend(chunk.bytes);
            }
            bytes
        };
        let (result, bytes) = timeout(Duration::from_secs(10), async {
            tokio::join!(
                run(command("background"), controller.restart(), send),
                collect
            )
        })
        .await
        .unwrap();
        assert_eq!(result.unwrap().unwrap().code(), Some(7));
        assert!(String::from_utf8_lossy(&bytes).contains("descendant:"));
    }

    #[cfg(unix)]
    async fn assert_terminated(pid: libc::pid_t) {
        timeout(Duration::from_secs(10), async {
            loop {
                // SAFETY: signal zero only queries the child's process ID.
                if unsafe { libc::kill(pid, 0) } != 0 {
                    return;
                }
                // A container's init may leave an orphaned zombie unreaped. It
                // has terminated and cannot retain task pipes or execute work.
                #[cfg(target_os = "linux")]
                if std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                    stat.rsplit_once(") ")
                        .is_some_and(|(_, s)| s.starts_with('Z'))
                }) {
                    return;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("task descendant survived cleanup");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_and_dropped_owner_terminate_descendants() {
        for abort in [false, true] {
            let mut controller = TaskController::new();
            let (send, mut receive) = mpsc::channel(8);
            let task = tokio::spawn(run(command("family"), controller.restart(), send));
            let descendant = timeout(Duration::from_secs(10), async {
                let mut text = Vec::new();
                loop {
                    text.extend(receive.recv().await.unwrap().bytes);
                    if let Some(pid) = String::from_utf8_lossy(&text)
                        .split_inclusive('\n')
                        .filter(|line| line.ends_with('\n'))
                        .find_map(|line| line.trim_end().strip_prefix("descendant:"))
                        .and_then(|pid| pid.parse().ok())
                    {
                        break pid;
                    }
                }
            })
            .await
            .unwrap();
            if abort {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
            } else {
                controller.cancel();
                assert!(timeout(Duration::from_secs(10), task)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap()
                    .is_none());
            }
            assert_terminated(descendant).await;
        }
    }
}
