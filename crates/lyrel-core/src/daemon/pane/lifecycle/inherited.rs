//! Unix exec 后沿用原 PTY descriptor 与 child，不重启 shell。

use super::*;
/// A pty master this process inherited from its own previous image.
///
/// `portable-pty` can only hand out a master it opened, and after an `exec`
/// there is nothing left of the one it opened except the descriptor number. The
/// operations a pane actually performs on a master are few enough — resize, ask
/// the size, clone a reader, take a writer, ask which process group is in the
/// foreground — and all of them are ioctls on that descriptor.
#[cfg(unix)]
pub(super) struct AdoptedMaster {
    fd: std::os::fd::OwnedFd,
}

#[cfg(unix)]
impl AdoptedMaster {
    pub(super) fn from_fd(fd: std::os::fd::RawFd) -> anyhow::Result<Self> {
        use std::os::fd::FromRawFd as _;

        // A descriptor that no longer refers to a terminal is one the previous
        // image did not really hold — a number reused after a close, or a blob
        // that named the wrong one. Adopting it would produce a pane whose
        // every ioctl fails in a different way; refusing produces a pane that
        // is simply gone, which is a thing the client already handles.
        if unsafe { libc::isatty(fd) } != 1 {
            anyhow::bail!("descriptor {fd} is not a terminal");
        }
        // Crossing the exec required stripping close-on-exec; being across is
        // when it goes back on. Children this daemon spawns from here — shells,
        // `lsof`, ssh transports — must not inherit another pane's master: a
        // child holding one keeps the pty open after this pane closes it, and
        // the hangup the shell should see never comes.
        if let Err(e) = crate::daemon::handoff::close_on_exec_again(fd) {
            log::warn!("could not restore close-on-exec on adopted pty {fd}: {e}");
        }
        Ok(Self {
            fd: unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) },
        })
    }

    fn dup(&self) -> std::io::Result<std::fs::File> {
        Ok(std::fs::File::from(self.fd.try_clone()?))
    }
}

#[cfg(unix)]
impl MasterPty for AdoptedMaster {
    fn resize(&self, size: PtySize) -> anyhow::Result<()> {
        use std::os::fd::AsRawFd as _;

        let ws = libc::winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: size.pixel_width,
            ws_ypixel: size.pixel_height,
        };
        if unsafe { libc::ioctl(self.fd.as_raw_fd(), libc::TIOCSWINSZ, &ws as *const _) } != 0 {
            anyhow::bail!("TIOCSWINSZ failed: {}", std::io::Error::last_os_error());
        }
        Ok(())
    }

    fn get_size(&self) -> anyhow::Result<PtySize> {
        use std::os::fd::AsRawFd as _;

        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(self.fd.as_raw_fd(), libc::TIOCGWINSZ, &mut ws as *mut _) } != 0 {
            anyhow::bail!("TIOCGWINSZ failed: {}", std::io::Error::last_os_error());
        }
        Ok(PtySize {
            rows: ws.ws_row,
            cols: ws.ws_col,
            pixel_width: ws.ws_xpixel,
            pixel_height: ws.ws_ypixel,
        })
    }

    fn try_clone_reader(&self) -> anyhow::Result<Box<dyn Read + Send>> {
        Ok(Box::new(self.dup()?))
    }

    /// Deliberately a plain `File`, unlike the writer `portable-pty` hands out:
    /// that one sends EOF to the shell when it is dropped. Correct for a pty
    /// whose pane is being closed, wrong for one whose pane is only changing
    /// which program serves it.
    fn take_writer(&self) -> anyhow::Result<Box<dyn Write + Send>> {
        Ok(Box::new(self.dup()?))
    }

    fn as_raw_fd(&self) -> Option<std::os::fd::RawFd> {
        use std::os::fd::AsRawFd as _;
        Some(self.fd.as_raw_fd())
    }

    fn tty_name(&self) -> Option<PathBuf> {
        use std::ffi::{CStr, OsStr};
        use std::os::fd::AsRawFd as _;
        use std::os::unix::ffi::OsStrExt as _;

        // Query the slave name from the inherited master without opening the
        // slave again: doing so would change this pane's hangup lifetime.
        // macOS fixes this ioctl's output at 128 bytes; Linux's /dev/pts/N
        // names fit the same stack buffer.
        let mut name = [0u8; 128];
        #[cfg(target_os = "macos")]
        let result = unsafe {
            libc::ioctl(
                self.fd.as_raw_fd(),
                libc::TIOCPTYGNAME as libc::c_ulong,
                name.as_mut_ptr(),
            )
        };
        #[cfg(not(target_os = "macos"))]
        let result =
            unsafe { libc::ptsname_r(self.fd.as_raw_fd(), name.as_mut_ptr().cast(), name.len()) };
        if result != 0 {
            return None;
        }
        let name = CStr::from_bytes_until_nul(&name).ok()?;
        Some(PathBuf::from(OsStr::from_bytes(name.to_bytes())))
    }

    fn process_group_leader(&self) -> Option<libc::pid_t> {
        use std::os::fd::AsRawFd as _;
        match unsafe { libc::tcgetpgrp(self.fd.as_raw_fd()) } {
            pid if pid > 0 => Some(pid),
            _ => None,
        }
    }
}

/// A child this process inherited from its own previous image.
///
/// Still genuinely our child — `exec` keeps the process, so the kernel's
/// parent-child bookkeeping is untouched and `waitpid` works. What was lost is
/// the `Child` value that knew how to ask.
#[cfg(unix)]
#[derive(Debug)]
pub(super) struct AdoptedChild {
    pid: libc::pid_t,
    exited: Option<portable_pty::ExitStatus>,
}

#[cfg(unix)]
impl AdoptedChild {
    pub(super) fn new(pid: u32) -> Self {
        Self {
            pid: pid as libc::pid_t,
            exited: None,
        }
    }

    fn reap(&mut self, flags: libc::c_int) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        if let Some(status) = &self.exited {
            return Ok(Some(status.clone()));
        }
        let mut raw: libc::c_int = 0;
        let seen = unsafe { libc::waitpid(self.pid, &mut raw, flags) };
        if seen == 0 {
            return Ok(None);
        }
        if seen < 0 {
            let e = std::io::Error::last_os_error();
            // ECHILD means someone already reaped it, or it was never ours.
            // Either way there is no status left to collect and no point in
            // asking again — reporting an exit of zero is what an unknown but
            // finished child amounts to.
            if e.raw_os_error() == Some(libc::ECHILD) {
                let status = portable_pty::ExitStatus::with_exit_code(0);
                self.exited = Some(status.clone());
                return Ok(Some(status));
            }
            return Err(e);
        }
        let code = if libc::WIFEXITED(raw) {
            libc::WEXITSTATUS(raw) as u32
        } else if libc::WIFSIGNALED(raw) {
            128 + libc::WTERMSIG(raw) as u32
        } else {
            0
        };
        let status = portable_pty::ExitStatus::with_exit_code(code);
        self.exited = Some(status.clone());
        Ok(Some(status))
    }
}

#[cfg(unix)]
impl portable_pty::Child for AdoptedChild {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        self.reap(libc::WNOHANG)
    }

    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        loop {
            if let Some(status) = self.reap(0)? {
                return Ok(status);
            }
        }
    }

    fn process_id(&self) -> Option<u32> {
        Some(self.pid as u32)
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct AdoptedKiller {
    pid: libc::pid_t,
}

#[cfg(unix)]
impl portable_pty::ChildKiller for AdoptedChild {
    fn kill(&mut self) -> std::io::Result<()> {
        AdoptedKiller { pid: self.pid }.kill()
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(AdoptedKiller { pid: self.pid })
    }
}

#[cfg(unix)]
impl portable_pty::ChildKiller for AdoptedKiller {
    fn kill(&mut self) -> std::io::Result<()> {
        if unsafe { libc::kill(self.pid, libc::SIGKILL) } != 0 {
            let e = std::io::Error::last_os_error();
            // Already gone is the outcome kill was asked for.
            if e.raw_os_error() != Some(libc::ESRCH) {
                return Err(e);
            }
        }
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(AdoptedKiller { pid: self.pid })
    }
}

#[cfg(test)]
mod tests;
