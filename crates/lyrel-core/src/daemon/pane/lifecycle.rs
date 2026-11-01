//! 平台传输资源的生命周期；DaemonPane 仍决定何时启动与关闭会话。

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use portable_pty::{Child, MasterPty, PtySize, native_pty_system};

use super::facts::ForegroundProbes;
use super::facts::probes::{
    foreground_agent, foreground_command_running, foreground_cwd, foreground_remote_context,
    pty_foreground_pgid,
};
use super::{PaneState, WinSize};

mod exit;
#[cfg(unix)]
mod inherited;
pub(super) use exit::DeathReporter;
#[cfg(unix)]
use inherited::{AdoptedChild, AdoptedMaster};

pub(super) struct PaneBackend(Transport);

enum Transport {
    Pty(PtyBackend),
    NativeSsh(NativeSshBackend),
}

struct PtyBackend {
    /// Owns the PTY master until the pane is closed.
    ///
    /// Windows takes this value when the shell process exits. Dropping the
    /// ConPTY master calls `ClosePseudoConsole`, which closes the output side
    /// only after its pending bytes can be consumed by the dedicated reader.
    /// Keeping the slot optional lets that monitor end the stream without
    /// racing the reader for ownership of the exit notification.
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    #[cfg_attr(windows, allow(dead_code))]
    shell_pid: Option<u32>,
    integration_dir: Option<PathBuf>,
}

/// A spawned or inherited PTY, before DaemonPane starts its ordered reader.
pub(super) struct PtyParts {
    master: Box<dyn MasterPty + Send>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    shell_pid: Option<u32>,
    integration_dir: Option<PathBuf>,
    reader_handle: Box<dyn Read + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

struct NativeSshBackend {
    handle: Arc<crate::daemon::ssh::SshSessionHandle>,
    connection: crate::daemon::ssh::SharedConnection,
}

pub(super) fn join_bounded(handle: JoinHandle<()>, timeout: Duration) -> bool {
    let (tx, rx) = mpsc::channel();
    if std::thread::Builder::new()
        .name("lyrel-daemon-pane-join".to_string())
        .spawn(move || {
            let _ = handle.join();
            let _ = tx.send(());
        })
        .is_err()
    {
        return false;
    }
    rx.recv_timeout(timeout).is_ok()
}

fn pty_size(size: WinSize) -> PtySize {
    PtySize {
        rows: size.rows.max(1),
        cols: size.cols.max(1),
        pixel_width: size.cols.saturating_mul(size.cell_w),
        pixel_height: size.rows.saturating_mul(size.cell_h),
    }
}

/// Opening precedes launch preparation, as it did before extraction.
pub(super) struct PtyLaunch(portable_pty::PtyPair);

impl PtyLaunch {
    pub(super) fn open(size: WinSize) -> anyhow::Result<Self> {
        #[cfg(windows)]
        crate::daemon::conpty::ensure_prepared()?;
        #[cfg(windows)]
        crate::daemon::winproc::allow_ctrl_c_in_children();
        Ok(Self(native_pty_system().openpty(pty_size(size))?))
    }

    pub(super) fn spawn(
        self,
        id: u64,
        command: portable_pty::CommandBuilder,
        integration_dir: Option<PathBuf>,
    ) -> anyhow::Result<PtyParts> {
        let pair = self.0;
        let child = pair.slave.spawn_command(command)?;
        let shell_pid = child.process_id();
        log::info!("pane {id} child started; child_pid={shell_pid:?}");
        let child = Arc::new(Mutex::new(child));
        drop(pair.slave);
        let reader_handle = pair.master.try_clone_reader()?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
        Ok(PtyParts {
            master: pair.master,
            child,
            shell_pid,
            integration_dir,
            reader_handle,
            writer,
        })
    }
}

impl PtyParts {
    #[cfg(unix)]
    pub(super) fn adopt(carried: &mut super::Carried) -> anyhow::Result<(Self, WinSize)> {
        let master = AdoptedMaster::from_fd(carried.master_fd)?;
        let reader_handle = master.try_clone_reader()?;
        let writer = Arc::new(Mutex::new(master.take_writer()?));
        // The kernel still owns the live geometry; the handoff records ring geometry.
        let size = master
            .get_size()
            .map(|s| WinSize {
                cols: s.cols,
                rows: s.rows,
                cell_w: carried.size.cell_w,
                cell_h: carried.size.cell_h,
            })
            .unwrap_or(carried.size);
        Ok((
            Self {
                master: Box::new(master),
                child: Arc::new(Mutex::new(Box::new(AdoptedChild::new(carried.child_pid)))),
                shell_pid: Some(carried.child_pid),
                integration_dir: carried.integration_dir.take(),
                reader_handle,
                writer,
            },
            size,
        ))
    }

    pub(super) fn connect(
        self,
    ) -> (
        PaneBackend,
        Box<dyn Read + Send>,
        Arc<Mutex<Box<dyn Write + Send>>>,
    ) {
        let backend = PaneBackend(Transport::Pty(PtyBackend {
            master: Arc::new(Mutex::new(Some(self.master))),
            child: self.child,
            shell_pid: self.shell_pid,
            integration_dir: self.integration_dir,
        }));
        (backend, self.reader_handle, self.writer)
    }
}

impl PaneBackend {
    pub(super) fn native_ssh(
        handle: Arc<crate::daemon::ssh::SshSessionHandle>,
        connection: crate::daemon::ssh::SharedConnection,
    ) -> Self {
        Self(Transport::NativeSsh(NativeSshBackend {
            handle,
            connection,
        }))
    }

    pub(super) fn is_local_pty(&self) -> bool {
        matches!(self.0, Transport::Pty(_))
    }

    pub(super) fn ssh_connection(&self) -> Option<Arc<crate::daemon::ssh::SshConnection>> {
        match &self.0 {
            Transport::NativeSsh(b) => b.connection.lock().unwrap().upgrade(),
            Transport::Pty(_) => None,
        }
    }

    fn pty(&self) -> Option<&PtyBackend> {
        match &self.0 {
            Transport::Pty(pty) => Some(pty),
            Transport::NativeSsh(_) => None,
        }
    }

    pub(super) fn local_shell_pid(&self) -> Option<u32> {
        self.pty().and_then(|pty| pty.shell_pid)
    }

    #[cfg(unix)]
    pub(super) fn carry(&self) -> Option<(std::os::fd::RawFd, u32, Option<PathBuf>)> {
        let pty = self.pty()?;
        let master_fd = pty
            .master
            .lock()
            .ok()?
            .as_ref()
            .and_then(|master| master.as_raw_fd())?;
        Some((master_fd, pty.shell_pid?, pty.integration_dir.clone()))
    }

    pub(super) fn foreground_command_running(&self) -> bool {
        self.pty()
            .is_some_and(|pty| foreground_command_running(&pty.master, pty.shell_pid))
    }

    pub(super) fn foreground_cwd(&self) -> Option<PathBuf> {
        let pty = self.pty()?;
        foreground_cwd(&pty.master, pty.shell_pid)
    }

    pub(super) fn foreground_title(&self, request: &super::facts::MetadataRequest) -> String {
        self.pty()
            .map(|pty| {
                super::facts::probes::foreground_title(&pty.master, pty.shell_pid, request)
            })
            .unwrap_or_default()
    }

    pub(super) fn foreground_remote_context(&self) -> Option<super::RemoteContext> {
        self.pty()
            .and_then(|pty| foreground_remote_context(&pty.master))
    }

    pub(super) fn procs(&self) -> crate::daemon::protocol::PaneProcs {
        self.pty()
            .and_then(|pty| {
                pty.shell_pid.map(|pid| {
                    crate::daemon::procinfo::snapshot(pid, pty_foreground_pgid(&pty.master))
                })
            })
            .unwrap_or_default()
    }

    pub(super) fn reader_probes(
        &self,
    ) -> (impl Fn() -> bool + Send + 'static + use<>, ForegroundProbes) {
        let pty = self.pty().expect("local PTY reader probes");
        let shell_pid = pty.shell_pid;
        let fg_master = pty.master.clone();
        let group_master = pty.master.clone();
        let remote_master = pty.master.clone();
        let agent_master = pty.master.clone();
        let cwd_master = pty.master.clone();
        (
            move || foreground_command_running(&fg_master, shell_pid),
            ForegroundProbes {
                group: Box::new(move || {
                    #[cfg(windows)]
                    {
                        let _ = &group_master;
                        shell_pid
                    }
                    #[cfg(not(windows))]
                    {
                        pty_foreground_pgid(&group_master)
                    }
                }),
                remote: Box::new(move || foreground_remote_context(&remote_master)),
                agent: Box::new(move || foreground_agent(&agent_master)),
                cwd: Box::new(move || foreground_cwd(&cwd_master, shell_pid)),
                local_group: crate::daemon::procinfo::foreground_group_is_local,
            },
        )
    }

    pub(super) fn monitor_exit(
        &self,
        state: &Arc<Mutex<PaneState>>,
        shutting_down: &Arc<AtomicBool>,
        death: &Arc<DeathReporter>,
    ) {
        let pty = self.pty().expect("local PTY exit monitor");
        death.probe_child(pty.child.clone());
        #[cfg(windows)]
        exit::spawn_exit_monitor(
            pty.shell_pid,
            pty.master.clone(),
            state.clone(),
            shutting_down.clone(),
            death.clone(),
        );
        #[cfg(not(windows))]
        let _ = (state, shutting_down);
    }

    pub(super) fn resize(&self, size: WinSize) {
        match &self.0 {
            Transport::Pty(p) => {
                if let Ok(master) = p.master.lock() {
                    if let Some(master) = master.as_ref() {
                        let _ = master.resize(pty_size(size));
                    }
                }
            }
            Transport::NativeSsh(b) => b.handle.resize(size),
        }
    }

    pub(super) fn redraw_resizer(&self) -> Box<dyn Fn(WinSize) + Send> {
        match &self.0 {
            Transport::Pty(p) => {
                let master = p.master.clone();
                Box::new(move |size| {
                    if let Ok(master) = master.lock()
                        && let Some(master) = master.as_ref()
                    {
                        let _ = master.resize(pty_size(size));
                    }
                })
            }
            Transport::NativeSsh(b) => {
                let handle = b.handle.clone();
                Box::new(move |size| handle.resize(size))
            }
        }
    }

    pub(super) fn hangup(&self) {
        match &self.0 {
            Transport::Pty(p) => {
                #[cfg(unix)]
                signal_group(p, libc::SIGHUP);
                // Close the console, not detached services from the process ancestry.
                #[cfg(windows)]
                {
                    let master = p.master.clone();
                    std::thread::Builder::new()
                        .name("lyrel-daemon-pane-hangup".into())
                        .spawn(move || exit::close_pty_master(&master))
                        .expect("spawn daemon pane hangup thread");
                }
                if let Ok(mut child) = p.child.lock() {
                    let _ = child.kill();
                }
                #[cfg(unix)]
                signal_group(p, libc::SIGKILL);
            }
            Transport::NativeSsh(b) => b.handle.close(),
        }
    }

    pub(super) fn reap(&self, id: u64) {
        if let Some(p) = self.pty() {
            if let Ok(mut child) = p.child.lock() {
                match child.wait() {
                    Ok(status) => log::info!(
                        "pane {id} child exited; child_pid={:?} status={status}",
                        p.shell_pid
                    ),
                    Err(e) => log::warn!("pane {id} child wait failed: {e}"),
                }
            }
        }
    }

    pub(super) fn remove_integration(&mut self) {
        if let Transport::Pty(p) = &mut self.0 {
            if let Some(dir) = p.integration_dir.take() {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }
}

#[cfg(test)]
mod tests;
#[cfg(unix)]
fn signal_group(pty: &PtyBackend, sig: libc::c_int) {
    if let Some(pid) = pty.shell_pid {
        unsafe {
            libc::killpg(pid as libc::pid_t, sig);
        }
    }
    let fg = pty
        .master
        .lock()
        .ok()
        .and_then(|m| m.as_ref().and_then(|m| m.process_group_leader()));
    if let Some(fg) = fg {
        if Some(fg as u32) != pty.shell_pid {
            unsafe {
                libc::killpg(fg, sig);
            }
        }
    }
}
