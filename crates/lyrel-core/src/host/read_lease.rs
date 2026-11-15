//! 读取期间的写入租约：版本一致性由内核的变更通知保证，而不是时间戳。
//!
//! Unix 的 `ctime` 由内核在任何写入后推进，用户态只能设置 mtime，改不回去，
//! [`FileVersion`](super::file_version) 的逐段复核就是可靠证据。Windows 不同：
//! `LastWriteTime`／`ChangeTime` 虽以 100ns 为单位存储，实际取值跟随系统时钟
//! 中断粒度（常见 15.6ms），同一 tick 内两次等长写入可以得到完全相同的两个
//! 时间戳；`SetFileTime` 又允许把写时间写回旧值。因此这两个字段既不是单调
//! 版本号，也不能证明“这期间没有写入”。
//!
//! 这里的 [`ReadLease`] 在本地读取会话期间对同一文件另开一个只用于通知的
//! 异步句柄，向卷申请 read-cache opportunistic lock（`OPLOCK_LEVEL_CACHE_READ`，
//! 等价于 Level 2）。oplock 的 break 是 advisory 的：写者不必等待确认，写入照常
//! 完成；但任何以写方式打开或写入都会让内核完成挂起的请求并 signal 它的 event，
//! 读取方在每段前后查看即可，不依赖 100ns 字段是否变化。租约在打开数据句柄之前
//! 取得，因此会话建立后的写入都由内核通知，而不是靠时间戳事后推断。
//!
//! 申请不到租约时 [`ReadLease::acquire`] 返回 `Ok(None)`：远端 SMB 重定向器
//! 明确拒绝客户端直接申请 oplock，不维护 oplock 的卷也可能拒绝。调用方据此
//! 报告版本化读取不可用，而不是退回到“时间戳看起来没变”的弱校验。

#[cfg(windows)]
mod windows {
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::mem::size_of;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_IO_PENDING, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;
    use windows_sys::Win32::System::IO::{CancelIoEx, DeviceIoControl, OVERLAPPED};
    use windows_sys::Win32::System::Ioctl::{
        FSCTL_REQUEST_OPLOCK, OPLOCK_LEVEL_CACHE_READ, REQUEST_OPLOCK_CURRENT_VERSION,
        REQUEST_OPLOCK_INPUT_BUFFER, REQUEST_OPLOCK_INPUT_FLAG_REQUEST,
        REQUEST_OPLOCK_OUTPUT_BUFFER,
    };
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    /// How long `Drop` waits for a canceled request to complete. Cancellation
    /// itself is immediate; the bound only keeps a broken filter driver from
    /// hanging the session on release.
    const RELEASE_TIMEOUT_MS: u32 = 1_000;

    /// One read-cache oplock held for the life of a local read session.
    pub(crate) struct ReadLease {
        /// `Option` so `Drop` can close the handle before the buffers below are
        /// freed: closing the handle a request was issued on completes it.
        file: Option<File>,
        event: HANDLE,
        /// The I/O manager keeps pointers into these until the pending request
        /// completes, so both live in stable heap boxes that outlive it.
        overlapped: Box<OVERLAPPED>,
        output: Box<REQUEST_OPLOCK_OUTPUT_BUFFER>,
    }

    impl ReadLease {
        /// Requests a read-cache oplock on its own handle for `path`.
        ///
        /// `Ok(None)` means this volume or path cannot grant one (a network
        /// redirector refuses client-requested oplocks), so a versioned read
        /// has no proof and must be reported unavailable rather than weak.
        pub(crate) fn acquire(path: &Path) -> io::Result<Option<Self>> {
            let file = OpenOptions::new()
                .access_mode(0)
                .custom_flags(FILE_FLAG_OVERLAPPED)
                .open(path)?;
            let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
            if event.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut overlapped = Box::new(OVERLAPPED::default());
            overlapped.hEvent = event;
            let mut output = Box::new(REQUEST_OPLOCK_OUTPUT_BUFFER::default());
            output.StructureVersion = REQUEST_OPLOCK_CURRENT_VERSION as u16;
            output.StructureLength = u16::try_from(size_of::<REQUEST_OPLOCK_OUTPUT_BUFFER>())
                .expect("the request oplock output buffer fits u16");
            let input = REQUEST_OPLOCK_INPUT_BUFFER {
                StructureVersion: REQUEST_OPLOCK_CURRENT_VERSION as u16,
                StructureLength: u16::try_from(size_of::<REQUEST_OPLOCK_INPUT_BUFFER>())
                    .expect("the request oplock input buffer fits u16"),
                RequestedOplockLevel: OPLOCK_LEVEL_CACHE_READ,
                Flags: REQUEST_OPLOCK_INPUT_FLAG_REQUEST,
            };
            let mut returned = 0u32;
            let started = unsafe {
                DeviceIoControl(
                    file.as_raw_handle(),
                    FSCTL_REQUEST_OPLOCK,
                    ptr::from_ref(&input).cast(),
                    u32::try_from(size_of::<REQUEST_OPLOCK_INPUT_BUFFER>())
                        .expect("the request oplock input buffer fits u32"),
                    ptr::from_mut(&mut *output).cast(),
                    u32::try_from(size_of::<REQUEST_OPLOCK_OUTPUT_BUFFER>())
                        .expect("the request oplock output buffer fits u32"),
                    &mut returned,
                    &mut *overlapped,
                )
            };
            if started != 0 {
                // A synchronous success means no oplock was pended; nothing
                // holds these buffers, and there is no lease to keep.
                unsafe { CloseHandle(event) };
                return Ok(None);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                return Ok(Some(Self {
                    file: Some(file),
                    event,
                    overlapped,
                    output,
                }));
            }
            unsafe { CloseHandle(event) };
            // Any other result means no oplock was pended: a network redirector
            // denies client-requested oplocks, and a volume may not implement
            // them. Either way a versioned read has no proof here; the caller
            // reports Unsupported, and this line keeps the reason visible
            // without turning it into a read failure the consumer cannot use.
            log::debug!("no read lease for {}: {error}", path.display());
            Ok(None)
        }

        /// Whether a writer opened or wrote the file since the lease was made.
        ///
        /// A manual-reset event keeps the break observable to every later
        /// check; it is signaled only after the I/O manager completed the
        /// request, which is also what makes the request's buffers safe to free.
        pub(crate) fn broke(&self) -> bool {
            let signaled = unsafe { WaitForSingleObject(self.event, 0) == WAIT_OBJECT_0 };
            if signaled {
                // Completion wrote `output`; its levels are the volume's own
                // account of the break (zero means broken to none), and reading
                // them is also what keeps the buffer a live field rather than
                // an unused allocation.
                log::debug!(
                    "read lease broke from 0x{:x} to 0x{:x}",
                    self.output.OriginalOplockLevel,
                    self.output.NewOplockLevel
                );
            }
            signaled
        }

        fn wait_for_release(&self) -> bool {
            unsafe { WaitForSingleObject(self.event, RELEASE_TIMEOUT_MS) == WAIT_OBJECT_0 }
        }
    }

    impl Drop for ReadLease {
        fn drop(&mut self) {
            // A pending oplock request owns the pointers into `overlapped` and
            // `output` until it completes. Cancel it, and free nothing before
            // the completion it guarantees arrives.
            if self.broke() {
                // The break already completed the request.
            } else if let Some(file) = self.file.as_ref() {
                unsafe { CancelIoEx(file.as_raw_handle(), ptr::from_ref(&*self.overlapped)) };
            }
            if !self.wait_for_release() {
                // Closing the handle the request was issued on completes it
                // with STATUS_OPLOCK_HANDLE_CLOSED.
                self.file = None;
                if !self.wait_for_release() {
                    // The I/O manager still owns both buffers; handing them
                    // back now would leave it writing into freed memory. One
                    // leaked request is the only memory-safe end.
                    log::error!(
                        "a Windows read lease did not complete after cancel and close; leaking its overlapped request"
                    );
                    std::mem::forget(std::mem::replace(
                        &mut self.overlapped,
                        Box::new(OVERLAPPED::default()),
                    ));
                    std::mem::forget(std::mem::replace(
                        &mut self.output,
                        Box::new(REQUEST_OPLOCK_OUTPUT_BUFFER::default()),
                    ));
                }
            }
            unsafe { CloseHandle(self.event) };
        }
    }

    // The session that owns a lease is what `HostRead: Send` moves between
    // threads; every field is owned, and the kernel only writes to the
    // heap-stable boxes above, so there is no thread affinity to lose.
    unsafe impl Send for ReadLease {}
}

#[cfg(not(windows))]
mod platform {
    use std::io;
    use std::path::Path;

    /// Unix and other platforms prove one read version with `ctime` and the
    /// opened object's identity, so holding a lease adds nothing.
    pub(crate) struct ReadLease;

    impl ReadLease {
        pub(crate) fn acquire(_path: &Path) -> io::Result<Option<Self>> {
            Ok(Some(Self))
        }

        pub(crate) fn broke(&self) -> bool {
            false
        }
    }
}

#[cfg(windows)]
pub(crate) use windows::ReadLease;

#[cfg(not(windows))]
pub(crate) use platform::ReadLease;
