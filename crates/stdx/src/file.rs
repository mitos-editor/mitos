/// Heuristic "is this a binary (non-text) file?" check over a leading chunk of a
/// file. Replaces the `content_inspector` crate — we only need the binary/text
/// verdict, not its encoding classification.
///
/// A leading byte-order mark marks the content as text (UTF-16/32 text
/// legitimately contains NUL bytes, so it must be excluded before the NUL scan);
/// otherwise a NUL byte in the first kilobyte — or a known binary magic number —
/// means binary.
pub fn is_binary(buffer: &[u8]) -> bool {
    // UTF-32 BOMs must be checked before UTF-16 (their BOMs overlap).
    const BYTE_ORDER_MARKS: &[&[u8]] = &[
        &[0xEF, 0xBB, 0xBF],       // UTF-8
        &[0x00, 0x00, 0xFE, 0xFF], // UTF-32BE
        &[0xFF, 0xFE, 0x00, 0x00], // UTF-32LE
        &[0xFE, 0xFF],             // UTF-16BE
        &[0xFF, 0xFE],             // UTF-16LE
    ];

    if BYTE_ORDER_MARKS.iter().any(|bom| buffer.starts_with(bom)) {
        return false;
    }

    let scan = &buffer[..buffer.len().min(1024)];
    scan.contains(&0) || has_binary_signature(buffer)
}

/// Recognize binary headers even when an explicit text encoding was requested.
pub fn has_binary_signature(buffer: &[u8]) -> bool {
    [
        b"%PDF".as_slice(),
        b"\x89PNG",
        b"\xff\xd8\xff",
        b"GIF87a",
        b"GIF89a",
    ]
    .iter()
    .any(|magic| buffer.starts_with(magic))
        || (buffer.starts_with(b"RIFF") && buffer.get(8..12) == Some(b"WEBP"))
}
/// Rename a file or directory without replacing an existing destination.
/// Unsupported platforms fail rather than falling back to a racy existence check.
pub fn rename_noreplace(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    #[cfg(any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "redox"
    ))]
    {
        use rustix::fs::{renameat_with, RenameFlags, CWD};
        Ok(renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE)?)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        fn wide(path: &std::path::Path) -> std::io::Result<Vec<u16>> {
            let mut path: Vec<_> = path.as_os_str().encode_wide().collect();
            if path.contains(&0) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "path contains a null character",
                ));
            }
            path.push(0);
            Ok(path)
        }
        let from = wide(from)?;
        let to = wide(to)?;
        // Both paths are NUL-terminated and remain alive for the call. Omitting
        // MOVEFILE_REPLACE_EXISTING makes destination collisions fail atomically.
        if unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 0)
        } == 0
        {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    #[cfg(not(any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "redox",
        windows
    )))]
    {
        let _ = (from, to);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "non-replacing rename is not supported on this platform",
        ))
    }
}

#[cfg(all(
    test,
    any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "redox",
        windows
    )
))]
mod rename_tests {
    use super::rename_noreplace;
    use std::fs;

    #[test]
    fn rename_preserves_existing_files_and_directories() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        for directory in [false, true] {
            let from = dir.path().join(if directory {
                "source-dir"
            } else {
                "source-file"
            });
            let to = dir.path().join(if directory {
                "target-dir"
            } else {
                "target-file"
            });
            if directory {
                fs::create_dir(&from)?;
                fs::create_dir(&to)?;
            } else {
                fs::write(&from, "source")?;
                fs::write(&to, "target")?;
            }
            assert_eq!(
                rename_noreplace(&from, &to).unwrap_err().kind(),
                std::io::ErrorKind::AlreadyExists
            );
            assert!(from.exists() && to.exists());
            if directory {
                fs::remove_dir(&to)?;
            } else {
                assert_eq!(fs::read_to_string(&to)?, "target");
                fs::remove_file(&to)?;
            }
            rename_noreplace(&from, &to)?;
            assert!(!from.exists() && to.exists());
        }
        Ok(())
    }
}
