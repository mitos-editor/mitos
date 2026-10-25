//! Content-Length framing shared by LSP and DAP transports.

use anyhow::Context;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Read one frame into reusable buffers. `false` means EOF while reading headers.
/// Ignore other headers and stray log lines emitted by server launch scripts.
/// Payload decoding and protocol-specific handling belong to the caller.
pub async fn read_frame(
    reader: &mut (impl AsyncBufRead + Unpin),
    header: &mut String,
    content: &mut Vec<u8>,
) -> anyhow::Result<bool> {
    let mut content_length = None;
    loop {
        header.clear();
        if reader.read_line(header).await? == 0 {
            return Ok(false);
        }
        if header == "\r\n" {
            break;
        }
        if let Some(("Content-Length", value)) = header.trim().split_once(": ") {
            content_length = Some(value.parse().context("invalid content length")?);
        }
    }
    content.resize(content_length.context("missing content length")?, 0);
    reader.read_exact(content).await?;
    Ok(true)
}

/// Write and flush a complete frame. Length is measured in UTF-8 bytes.
pub async fn write_frame(
    writer: &mut (impl AsyncWrite + Unpin),
    content: &str,
) -> std::io::Result<()> {
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", content.len()).as_bytes())
        .await?;
    writer.write_all(content.as_bytes()).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_preserve_utf8_boundaries_and_reuse_buffers() {
        let mut wire = Vec::new();
        write_frame(&mut wire, "界").await.unwrap();
        write_frame(&mut wire, "{}").await.unwrap();
        assert!(wire.starts_with(b"Content-Length: 3\r\n\r\n"));
        let mut reader = &wire[..];
        let mut header = String::new();
        let mut content = Vec::new();
        for expected in ["界", "{}"] {
            assert!(read_frame(&mut reader, &mut header, &mut content)
                .await
                .unwrap());
            assert_eq!(content, expected.as_bytes());
        }
        assert!(!read_frame(&mut reader, &mut header, &mut content)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn extra_headers_and_launcher_output_are_ignored() {
        let mut reader =
            &b"server starting\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}"[..];
        let mut header = String::new();
        let mut content = Vec::new();
        assert!(read_frame(&mut reader, &mut header, &mut content)
            .await
            .unwrap());
        assert_eq!(content, b"{}");
    }

    #[tokio::test]
    async fn malformed_and_truncated_frames_fail() {
        for wire in [
            &b"\r\n{}"[..],
            &b"Content-Length: invalid\r\n\r\n{}"[..],
            &b"Content-Length: 3\r\n\r\n{}"[..],
        ] {
            let mut reader = wire;
            assert!(read_frame(&mut reader, &mut String::new(), &mut Vec::new())
                .await
                .is_err());
        }
    }
}
