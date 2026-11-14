//! 一次打开绑定到的文件版本：对象身份 + 变化元数据。
//!
//! 只比较 `(len, mtime)` 无法识别“同尺寸改写后把 mtime 改回去”的文件。本模块
//! 在真实平台取最小的可靠组合：
//!
//! - Unix：`(dev, ino)` 标识打开的对象；`ctime` 由内核在任何写入或元数据
//!   变更后推进，用户态只能设置 mtime，不能把 ctime 改回去。
//! - Windows：卷序号 + 文件索引标识对象；NTFS `ChangeTime` 由系统在写入后
//!   推进，`SetFileTime` 只能写回 creation/access/write 时间。
//!
//! 拿不到变化时间（例如不维护 change time 的文件系统）时返回 `None`，调用
//! 方据此报告“版本化读取不可用”，而不是把弱校验当成一致性证明。

use std::fs::File;
use std::io;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

/// 一个打开文件的版本指纹。`volume`/`index` 在文件存在期间唯一标识同一对象。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileVersion {
    len: u64,
    modified: u64,
    changed: u64,
    volume: u64,
    index: u64,
}

#[cfg(unix)]
fn stamp(secs: i64, nanos: i64) -> u64 {
    // Injective for the `(secs, nanos < 1e9)` pairs the platform produces; the
    // bit pattern of a pre-1970 timestamp still compares consistently.
    ((secs as u64) << 32) | (nanos as u32 as u64)
}

impl FileVersion {
    /// The version of an already-open file, or `None` when this volume cannot
    /// supply change metadata.
    pub(crate) fn of_open_file(file: &File) -> io::Result<Option<Self>> {
        #[cfg(unix)]
        {
            let md = file.metadata()?;
            Ok(Some(Self {
                len: md.len(),
                modified: stamp(md.mtime(), md.mtime_nsec()),
                changed: stamp(md.ctime(), md.ctime_nsec()),
                volume: md.dev(),
                index: md.ino(),
            }))
        }
        #[cfg(windows)]
        {
            let identity = win_identity(file)?;
            let Some(changed) = win_change_time(file)? else {
                return Ok(None);
            };
            Ok(Some(Self {
                len: identity.len,
                modified: identity.modified,
                changed,
                volume: identity.volume,
                index: identity.index,
            }))
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = file;
            Ok(None)
        }
    }

    /// Whether `path` still names this very object with the same length and
    /// write time. A removed path counts as a change, not a read failure.
    pub(crate) fn matches_path(&self, path: &Path) -> io::Result<bool> {
        #[cfg(unix)]
        {
            let md = match std::fs::metadata(path) {
                Ok(md) => md,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(error),
            };
            Ok(md.len() == self.len
                && md.dev() == self.volume
                && md.ino() == self.index
                && stamp(md.mtime(), md.mtime_nsec()) == self.modified)
        }
        #[cfg(windows)]
        {
            let file = match metadata_handle(path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(error),
            };
            let info = match win_identity(&file) {
                Ok(info) => info,
                // A path that now names something without a file index (a
                // directory replaced the file, for instance) is not this
                // version.
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => return Ok(false),
                Err(error) => return Err(error),
            };
            Ok(info.len == self.len
                && info.volume == self.volume
                && info.index == self.index
                && info.modified == self.modified)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
            Ok(false)
        }
    }
}

/// Opens `path` with no read or write access, only enough for metadata.
///
/// Requesting zero access keeps the query possible while a writer holds the
/// file; Rust's open flags still share read, write and delete with everyone.
#[cfg(windows)]
fn metadata_handle(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().access_mode(0).open(path)
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug)]
struct WinIdentity {
    len: u64,
    modified: u64,
    volume: u64,
    index: u64,
}

#[cfg(windows)]
fn win_identity(file: &File) -> io::Result<WinIdentity> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let mut by_handle = BY_HANDLE_FILE_INFORMATION::default();
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut by_handle) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let ticks =
        |value: FILETIME| (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime);
    Ok(WinIdentity {
        len: (u64::from(by_handle.nFileSizeHigh) << 32) | u64::from(by_handle.nFileSizeLow),
        modified: ticks(by_handle.ftLastWriteTime),
        volume: u64::from(by_handle.dwVolumeSerialNumber),
        index: (u64::from(by_handle.nFileIndexHigh) << 32) | u64::from(by_handle.nFileIndexLow),
    })
}

/// `None` when the volume does not maintain a change time.
#[cfg(windows)]
fn win_change_time(file: &File) -> io::Result<Option<u64>> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_BASIC_INFO, FileBasicInfo, GetFileInformationByHandleEx,
    };

    let mut info = FILE_BASIC_INFO::default();
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileBasicInfo,
            std::ptr::from_mut(&mut info).cast(),
            u32::try_from(std::mem::size_of::<FILE_BASIC_INFO>())
                .expect("FILE_BASIC_INFO fits u32"),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((info.ChangeTime > 0).then_some(info.ChangeTime as u64))
}
