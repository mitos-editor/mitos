//! Content-Length framing without protocol decoding or connection policy.
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("server closed the stream")]
    Closed,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid content length")]
    InvalidLength(#[source] std::num::ParseIntError),
    #[error("missing content length")]
    MissingLength,
}

/// Read one frame into reusable buffers. Unknown headers and launcher output are ignored.
pub async fn read_frame(
    reader: &mut (impl AsyncBufRead + Unpin + ?Sized),
    header: &mut String,
    content: &mut Vec<u8>,
) -> Result<(), ReadError> {
    let mut length = None;
    loop {
        header.clear();
        if reader.read_line(header).await? == 0 {
            return Err(ReadError::Closed);
        }
        if header == "\r\n" {
            break;
        }
        if let Some(("Content-Length", value)) = header.trim().split_once(": ") {
            length = Some(value.parse().map_err(ReadError::InvalidLength)?);
        }
    }
    content.resize(length.ok_or(ReadError::MissingLength)?, 0);
    reader.read_exact(content).await?;
    Ok(())
}

/// Write the complete frame and flush before returning to the transport.
pub async fn write_frame(
    writer: &mut (impl AsyncWrite + Unpin + ?Sized),
    content: &[u8],
) -> std::io::Result<()> {
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", content.len()).as_bytes())
        .await?;
    writer.write_all(content).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_keep_unicode_byte_lengths_and_replace_reused_bodies() {
        let mut wire = Vec::new();
        for body in ["{\"text\":\"😀界\"}", "{}"] {
            write_frame(&mut wire, body.as_bytes()).await.unwrap();
        }
        let mut reader = &wire[..];
        let (mut header, mut content) = (String::new(), Vec::new());
        for body in ["{\"text\":\"😀界\"}", "{}"] {
            read_frame(&mut reader, &mut header, &mut content)
                .await
                .unwrap();
            assert_eq!(content, body.as_bytes());
        }
        assert!(matches!(
            read_frame(&mut reader, &mut header, &mut content).await,
            Err(ReadError::Closed)
        ));
    }

    #[tokio::test]
    async fn header_tolerance_and_failures_preserve_transport_boundaries() {
        let mut header = String::new();
        let mut content = Vec::new();
        let mut reader =
            &b"launcher output\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}"[..];
        read_frame(&mut reader, &mut header, &mut content)
            .await
            .unwrap();
        assert_eq!(content, b"{}");
        assert!(matches!(
            read_frame(
                &mut &b"Content-Length: nope\r\n\r\n"[..],
                &mut header,
                &mut content
            )
            .await,
            Err(ReadError::InvalidLength(_))
        ));
        assert!(matches!(
            read_frame(&mut &b"\r\n"[..], &mut header, &mut content).await,
            Err(ReadError::MissingLength)
        ));
        assert!(
            matches!(read_frame(&mut &b"Content-Length: 3\r\n\r\n{}"[..], &mut header, &mut content).await, Err(ReadError::Io(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof)
        );
        assert!(matches!(
            read_frame(
                &mut &b"Content-Length: 3\r\n"[..],
                &mut header,
                &mut content
            )
            .await,
            Err(ReadError::Closed)
        ));
    }
}
