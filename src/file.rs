//! Opening files: regular files only, and never a blocking open on a FIFO or device. Files
//! named by untrusted data (stream files) also may not be symbolic links or carry other hard
//! links; a file the caller chose may be a link.
//!
//! Where the platform's flag values are known, the open itself does not block, does not take
//! a controlling terminal, and (for stream files) refuses links. On Unix the opened file must
//! be the one that was checked. On Windows and other platforms a file swapped in between check
//! and open is not detected.

use crate::{Error, Result};

use std::fs::{File, Metadata, OpenOptions};
use std::path::Path;

/// Who chose the file.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Chosen {
    /// The caller: links are followed.
    ByCaller,
    /// The data: no symbolic links, no other hard links.
    ByData,
}

/// Open `path` for reading if it is a regular file, following [`Chosen`]'s rules.
pub(crate) fn open_regular(path: &Path, chosen: Chosen) -> Result<(File, u64)> {
    let before = match chosen {
        Chosen::ByCaller => std::fs::metadata(path),
        Chosen::ByData => std::fs::symlink_metadata(path),
    }
    .map_err(Error::io(path))?;
    if !before.file_type().is_file() {
        return Err(not_regular(path, chosen));
    }
    let file = open_flagged(path, chosen)?;
    let after = file.metadata().map_err(Error::io(path))?;
    check_opened(path, chosen, &before, &after)?;
    Ok((file, after.len()))
}

fn not_regular(path: &Path, chosen: Chosen) -> Error {
    match chosen {
        Chosen::ByCaller => Error::Io {
            path: path.to_path_buf(),
            error: std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file"),
        },
        Chosen::ByData => Error::Unsupported(format!(
            "{:?} is not a regular file",
            path.display().to_string()
        )),
    }
}

/// The opened file must be a regular file, the one checked before opening, and, when the
/// data chose it, have no other hard links (a hard link reaches outside the folder as surely
/// as a symbolic one).
fn check_opened(path: &Path, chosen: Chosen, before: &Metadata, after: &Metadata) -> Result<()> {
    if !after.is_file() {
        return Err(not_regular(path, chosen));
    }
    if !same_file(before, after) {
        return Err(Error::Unsupported(format!(
            "{:?} changed while it was being opened",
            path.display().to_string()
        )));
    }
    if chosen == Chosen::ByData && links(after) > 1 {
        return Err(Error::Unsupported(format!(
            "{:?} has other hard links",
            path.display().to_string()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn same_file(a: &Metadata, b: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
const fn same_file(_: &Metadata, _: &Metadata) -> bool {
    true
}

#[cfg(unix)]
fn links(m: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    m.nlink()
}

#[cfg(not(unix))]
const fn links(_: &Metadata) -> u64 {
    1
}

/// Which file an open handle is, however its path was spelled: device and inode on Unix, the
/// lower-cased file name elsewhere (so `A.resS` and `a.resS` are one file on a case-blind
/// system, and possibly two different files are taken for one: the safe mistake).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum FileId {
    #[cfg(unix)]
    Inode(u64, u64),
    #[cfg(not(unix))]
    Name(String),
}

pub(crate) fn identity(file: &File, path: &Path) -> Result<FileId> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = file.metadata().map_err(Error::io(path))?;
        Ok(FileId::Inode(m.dev(), m.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Ok(FileId::Name(
            path.file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default(),
        ))
    }
}

/// `(O_NONBLOCK, O_NOFOLLOW, O_NOCTTY)` for this target, where the values are certain.
#[allow(
    clippy::unnecessary_wraps,
    reason = "always Some on some targets, always None on others"
)]
const fn flags() -> Option<(i32, i32, i32)> {
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "tvos",
        target_os = "watchos",
        target_os = "visionos"
    ))]
    return Some((0x0004, 0x0100, 0x20000));
    #[cfg(any(
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    return Some((0x0004, 0x0100, 0x8000));
    #[cfg(any(target_os = "illumos", target_os = "solaris"))]
    return Some((0x80, 0x20000, 0x800));
    #[cfg(all(
        any(target_os = "linux", target_os = "android"),
        any(
            target_arch = "x86_64",
            target_arch = "x86",
            target_arch = "riscv64",
            target_arch = "loongarch64",
            target_arch = "s390x"
        )
    ))]
    return Some((0o4000, 0o400000, 0o400));
    #[cfg(all(
        any(target_os = "linux", target_os = "android"),
        any(
            target_arch = "aarch64",
            target_arch = "arm",
            target_arch = "powerpc",
            target_arch = "powerpc64"
        )
    ))]
    return Some((0o4000, 0o100000, 0o400));
    #[cfg(all(target_os = "linux", any(target_arch = "mips", target_arch = "mips64")))]
    return Some((0x80, 0x20000, 0x800));
    #[cfg(all(target_os = "linux", target_arch = "sparc64"))]
    return Some((0x4000, 0x20000, 0x8000));
    #[allow(
        unreachable_code,
        reason = "reached only on targets without a table entry"
    )]
    None
}

/// Open with the flags above: never blocking, never a controlling terminal, and no links
/// when the data chose the file.
pub(crate) fn open_flagged(path: &Path, chosen: Chosen) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    if let Some((nonblock, nofollow, noctty)) = flags() {
        use std::os::unix::fs::OpenOptionsExt;
        let follow = if chosen == Chosen::ByData {
            nofollow
        } else {
            0
        };
        options.custom_flags(nonblock | noctty | follow);
    }
    #[cfg(not(unix))]
    let _ = chosen;
    options.open(path).map_err(Error::io(path))
}

/// How much of a file its first bytes justify reading.
pub(crate) type HeadCheck = fn(head: &[u8], len: u64) -> Result<u64>;

/// Bytes read before [`HeadCheck`] decides on the rest.
const HEAD: u64 = 64 * 1024;

/// Read a file the caller chose, under a size limit, refusing anything irregular. The first
/// bytes are read and checked before the rest, so a large file that is not what its header
/// claims (a sparse file, say) is refused before it is read; `check` says how many bytes to
/// read in all. A file that grows past the limit while being read is refused too.
pub(crate) fn read_limited(path: &Path, limit: u64, check: HeadCheck) -> Result<Vec<u8>> {
    use std::io::Read;
    let (mut file, len) = open_regular(path, Chosen::ByCaller)?;
    Error::limit(crate::LimitKind::FileSize, len, limit)?;
    let mut data = Vec::new();
    file.by_ref()
        .take(HEAD.min(len))
        .read_to_end(&mut data)
        .map_err(Error::io(path))?;
    let want = check(&data, len)?.min(len);
    data.truncate(usize::try_from(want).unwrap_or(usize::MAX));
    data.try_reserve_exact(
        usize::try_from(want)
            .unwrap_or(usize::MAX)
            .saturating_sub(data.len()),
    )
    .map_err(|_| Error::LimitExceeded {
        kind: crate::LimitKind::FileSize,
        value: want,
        limit,
    })?;
    let rest = want.saturating_sub(data.len() as u64);
    file.take(rest.min(limit.saturating_add(1)))
        .read_to_end(&mut data)
        .map_err(Error::io(path))?;
    Error::limit(crate::LimitKind::FileSize, data.len() as u64, limit)?;
    Ok(data)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("uba-file-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn test_a_file_swapped_after_the_check_is_refused() {
        let d = dir("swap");
        let (a, b) = (d.join("a.resS"), d.join("b.resS"));
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        let before = std::fs::symlink_metadata(&a).unwrap();
        let after = File::open(&b).unwrap().metadata().unwrap();
        let err = check_opened(&b, Chosen::ByData, &before, &after).unwrap_err();
        assert!(err.to_string().contains("changed"), "{err}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn test_the_open_itself_refuses_symbolic_links() {
        assert!(flags().is_some(), "flags unknown for this target");
        let d = dir("nofollow");
        std::fs::write(d.join("real"), b"x").unwrap();
        std::os::unix::fs::symlink(d.join("real"), d.join("link.resS")).unwrap();
        // Skip the metadata check and open directly: the flags alone must refuse the link.
        assert!(open_flagged(&d.join("link.resS"), Chosen::ByData).is_err());
        assert!(open_flagged(&d.join("link.resS"), Chosen::ByCaller).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn test_only_what_the_head_justifies_is_read() {
        let d = dir("head");
        let f = std::fs::File::create(d.join("sparse")).unwrap();
        f.set_len(1 << 30).unwrap(); // 1 GiB, all holes
        drop(f);
        let data = read_limited(&d.join("sparse"), 2 << 30, |head, len| {
            assert_eq!((head.len(), len), (64 * 1024, 1 << 30));
            Ok(100)
        })
        .unwrap();
        assert_eq!(data.len(), 100);
        let refused = read_limited(&d.join("sparse"), 2 << 30, |_, _| {
            Err(Error::NotUnity("test".into()))
        });
        assert!(matches!(refused, Err(Error::NotUnity(_))));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn test_a_read_only_file_opens() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("readonly");
        let p = d.join("r.assets");
        std::fs::write(&p, b"x").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o444)).unwrap();
        for chosen in [Chosen::ByCaller, Chosen::ByData] {
            assert!(open_flagged(&p, chosen).is_ok());
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn test_hard_links_are_refused_for_data_but_not_the_caller() {
        let d = dir("hardlink");
        std::fs::write(d.join("real"), b"x").unwrap();
        std::fs::hard_link(d.join("real"), d.join("x.resS")).unwrap();
        let err = open_regular(&d.join("x.resS"), Chosen::ByData).unwrap_err();
        assert!(err.to_string().contains("hard links"), "{err}");
        assert!(open_regular(&d.join("x.resS"), Chosen::ByCaller).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }
}
