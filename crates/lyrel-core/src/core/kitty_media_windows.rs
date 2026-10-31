//! Host-local Windows media. No filesystem name is resolved by the desktop.
use std::fs::{File, OpenOptions};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Component, Path, PathBuf, Prefix};

use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_TYPE_DISK, FileDispositionInfo, GetDriveTypeW, GetFileType,
    GetVolumeNameForVolumeMountPointW, ReOpenFile, SetFileInformationByHandle,
};
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::Memory::{
    FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use super::{MAX_IMAGE_BYTES, Medium, MediumTransfer};

fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    text.encode_wide().chain(Some(0)).collect()
}

// Reject namespaces, UNC, ADS and DOS devices *before* metadata/open. A mapped
// network drive is not a local disk even if its name starts with a drive letter.
fn disk_path(path: &Path) -> Option<PathBuf> {
    // GetFullPathNameW, used by absolute(), strips trailing dots/spaces from
    // non-verbatim names. Reject that ambiguity before normalization can hide it.
    if path.components().any(|component| match component {
        Component::Normal(name) => name.to_str().is_none_or(|name| name.ends_with(['.', ' '])),
        _ => false,
    }) {
        return None;
    }
    let path = std::path::absolute(path).ok()?;
    let mut components = path.components();
    let Component::Prefix(prefix) = components.next()? else {
        return None;
    };
    let root = match prefix.kind() {
        Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => format!("{}:\\", char::from(drive)),
        Prefix::Verbatim(volume) => {
            let volume = volume.to_str()?;
            let guid = volume.strip_prefix("Volume{")?.strip_suffix('}')?;
            uuid::Uuid::parse_str(guid).ok()?;
            format!(r"\\?\{}\", volume)
        }
        _ => return None,
    };
    let root = wide(std::ffi::OsStr::new(&root));
    // DRIVE_REMOVABLE=2, DRIVE_FIXED=3, DRIVE_RAMDISK=6.
    if !matches!(unsafe { GetDriveTypeW(root.as_ptr()) }, 2 | 3 | 6) {
        return None;
    }
    let mut volume = [0_u16; 64];
    if unsafe {
        GetVolumeNameForVolumeMountPointW(root.as_ptr(), volume.as_mut_ptr(), volume.len() as u32)
    } == 0
    {
        return None;
    }
    let len = volume.iter().position(|&c| c == 0)?;
    // All subsequent metadata/opens use the local volume GUID, not a mutable
    // drive-letter mapping that could be redirected to an SMB share.
    let mut clean = PathBuf::from(String::from_utf16(&volume[..len]).ok()?);
    for component in components {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                if !clean.pop() {
                    return None;
                }
            }
            Component::Normal(name) => {
                let name = name.to_str()?;
                if name.contains([':', '\0']) || name.ends_with(['.', ' ']) {
                    return None;
                }
                let base = name.split('.').next()?.to_ascii_uppercase();
                if matches!(
                    base.as_str(),
                    "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
                ) || base
                    .strip_prefix("COM")
                    .or_else(|| base.strip_prefix("LPT"))
                    .is_some_and(|n| {
                        matches!(
                            n,
                            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                        )
                    })
                {
                    return None;
                }
                clean.push(name);
            }
            _ => return None,
        }
    }
    Some(clean)
}

// Follow ordinary symlinks/junctions, but do not let canonicalize traverse an
// unvalidated reparse into a network/device namespace. Unknown reparse tags do
// not support read_link and fail closed. Bound loops and path expansion.
fn resolve_disk_path(path: &Path) -> Option<PathBuf> {
    let mut path = disk_path(path)?;
    for _ in 0..40 {
        let mut cursor = PathBuf::new();
        let mut changed = false;
        let components: Vec<_> = path.components().collect();
        for (index, component) in components.iter().enumerate() {
            cursor.push(component.as_os_str());
            if !matches!(component, Component::Normal(_)) {
                continue;
            }
            let meta = std::fs::symlink_metadata(&cursor).ok()?;
            if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                let target = std::fs::read_link(&cursor).ok()?;
                let mut replacement = if target.is_absolute() {
                    target
                } else {
                    cursor.parent()?.join(target)
                };
                for rest in &components[index + 1..] {
                    replacement.push(rest.as_os_str());
                }
                path = disk_path(&replacement)?;
                if path.as_os_str().len() > 32760 {
                    return None;
                }
                changed = true;
                break;
            }
            if index + 1 < components.len() && !meta.is_dir() {
                return None;
            }
        }
        if !changed {
            return Some(path);
        }
    }
    None
}

struct LockedPath {
    path: PathBuf,
    // Pin the verified leaf independently of its name, including across a
    // failed DELETE-access request and the subsequent read-only fallback.
    object: File,
    // No FILE_SHARE_DELETE: resolved ancestors cannot become junctions between
    // validation and open/delete. FileSystem checks are repeated on each handle.
    _directories: Vec<File>,
}

impl LockedPath {
    fn open(path: &Path) -> Option<Self> {
        let path = resolve_disk_path(path)?;
        let mut cursor = PathBuf::new();
        let mut directories = Vec::new();
        let components: Vec<_> = path.components().collect();
        for component in &components[..components.len().checked_sub(1)?] {
            cursor.push(component.as_os_str());
            if !matches!(component, Component::Normal(_) | Component::RootDir) {
                continue;
            }
            let directory = OpenOptions::new()
                .access_mode(0)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&cursor)
                .ok()?;
            let attr = directory.metadata().ok()?.file_attributes();
            if attr & FILE_ATTRIBUTE_DIRECTORY == 0 || attr & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return None;
            }
            directories.push(directory);
        }
        let object = OpenOptions::new()
            .access_mode(0)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)
            .ok()?;
        let meta = object.metadata().ok()?;
        if !meta.is_file()
            || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || unsafe { GetFileType(object.as_raw_handle()) } != FILE_TYPE_DISK
        {
            return None;
        }
        Some(Self {
            path,
            object,
            _directories: directories,
        })
    }

    fn eligible_temp(&self) -> bool {
        self.path
            .to_string_lossy()
            .contains("tty-graphics-protocol")
            && resolve_disk_path(&std::env::temp_dir()).is_some_and(|root| {
                let mut path = self.path.components();
                root.components().all(|part| {
                    path.next().is_some_and(|actual| {
                        actual
                            .as_os_str()
                            .to_string_lossy()
                            .eq_ignore_ascii_case(&part.as_os_str().to_string_lossy())
                    })
                }) && path.next().is_some()
            })
    }

    fn file(&self, delete: bool) -> Option<File> {
        // ReOpenFile upgrades access to this verified object, never a new
        // lookup of a name that may now identify an unrelated replacement.
        let raw = unsafe {
            ReOpenFile(
                self.object.as_raw_handle(),
                GENERIC_READ | if delete { DELETE } else { 0 },
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                FILE_FLAG_OPEN_REPARSE_POINT,
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return None;
        }
        let file = unsafe { File::from_raw_handle(raw) };
        let meta = file.metadata().ok()?;
        if !meta.is_file()
            || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK
        {
            return None;
        }
        Some(file)
    }
}

pub(super) fn read_file(transfer: &MediumTransfer) -> Option<Vec<u8>> {
    let name = std::str::from_utf8(&transfer.name).ok()?;
    if name.is_empty() || name.contains('\0') {
        return None;
    }
    let locked = LockedPath::open(Path::new(name))?;
    let delete = transfer.medium == Medium::TempFile && locked.eligible_temp();
    // Lack of DELETE permission must not turn a readable image into a failure.
    let file = locked.file(delete).or_else(|| locked.file(false))?;
    let out = transfer.read_region(file.try_clone().ok()?)?;
    if delete {
        let info = FILE_DISPOSITION_INFO { DeleteFile: true };
        // This exact opened object, never a second path lookup. Failure retains
        // the sender's object and does not invalidate the successful read.
        unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileDispositionInfo,
                (&info as *const FILE_DISPOSITION_INFO).cast(),
                std::mem::size_of_val(&info) as u32,
            );
        }
    }
    Some(out)
}

struct View(MEMORY_MAPPED_VIEW_ADDRESS);
impl Drop for View {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.0);
        }
    }
}

fn map(handle: &OwnedHandle, base: u64, extent: usize) -> Option<View> {
    if extent == 0 {
        return None;
    }
    let mapped = unsafe {
        MapViewOfFile(
            handle.as_raw_handle(),
            FILE_MAP_READ,
            (base >> 32) as u32,
            base as u32,
            extent,
        )
    };
    (!mapped.Value.is_null()).then_some(View(mapped))
}

pub(super) fn read_shared(transfer: &MediumTransfer) -> Option<Vec<u8>> {
    let name = std::str::from_utf8(&transfer.name).ok()?;
    // Only the documented local/global named-object namespace, no paths or
    // private object-manager namespaces. Windows closes, never unlinks, shm.
    let component = name
        .strip_prefix("Local\\")
        .or_else(|| name.strip_prefix("Global\\"))
        .unwrap_or(name);
    if component.is_empty() || component.contains(['\\', '/', '\0', ':']) || name.len() > 4096 {
        return None;
    }
    let raw =
        unsafe { OpenFileMappingW(FILE_MAP_READ, 0, wide(std::ffi::OsStr::new(name)).as_ptr()) };
    if raw.is_null() {
        return None;
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut info: SYSTEM_INFO = unsafe { std::mem::zeroed() };
    unsafe {
        GetSystemInfo(&mut info);
    }
    let granularity = usize::try_from(info.dwAllocationGranularity).ok()?;
    if granularity == 0 || granularity > 65536 {
        return None;
    }
    let offset = transfer.offset as usize;
    let base = offset / granularity * granularity;
    let displacement = offset - base;
    let len = if transfer.size != 0 {
        let len = transfer.size as usize;
        if len > MAX_IMAGE_BYTES {
            return None;
        }
        len
    } else {
        // Never use MapViewOfFile(..., 0): that maps an unbounded object. Probe
        // the end using bounded views, one live view at a time, then map only
        // the accepted region. Windows sections have page-rounded extents.
        let fits = |len: usize| map(&handle, base as u64, displacement + len).is_some();
        if !fits(1) || fits(MAX_IMAGE_BYTES + 1) {
            return None;
        }
        let (mut low, mut high) = (1, MAX_IMAGE_BYTES);
        while low < high {
            let middle = low + (high - low + 1) / 2;
            if fits(middle) {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        low
    };
    let extent = displacement.checked_add(len)?;
    let mapped = map(&handle, base as u64, extent)?;
    let mut out = vec![0; len];
    let mut copied = 0;
    // The OS copy validates accessibility and reports in-page errors instead
    // of exposing a Rust slice to attacker-controlled file-backed mappings.
    let ok = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            mapped.0.Value.cast::<u8>().add(displacement).cast(),
            out.as_mut_ptr().cast(),
            len,
            &mut copied,
        )
    };
    (ok != 0 && copied == len).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_names_reject_devices_network_streams_and_namespaces_before_open() {
        for name in [
            r"\\server\share\a",
            r"\\.\pipe\test",
            r"\\?\UNC\host\share\a",
            r"C:\a:stream",
            r"C:\NUL.png",
            r"C:\COM1",
            r"C:\LPT¹.txt",
            r"C:\a.\image",
        ] {
            assert!(disk_path(Path::new(name)).is_none(), "{name}");
        }
    }

    #[test]
    fn validated_handoff_reads_and_deletes_original_after_identity_swap() {
        use std::io::Read as _;

        for delete in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("tty-graphics-protocol.rgba");
            let original = dir.path().join("original.rgba");
            std::fs::write(&path, [1, 2, 3, 4]).unwrap();
            let locked = LockedPath::open(&path).unwrap();
            assert!(locked.eligible_temp());

            // Replace the name after validation but before acquiring read/DELETE access.
            std::fs::rename(&path, &original).unwrap();
            std::fs::write(&path, [9, 8, 7, 6]).unwrap();
            let mut file = locked.file(delete).unwrap();
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, [1, 2, 3, 4]);
            if delete {
                let info = FILE_DISPOSITION_INFO { DeleteFile: true };
                assert_ne!(
                    unsafe {
                        SetFileInformationByHandle(
                            file.as_raw_handle(),
                            FileDispositionInfo,
                            (&info as *const FILE_DISPOSITION_INFO).cast(),
                            std::mem::size_of_val(&info) as u32,
                        )
                    },
                    0
                );
            }
            drop(file);
            drop(locked);
            assert_eq!(std::fs::read(&path).unwrap(), [9, 8, 7, 6]);
            assert_eq!(original.exists(), !delete);
        }
    }

    #[test]
    fn delete_access_fallback_reads_verified_object_not_replacement() {
        use std::io::Read as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tty-graphics-protocol.rgba");
        let original = dir.path().join("original.rgba");
        std::fs::write(&path, [1, 2, 3, 4]).unwrap();
        let locked = LockedPath::open(&path).unwrap();
        std::fs::rename(&path, &original).unwrap();
        std::fs::write(&path, [9, 8, 7, 6]).unwrap();

        // Another reader permits reads but deterministically denies DELETE access.
        let reader = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&original)
            .unwrap();
        assert!(locked.file(true).is_none());
        let mut file = locked.file(true).or_else(|| locked.file(false)).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, [1, 2, 3, 4]);
        drop(file);
        drop(reader);
        drop(locked);
        assert_eq!(std::fs::read(&original).unwrap(), [1, 2, 3, 4]);
        assert_eq!(std::fs::read(&path).unwrap(), [9, 8, 7, 6]);
    }

    #[test]
    fn opened_temp_and_ancestors_cannot_be_replaced_before_handle_delete() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("owned");
        std::fs::create_dir(&parent).unwrap();
        let path = parent.join("tty-graphics-protocol.rgba");
        std::fs::write(&path, [1, 2, 3, 4]).unwrap();
        let locked = LockedPath::open(&path).unwrap();
        let file = locked.file(true).unwrap();
        assert!(std::fs::rename(&path, parent.join("other")).is_err());
        assert!(std::fs::rename(&parent, dir.path().join("replaced")).is_err());
        let info = FILE_DISPOSITION_INFO { DeleteFile: true };
        assert_ne!(
            unsafe {
                SetFileInformationByHandle(
                    file.as_raw_handle(),
                    FileDispositionInfo,
                    (&info as *const FILE_DISPOSITION_INFO).cast(),
                    std::mem::size_of_val(&info) as u32,
                )
            },
            0
        );
        drop(file);
        drop(locked);
        assert!(!path.exists());
    }
}
