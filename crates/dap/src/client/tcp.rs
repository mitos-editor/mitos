//! Readiness and process ownership for locally managed TCP adapters.
use crate::Result;
use anyhow::{anyhow, Context as _};
use std::{net::SocketAddr, process::Stdio, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    process::Child,
    time::{self, Instant},
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_DELAY: Duration = Duration::from_millis(25);

pub(super) async fn spawn(
    command: &str,
    args: Vec<&str>,
    port_format: &str,
) -> Result<(TcpStream, Child, SocketAddr)> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("Could not allocate a debug adapter port")?;
    let socket = listener.local_addr()?;
    drop(listener);
    let mut process = super::adapter_command(command)
        .args(args)
        .args(
            port_format
                .replace("{}", &socket.port().to_string())
                .split_whitespace(),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // The startup future owns this process until the connected client takes over.
        .kill_on_drop(true)
        .spawn()?;
    let stream = tokio::select! {
        biased;
        status = process.wait() => {
            return Err(anyhow!("Debug adapter {command} exited before accepting connections ({})", status?).into());
        }
        stream = connect(socket, Instant::now() + STARTUP_TIMEOUT) => stream?,
    };
    Ok((stream, process, socket))
}

async fn connect(socket: SocketAddr, deadline: Instant) -> Result<TcpStream> {
    time::timeout_at(deadline, async {
        loop {
            match TcpStream::connect(socket).await {
                Ok(stream) => return Ok(stream),
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                    time::sleep(RETRY_DELAY).await;
                }
                Err(error) => return Err(error),
            }
        }
    })
    .await
    .with_context(|| format!("Timed out waiting for debug adapter at {socket}"))?
    .with_context(|| format!("Cannot connect to debug adapter at {socket}"))
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connection_retries_until_the_listener_is_ready() {
        let reservation = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let socket = reservation.local_addr().unwrap();
        drop(reservation);
        let connect = connect(socket, Instant::now() + Duration::from_secs(5));
        tokio::pin!(connect);
        tokio::select! {
            result = &mut connect => panic!("connection finished before readiness: {result:?}"),
            _ = time::sleep(Duration::from_millis(75)) => {},
        }
        let listener = TcpListener::bind(socket).await.unwrap();
        assert!(connect.await.is_ok());
        drop(listener);
    }

    #[tokio::test]
    async fn readiness_deadline_reports_the_adapter_address() {
        let reservation = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let socket = reservation.local_addr().unwrap();
        drop(reservation);
        let error = connect(socket, Instant::now() + Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains(&format!("Timed out waiting for debug adapter at {socket}")));
    }
}
