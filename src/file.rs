//! Opening files: regular files only, and never a blocking open on a FIFO or device. Files
//! named by untrusted data (stream files) also may not be symbolic links; a file the caller
//! chose may be.

use crate::{Error, Result};

use std::fs::{File, OpenOptions};
use std::path::Path;

/// Open `path` for reading if it is a regular file, and, unless `follow_links`, not a
/// symbolic link.
///
/// The check is made before the open; where the platform flags are known the open itself
/// does not block (and, without `follow_links`, refuses links), and on Unix the opened file
/// must be the one that was checked. Elsewhere a file swapped in between check and open is not
/// detected.
pub fn open_regular(path: &Path, follow_links: bool) -> Result<(File, u64)> {
    let before = if follow_links {
        std::fs::metadata(path)
    } else {
        std::fs::symlink_metadata(path)
    }
    .map_err(Error::io(path))?;
    if !before.file_type().is_file() {
        return Err(Error::Unsupported(format!(
            "{:?} is not a regular file",
            path.display().to_string()
        )));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    open_flags(&mut options, follow_links);
    let file = options.open(path).map_err(Error::io(path))?;
    let after = file.metadata().map_err(Error::io(path))?;
    if !after.is_file() || !same_file(&before, &after) {
        return Err(Error::Unsupported(format!(
            "{:?} changed while it was being opened",
            path.display().to_string()
        )));
    }
    Ok((file, after.len()))
}

#[cfg(unix)]
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file(_: &std::fs::Metadata, _: &std::fs::Metadata) -> bool {
    true
}

/// `O_NONBLOCK`, and `O_NOFOLLOW` unless `follow_links`, where their values are certain.
fn open_flags(options: &mut OpenOptions, follow_links: bool) {
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    const FLAGS: (i32, i32) = (0x0004, 0x0100);
    #[cfg(all(
        any(target_os = "linux", target_os = "android"),
        any(target_arch = "x86_64", target_arch = "x86", target_arch = "riscv64")
    ))]
    const FLAGS: (i32, i32) = (0o4000, 0o400000);
    #[cfg(all(
        any(target_os = "linux", target_os = "android"),
        any(target_arch = "aarch64", target_arch = "arm")
    ))]
    const FLAGS: (i32, i32) = (0o4000, 0o100000);
    #[cfg(any(
        any(target_os = "macos", target_os = "ios", target_os = "freebsd"),
        all(
            any(target_os = "linux", target_os = "android"),
            any(
                target_arch = "x86_64",
                target_arch = "x86",
                target_arch = "riscv64",
                target_arch = "aarch64",
                target_arch = "arm"
            )
        )
    ))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let (nonblock, nofollow) = FLAGS;
        options.custom_flags(if follow_links {
            nonblock
        } else {
            nonblock | nofollow
        });
    }
    let _ = (options, follow_links);
}

/// Read a whole file the caller chose, under a size limit, refusing anything irregular. A
/// file that grows past the limit while being read is refused too.
pub fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let (file, len) = open_regular(path, true)?;
    Error::limit(crate::LimitKind::FileSize, len, limit)?;
    let mut data = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    file.take(limit.saturating_add(1))
        .read_to_end(&mut data)
        .map_err(Error::io(path))?;
    Error::limit(crate::LimitKind::FileSize, data.len() as u64, limit)?;
    Ok(data)
}
