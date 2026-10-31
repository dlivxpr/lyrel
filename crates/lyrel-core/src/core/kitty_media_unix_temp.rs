//! Identity-safe POSIX temporary handoff consumption. There is deliberately no
//! recursive temporary-directory owner: an unknown raced object must survive.
use super::MediumTransfer;
use std::ffi::CString;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

struct Capture {
    parent: File,
    directory: File,
    private: CString,
    original: CString,
    recovery: std::path::PathBuf,
    consumed: bool,
}

impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            if !self.consumed {
                // Exclusive rename preserves the original object (including
                // symlinks/directories) without unlinking or overwriting it.
                #[cfg(target_os = "linux")]
                let restored = libc::renameat2(
                    self.directory.as_raw_fd(),
                    c"object".as_ptr(),
                    self.parent.as_raw_fd(),
                    self.original.as_ptr(),
                    libc::RENAME_NOREPLACE,
                ) == 0;
                #[cfg(target_os = "macos")]
                let restored = libc::renameatx_np(
                    self.directory.as_raw_fd(),
                    c"object".as_ptr(),
                    self.parent.as_raw_fd(),
                    self.original.as_ptr(),
                    libc::RENAME_EXCL,
                ) == 0;
                #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                let restored = false;
                if !restored {
                    log::warn!(
                        "graphics handoff preserved at {}: {}",
                        self.recovery.display(),
                        std::io::Error::last_os_error()
                    );
                }
            }
            // Never recurse: an unknown/replaced object keeps its directory.
            libc::unlinkat(
                self.parent.as_raw_fd(),
                self.private.as_ptr(),
                libc::AT_REMOVEDIR,
            );
        }
    }
}

pub(super) fn read_handoff(transfer: &MediumTransfer, path: &Path, file: File) -> Option<Vec<u8>> {
    consume(transfer, path, file, || {}, || {})
}

fn consume(
    transfer: &MediumTransfer,
    path: &Path,
    file: File,
    before_capture: impl FnOnce(),
    after_capture: impl FnOnce(),
) -> Option<Vec<u8>> {
    let identity = file.metadata().ok()?;
    if !identity.is_file() {
        return None;
    }
    let parent_path = path.parent()?;
    let parent = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(parent_path)
        .ok()?;
    let original = CString::new(path.file_name()?.as_encoded_bytes()).ok()?;
    let private = CString::new(format!(".lyrel-graphics-{}", uuid::Uuid::new_v4())).ok()?;
    if unsafe { libc::mkdirat(parent.as_raw_fd(), private.as_ptr(), 0o700) } != 0 {
        return None;
    }
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            private.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        unsafe {
            libc::unlinkat(parent.as_raw_fd(), private.as_ptr(), libc::AT_REMOVEDIR);
        }
        return None;
    }
    let directory = unsafe { File::from_raw_fd(fd) };
    before_capture();
    if unsafe {
        libc::renameat(
            parent.as_raw_fd(),
            original.as_ptr(),
            directory.as_raw_fd(),
            c"object".as_ptr(),
        )
    } != 0
    {
        unsafe {
            libc::unlinkat(parent.as_raw_fd(), private.as_ptr(), libc::AT_REMOVEDIR);
        }
        return None;
    }
    let mut capture = Capture {
        recovery: parent_path
            .join(std::ffi::OsStr::from_bytes(private.as_bytes()))
            .join("object"),
        parent,
        directory,
        private,
        original,
        consumed: false,
    };
    after_capture();
    let mut captured: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::fstatat(
            capture.directory.as_raw_fd(),
            c"object".as_ptr(),
            &mut captured,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
        || captured.st_mode & libc::S_IFMT != libc::S_IFREG
        || identity.dev() != captured.st_dev as u64
        || identity.ino() != captured.st_ino as u64
    {
        return None;
    }
    let out = transfer.read_region(file)?;
    if unsafe { libc::unlinkat(capture.directory.as_raw_fd(), c"object".as_ptr(), 0) } != 0 {
        return None;
    }
    capture.consumed = true;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::kitty_graphics::{Medium, WireFormat};
    #[test]
    fn replacement_is_restored_without_reading_or_deleting_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tty-graphics-protocol-race");
        let old = dir.path().join("original");
        std::fs::write(&path, [1, 2, 3, 4]).unwrap();
        let file = File::open(&path).unwrap();
        let transfer = MediumTransfer {
            medium: Medium::TempFile,
            name: Vec::new(),
            offset: 0,
            size: 4,
            width: 1,
            height: 1,
            format: WireFormat::Rgba,
            compressed: false,
        };
        assert!(
            consume(
                &transfer,
                &path,
                file,
                || {
                    std::fs::rename(&path, &old).unwrap();
                    std::fs::write(&path, [9, 8, 7, 6]).unwrap();
                },
                || {}
            )
            .is_none()
        );
        assert_eq!(std::fs::read(&path).unwrap(), [9, 8, 7, 6]);
        assert_eq!(std::fs::read(&old).unwrap(), [1, 2, 3, 4]);
    }

    #[test]
    fn replacement_restore_conflict_preserves_both_objects_without_recursive_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tty-graphics-protocol-race");
        let old = dir.path().join("original");
        std::fs::write(&path, [1, 2, 3, 4]).unwrap();
        let file = File::open(&path).unwrap();
        let transfer = MediumTransfer {
            medium: Medium::TempFile,
            name: Vec::new(),
            offset: 0,
            size: 4,
            width: 1,
            height: 1,
            format: WireFormat::Rgba,
            compressed: false,
        };
        assert!(
            consume(
                &transfer,
                &path,
                file,
                || {
                    std::fs::rename(&path, &old).unwrap();
                    std::fs::write(&path, [9, 8, 7, 6]).unwrap();
                },
                || {
                    std::fs::write(&path, [5, 5, 5, 5]).unwrap();
                }
            )
            .is_none()
        );
        assert_eq!(std::fs::read(&path).unwrap(), [5, 5, 5, 5]);
        assert_eq!(std::fs::read(&old).unwrap(), [1, 2, 3, 4]);
        let recovery = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".lyrel-graphics-")
            })
            .unwrap();
        assert_eq!(
            std::fs::read(recovery.join("object")).unwrap(),
            [9, 8, 7, 6]
        );
    }
}
