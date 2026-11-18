// Disposable native objects shared by the real PTY / daemon / rooted consumer.
pub struct SharedMedia {
    pub name: String,
    #[cfg(windows)]
    _handle: std::os::windows::io::OwnedHandle,
    #[cfg(windows)]
    object_len: usize,
    #[cfg(unix)]
    name_c: std::ffi::CString,
}

impl SharedMedia {
    pub fn new(object_len: usize, offset: usize, bytes: &[u8]) -> Self {
        #[cfg(windows)]
        unsafe {
            use std::os::windows::io::FromRawHandle;
            use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
            use windows_sys::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_WRITE, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile};
            let name = format!("Local\\lyrel-media-{}", uuid::Uuid::new_v4());
            let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            let raw = CreateFileMappingW(INVALID_HANDLE_VALUE, std::ptr::null(), PAGE_READWRITE, 0, object_len.try_into().unwrap(), wide.as_ptr());
            assert!(!raw.is_null(), "CreateFileMappingW: {}", std::io::Error::last_os_error());
            let handle = std::os::windows::io::OwnedHandle::from_raw_handle(raw);
            // The owned sender fixture may map its entire allocation; the
            // production receiver must independently bound its mapping/copy.
            let view = MapViewOfFile(raw, FILE_MAP_WRITE, 0, 0, object_len);
            assert!(!view.Value.is_null());
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), view.Value.cast::<u8>().add(offset), bytes.len());
            assert_ne!(UnmapViewOfFile(view), 0);
            Self { name, _handle: handle, object_len }
        }
        #[cfg(unix)]
        unsafe {
            use std::os::fd::{AsRawFd, FromRawFd};
            let name = format!("/lyrel-m-{:016x}", uuid::Uuid::new_v4().as_u128() as u64);
            let name_c = std::ffi::CString::new(name.clone()).unwrap();
            let fd = libc::shm_open(name_c.as_ptr(), libc::O_CREAT | libc::O_EXCL | libc::O_RDWR, 0o600);
            assert!(fd >= 0, "shm_open: {}", std::io::Error::last_os_error());
            let file = std::fs::File::from_raw_fd(fd);
            file.set_len(object_len as u64).unwrap();
            let mapped = libc::mmap(std::ptr::null_mut(), object_len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, file.as_raw_fd(), 0);
            assert_ne!(mapped, libc::MAP_FAILED);
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.cast::<u8>().add(offset), bytes.len());
            assert_eq!(libc::munmap(mapped, object_len), 0);
            Self { name, name_c }
        }
    }

    #[cfg(windows)]
    pub fn len(&self) -> u64 {
        self.object_len as u64
    }

    #[cfg(unix)]
    pub fn len(&self) -> u64 {
        use std::os::fd::FromRawFd;
        // Darwin reports the stable allocation length rounded to its VM page,
        // not the original ftruncate argument. Test the actual public bound.
        let fd = unsafe { libc::shm_open(self.name_c.as_ptr(), libc::O_RDONLY, 0) };
        assert!(fd >= 0, "shm_open length: {}", std::io::Error::last_os_error());
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        file.metadata().unwrap().len()
    }
}

#[cfg(unix)]
impl Drop for SharedMedia {
    fn drop(&mut self) { unsafe { libc::shm_unlink(self.name_c.as_ptr()); } }
}

pub fn region_file(path: &std::path::Path, object_len: u64, offset: u64, bytes: &[u8]) {
    use std::io::{Seek, SeekFrom, Write};
    let mut file = std::fs::File::create(path).unwrap();
    file.set_len(object_len).unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(bytes).unwrap();
}
