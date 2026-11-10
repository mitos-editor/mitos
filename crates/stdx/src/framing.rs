//! Byte framing shared by language servers and debug adapters.
use std::{io, num::ParseIntError};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug)]
pub enum ReadError {
    Io(io::Error),
    InvalidLength(ParseIntError),
    MissingLength,
}

/// Read headers and a byte-counted body into caller-owned reusable buffers.
/// Returns false when the stream closes while reading headers.
pub async fn read(
    reader: &mut (impl AsyncBufRead + Unpin),
    header: &mut String,
    content: &mut Vec<u8>,
) -> Result<bool, ReadError> {
    let mut length = None;
    loop {
        header.clear();
        if reader.read_line(header).await.map_err(ReadError::Io)? == 0 {
            return Ok(false);
        }
        if header == "\r\n" {
            break;
        }
        if let Some(("Content-Length", value)) = header.trim().split_once(": ") {
            length = Some(value.parse().map_err(ReadError::InvalidLength)?);
        }
    }
    let length = length.ok_or(ReadError::MissingLength)?;
    content.resize(length, 0);
    reader.read_exact(content).await.map_err(ReadError::Io)?;
    Ok(true)
}

/// Write the byte length and body in order, then flush.
pub async fn write(writer: &mut (impl AsyncWrite + Unpin), content: &[u8]) -> io::Result<()> {
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", content.len()).as_bytes())
        .await?;
    writer.write_all(content).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn reads_noise_headers_and_utf8_as_bytes_with_reused_buffers() {
        let data = "noise\r\nContent-Type: application/json\r\nContent-Length: 1\r\nContent-Length: 3\r\n\r\n界Content-Length: 1\r\n\r\nx";
        let mut reader = BufReader::new(data.as_bytes());
        let mut header = String::new();
        let mut body = Vec::new();
        assert!(read(&mut reader, &mut header, &mut body).await.unwrap());
        assert_eq!(body, "界".as_bytes());
        assert!(read(&mut reader, &mut header, &mut body).await.unwrap());
        assert_eq!(body, b"x");
        assert!(!read(&mut reader, &mut header, &mut body).await.unwrap());
    }

    #[tokio::test]
    async fn writes_and_flushes_a_byte_counted_frame() {
        let mut writer = tokio::io::BufWriter::new(Vec::new());
        write(&mut writer, "界".as_bytes()).await.unwrap();
        assert_eq!(writer.get_ref(), "Content-Length: 3\r\n\r\n界".as_bytes());
    }

    #[tokio::test]
    async fn distinguishes_bad_headers_and_truncated_bodies() {
        for (data, kind) in [
            ("Content-Length: nope\r\n\r\n", 0),
            ("Other: x\r\n\r\n", 1),
            ("Content-Length: 2\r\n\r\nx", 2),
        ] {
            let mut reader = BufReader::new(data.as_bytes());
            let error = read(&mut reader, &mut String::new(), &mut Vec::new())
                .await
                .unwrap_err();
            assert!(match (kind, error) {
                (0, ReadError::InvalidLength(_)) | (1, ReadError::MissingLength) => true,
                (2, ReadError::Io(error)) => error.kind() == io::ErrorKind::UnexpectedEof,
                _ => false,
            });
        }
    }
}
