//! File sniffing and binary detection for document opening.

use ::text_io::from_reader;
use editor_core::{encoding, encoding::Encoding, Rope};
use std::io;

pub(super) struct DecodedFile {
    pub text: Rope,
    pub encoding: &'static Encoding,
    pub has_bom: bool,
    pub binary: bool,
}

pub(super) fn read_file(
    file: &mut std::fs::File,
    encoding: Option<&'static Encoding>,
) -> io::Result<DecodedFile> {
    use std::io::BufRead;
    let mut reader = io::BufReader::with_capacity(1024, file);
    let explicitly_unicode = encoding
        .is_some_and(|encoding| encoding == encoding::UTF_16LE || encoding == encoding::UTF_16BE);
    let prefix = reader.fill_buf()?;
    if stdx::file::has_binary_signature(prefix)
        || (!explicitly_unicode && stdx::file::is_binary(prefix))
    {
        return Ok(DecodedFile {
            text: Rope::new(),
            encoding: encoding::UTF_8,
            has_bom: false,
            binary: true,
        });
    }
    let (text, encoding, has_bom) = from_reader(&mut reader, encoding)?;
    Ok(DecodedFile {
        text,
        encoding,
        has_bom,
        binary: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::text_io::to_writer;

    #[test]
    fn detect_utf8_with_split_cjk_character() {
        // File opening preloads 1024 bytes for binary detection. This splits a
        // three-byte character, leaving the rest of the decoding buffer unused.
        let mut text = "中文日本語한국어".repeat(1000);
        let mut file = tempfile::tempfile().unwrap();
        std::io::Write::write_all(&mut file, text.as_bytes()).unwrap();
        std::io::Seek::rewind(&mut file).unwrap();

        let DecodedFile {
            text: mut rope,
            encoding,
            has_bom,
            binary,
        } = read_file(&mut file, None).unwrap();
        assert_eq!(encoding, encoding::UTF_8);
        assert!(!has_bom);
        assert!(!binary);
        assert_eq!(rope.to_string(), text);

        // Saving and reopening must also preserve pasted CJK text.
        let pasted = "粘贴した텍스트";
        rope.insert(rope.len_chars(), pasted);
        text.push_str(pasted);
        let mut saved = Vec::new();
        lsp_client::block_on(to_writer(&mut saved, (encoding, has_bom), &rope)).unwrap();
        assert_eq!(saved, text.as_bytes());
        let (reopened, encoding, _) = from_reader(&mut saved.as_slice(), None).unwrap();
        assert_eq!(encoding, encoding::UTF_8);
        assert_eq!(reopened, rope);
    }
}
