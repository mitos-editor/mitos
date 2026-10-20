//! Owned process I/O for external editor tools.
//!
//! The caller configures the executable, arguments, and directory, and interprets
//! exit status and output. This module owns pipe setup, concurrent input/output,
//! and cancellation. No detached task retains the child or its stdin.

use editor_core::{encoding, Rope};
use std::{
    io,
    process::{Output, Stdio},
};
use tokio::process::Command;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Failed to start process: {0}")]
    Spawn(#[source] io::Error),
    #[error("Could not write to process stdin: {0}")]
    Input(#[source] anyhow::Error),
    #[error("Waiting for process output failed: {0}")]
    Wait(#[source] io::Error),
}

pub(crate) async fn output(mut command: Command, input: Option<&Rope>) -> Result<Output, Error> {
    command
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(Error::Spawn)?;
    let stdin = child.stdin.take();
    let (output, ()) = tokio::try_join!(
        async { child.wait_with_output().await.map_err(Error::Wait) },
        async move {
            if let (Some(mut stdin), Some(input)) = (stdin, input) {
                let result = text_io::to_writer(&mut stdin, (encoding::UTF_8, false), input).await;
                // Tools may deliberately stop reading early. Their exit status and
                // captured output remain authoritative when they close the pipe.
                if let Err(error) = result
                    && !error
                        .downcast_ref::<io::Error>()
                        .is_some_and(|error| error.kind() == io::ErrorKind::BrokenPipe)
                {
                    return Err(Error::Input(error));
                }
            }
            Ok(())
        }
    )?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use tokio::io::AsyncReadExt;

    fn command(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "features::process::tests::fixture_process",
                "--nocapture",
            ])
            .env("MITOS_TEST_PROCESS_MODE", mode);
        command
    }

    #[test]
    fn fixture_process() {
        let Ok(mode) = std::env::var("MITOS_TEST_PROCESS_MODE") else {
            return;
        };
        match mode.as_str() {
            "echo" => {
                std::io::stdout()
                    .write_all(&vec![b'o'; 1024 * 1024])
                    .unwrap();
                std::io::stderr()
                    .write_all(&vec![b'e'; 1024 * 1024])
                    .unwrap();
                let mut input = Vec::new();
                std::io::stdin().read_to_end(&mut input).unwrap();
                std::io::stdout().write_all(&input).unwrap();
            }
            "eof" => {
                let mut input = Vec::new();
                std::io::stdin().read_to_end(&mut input).unwrap();
                assert!(input.is_empty());
            }
            "close-input" => {
                std::io::stderr().write_all(b"closed").unwrap();
                std::process::exit(17);
            }
            "wait" => {
                let mut control = std::net::TcpStream::connect(
                    std::env::var("MITOS_TEST_PROCESS_CONTROL").unwrap(),
                )
                .unwrap();
                control.write_all(&[1]).unwrap();
                control.read_exact(&mut [0]).unwrap();
            }
            _ => panic!("unknown fixture mode"),
        }
        std::process::exit(0);
    }

    #[tokio::test]
    async fn tools_can_write_large_outputs_while_consuming_input() {
        let input = Rope::from("界\n".repeat(1024 * 1024));
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            output(command("echo"), Some(&input)),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(result.status.success());
        assert!(result.stdout.ends_with(input.to_string().as_bytes()));
        assert_eq!(result.stderr, vec![b'e'; 1024 * 1024]);
    }

    #[tokio::test]
    async fn tools_receive_eof_without_input() {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            output(command("eof"), None),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(result.status.success());
    }

    #[tokio::test]
    async fn early_input_closure_preserves_exit_status_and_stderr() {
        let input = Rope::from("x".repeat(8 * 1024 * 1024));
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            output(command("close-input"), Some(&input)),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result.status.code(), Some(17));
        assert_eq!(result.stderr, b"closed");
    }

    #[tokio::test]
    async fn cancellation_terminates_the_child_with_a_blocked_input_writer() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut command = command("wait");
        command.env(
            "MITOS_TEST_PROCESS_CONTROL",
            listener.local_addr().unwrap().to_string(),
        );
        let task = tokio::spawn(async move {
            let input = Rope::from("x".repeat(8 * 1024 * 1024));
            output(command, Some(&input)).await
        });
        let mut control =
            tokio::time::timeout(std::time::Duration::from_secs(10), listener.accept())
                .await
                .unwrap()
                .unwrap()
                .0;
        control.read_exact(&mut [0]).await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        // The child's control connection closes when it exits. Platforms may
        // report abrupt socket teardown as either EOF or a connection reset.
        let read = tokio::time::timeout(std::time::Duration::from_secs(10), control.read(&mut [0]))
            .await
            .unwrap();
        assert!(match read {
            Ok(0) => true,
            Err(error) => matches!(
                error.kind(),
                io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
            ),
            _ => false,
        });
    }
}
