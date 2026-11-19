//! Cancellable Windows pipes for command-based clipboard providers.

use std::{
    io,
    os::windows::io::AsRawHandle,
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

pub(super) async fn pipe(parent_writes: bool) -> io::Result<(NamedPipeServer, Stdio)> {
    static NEXT_PIPE: AtomicU64 = AtomicU64::new(0);
    let name = format!(
        r"\\.\pipe\mitos-clipboard-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT_PIPE.fetch_add(1, Ordering::Relaxed),
    );
    let server = ServerOptions::new()
        .access_inbound(!parent_writes)
        .access_outbound(parent_writes)
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .create(&name)?;
    let client = std::fs::OpenOptions::new()
        .read(parent_writes)
        .write(!parent_writes)
        .open(&name)?;
    server.connect().await?;
    // Stdio duplicates this synchronous child end as an inheritable handle.
    Ok((server, Stdio::from(client)))
}

pub(super) async fn finish_write(pipe: &NamedPipeServer) -> io::Result<()> {
    // Mio can accept an entire write into an overlapped operation. Its flush is
    // a no-op; a further empty write checks that the preceding write completed
    // before closing stdin and remains cancellable under the command deadline.
    loop {
        pipe.writable().await?;
        match pipe.try_write(&[]) {
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
            result => return result.map(|_| ()),
        }
    }
}

pub(super) fn cancel(pipe: &NamedPipeServer) {
    // The owned pipe stays alive while outstanding I/O is marked for cancellation.
    // ERROR_NOT_FOUND simply means the operation has already completed.
    unsafe {
        windows_sys::Win32::System::IO::CancelIoEx(pipe.as_raw_handle(), std::ptr::null());
    }
}
