//! 版本一致的分段读取：一次打开，多段拉取，任何版本漂移都可见地作废。
//!
//! [`Host::open_read`] 把读取绑定到 Host 观察到的文件版本；本模块把该会话
//! 变成消费者可用的字节流。调用方给出的预算同时是内存上界和“读多少就够”的
//! 声明：文件超过预算时在分配前拒绝（沿用整个工作台图片输入的既有限额），
//! 不会先读完再发现太大。
//!
//! [`Host::open_read`]: super::Host::open_read

use std::fmt;
use std::io;
use std::path::Path;

use super::transfer::{self, CancelToken, Cancelled, WorkingPressure, transfer_admission};
use super::{Host, Meta};

/// 与这些字节同一版本的文件元数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadOutcome {
    pub meta: Meta,
    pub bytes: Vec<u8>,
}

/// 一段读完后的进度，两个数值的单位都是字节。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadProgress {
    pub read: u64,
    pub total: u64,
}

/// 读取会话绑定的文件已经不是打开时看到的版本。
///
/// 作为 [`io::Error`] 的自定义 payload 携带，既保持现有 `io::Result` 形状，
/// 也让本地与 wire 两侧能把它和普通 `InvalidData` 区分开。
#[derive(Debug)]
pub struct FileChanged {
    message: String,
}

impl fmt::Display for FileChanged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FileChanged {}

/// An [`io::Error`] marking a versioned read as invalidated.
pub fn file_changed(message: impl Into<String>) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        FileChanged {
            message: message.into(),
        },
    )
}

/// Whether `error` says the file changed under an open read session.
pub fn is_file_changed(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::InvalidData
        && error
            .get_ref()
            .is_some_and(|inner| inner.is::<FileChanged>())
}

/// Reads `path` through one Host read session, at most `budget` bytes.
///
/// Cancellation is checked before every segment; a cancelled or failed read
/// drops the session (and its server handle) with the partially read buffer.
pub fn read_file_segmented(
    host: &dyn Host,
    path: &Path,
    budget: u64,
    cancel: &CancelToken,
    mut progress: impl FnMut(ReadProgress),
) -> io::Result<ReadOutcome> {
    cancel.check().map_err(Cancelled::into_io)?;
    let mut reader = host.open_read(path)?;
    let meta = reader.meta();
    if meta.is_dir {
        return Err(io::Error::new(
            io::ErrorKind::IsADirectory,
            format!("{} is a directory", path.display()),
        ));
    }
    if let Err(exceeded) =
        transfer::preview_budget(meta.len <= budget, "the preview input budget was exceeded")
    {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!("{}: {exceeded}", path.display()),
        ));
    }
    let capacity = usize::try_from(meta.len).map_err(|_| {
        io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!("{} does not fit this platform's address space", path.display()),
        )
    })?;
    // Account before allocating: concurrent previews share one transfer ledger,
    // and a read that cannot get its bytes back fails instead of shrinking.
    let _reservation = transfer_admission()
        .reserve(meta.len, meta.len)
        .map_err(|pressure: WorkingPressure| {
            io::Error::new(io::ErrorKind::OutOfMemory, pressure.to_string())
        })?;
    let mut bytes = Vec::with_capacity(capacity);
    while (bytes.len() as u64) < meta.len {
        cancel.check().map_err(Cancelled::into_io)?;
        let remaining = meta.len - bytes.len() as u64;
        let segment = reader.read_segment(remaining.min(transfer::SEGMENT_BYTES))?;
        if segment.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "host returned no bytes with {} of {} read from {}",
                    bytes.len(),
                    meta.len,
                    path.display()
                ),
            ));
        }
        if segment.len() as u64 > remaining {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "host returned {} bytes for the {} left in the opened version of {}",
                    segment.len(),
                    remaining,
                    path.display()
                ),
            ));
        }
        bytes.extend_from_slice(&segment);
        progress(ReadProgress {
            read: bytes.len() as u64,
            total: meta.len,
        });
    }
    // A cancellation that lands after the last segment must not publish bytes
    // nobody is waiting for.
    cancel.check().map_err(Cancelled::into_io)?;
    Ok(ReadOutcome { meta, bytes })
}
