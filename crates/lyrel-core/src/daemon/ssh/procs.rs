//! Process-tree sampling for native-SSH panes.
//!
//! A native-SSH pane's PTY belongs to the far machine, so the local
//! `procinfo` walk has nothing to start from and the Info panel used to show
//! no Processes section at all. This asks the far side instead: one command
//! channel per poll runs `ps` (POSIX) or `Get-CimInstance Win32_Process`
//! (PowerShell), and the returned table is walked from the shell whose parent
//! is the per-connection `sshd`.
//!
//! The root is a heuristic: SSH does not expose the session's remote PID to
//! the client. A login shell is the newest shell process under an `sshd`
//! parent, which is the right process for the pane that just connected; a
//! second concurrent session in another window is indistinguishable, so the
//! newest one wins.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use russh::ChannelMsg;

use crate::daemon::protocol::{PaneProcs, PortProbe, ProcEntry};
use crate::daemon::ssh::{SshConnection, SshManager};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_DEPTH: u8 = 16;
const MAX_PROCS: usize = 128;
/// A comment appended to every probe command. The remote shell (and the
/// PowerShell it starts) carries this marker in its own argv, which lets the
/// parser drop the probe's temporary process tree — otherwise the newest
/// shell under `sshd` would be the probe itself, not the pane's login shell.
const PROBE_MARKER: &str = "lyrel-process-probe";

/// The login shell the remote prompt integration reported decides which probe
/// to run. Unknown shells keep the full fallback; a known POSIX shell never
/// pays for two failed PowerShell starts.
fn commands_for_shell(shell: Option<&str>) -> &'static [&'static str] {
    let name = shell.map(|shell| {
        shell
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(shell)
            .trim_end_matches(".exe")
            .trim_start_matches('-')
            .to_ascii_lowercase()
    });
    match name.as_deref() {
        Some("pwsh") => &[POWERSHELL_PWSH],
        Some("powershell") => &[POWERSHELL_DESKTOP],
        Some("cmd") => &[POWERSHELL_PWSH, POWERSHELL_DESKTOP],
        Some(_) => &[POSIX_PS],
        None => &[POWERSHELL_PWSH, POWERSHELL_DESKTOP, POSIX_PS],
    }
}

/// `-NoProfile` keeps the user's profile from printing into the JSON, and
/// `-NonInteractive` stops a prompt from hanging the channel.
const POWERSHELL_PWSH: &str = "pwsh -NoProfile -NonInteractive -Command \"Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,WorkingSetSize,Name,CommandLine | ConvertTo-Json -Compress -Depth 2 # lyrel-process-probe\"";
const POWERSHELL_DESKTOP: &str = "powershell -NoProfile -NonInteractive -Command \"Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,WorkingSetSize,Name,CommandLine | ConvertTo-Json -Compress -Depth 2 # lyrel-process-probe\"";
/// `rss` is KiB on both Linux and BSD; `time` is CPU time, and `args` is the
/// command line (the first token names the process).
const POSIX_PS: &str = "ps -Ao pid=,ppid=,rss=,time=,args= # lyrel-process-probe";

pub(crate) fn snapshot(conn: &Arc<SshConnection>, shell: Option<&str>) -> Option<PaneProcs> {
    for cmd in commands_for_shell(shell) {
        let Some(out) = exec(conn, cmd) else {
            continue;
        };
        let Some(procs) = parse(&out) else {
            continue;
        };
        return Some(PaneProcs {
            procs,
            ports: Vec::new(),
            probe: PortProbe::Unavailable(
                "listening ports are not probed on a native-SSH connection".into(),
            ),
            context: None,
        });
    }
    None
}

fn exec(conn: &Arc<SshConnection>, cmd: &str) -> Option<String> {
    let runtime = SshManager::global().handle();
    let conn = conn.clone();
    let cmd = cmd.to_string();
    runtime.block_on(async move {
        let exchange = async move {
            let mut channel = conn.open_command_channel().await.ok()?;
            channel.exec(true, cmd.as_str()).await.ok()?;
            let _ = channel.eof().await;
            let mut stdout = Vec::new();
            let mut stderr_len = 0usize;
            let mut status = None;
            while let Some(msg) = channel.wait().await {
                match msg {
                    ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                    ChannelMsg::ExtendedData { data, .. } => stderr_len += data.len(),
                    ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                    _ => {}
                }
            }
            if status != Some(0) && stdout.is_empty() {
                log::debug!(
                    "remote process probe exited with {status:?} ({stderr_len} bytes of stderr)"
                );
                return None;
            }
            Some(String::from_utf8_lossy(&stdout).into_owned())
        };
        tokio::time::timeout(COMMAND_TIMEOUT, exchange).await.ok()?
    })
}

/// Parse either the PowerShell JSON or the POSIX `ps` table. `None` means the
/// command was not the one this host understands.
fn parse(out: &str) -> Option<Vec<ProcEntry>> {
    let trimmed = out.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(json) = parse_windows(trimmed) {
        return Some(json);
    }
    let posix = parse_posix(trimmed);
    (!posix.is_empty()).then_some(posix)
}

#[derive(serde::Deserialize)]
struct WinProc {
    #[serde(rename = "ProcessId", alias = "pid")]
    pid: u32,
    #[serde(rename = "ParentProcessId", alias = "ppid")]
    ppid: u32,
    #[serde(rename = "WorkingSetSize", default)]
    rss: Option<u64>,
    #[serde(rename = "Name", alias = "name", default)]
    name: String,
    #[serde(rename = "CommandLine", alias = "command_line", default)]
    command_line: Option<String>,
}

fn parse_windows(json: &str) -> Option<Vec<ProcEntry>> {
    let rows: Vec<WinProc> = match serde_json::from_str(json) {
        Ok(rows) => rows,
        Err(_) => vec![serde_json::from_str(json).ok()?],
    };
    let rows = rows
        .into_iter()
        .filter(|p| p.pid > 0)
        .filter(|p| {
            !p.command_line
                .as_deref()
                .is_some_and(|line| line.contains(PROBE_MARKER))
        })
        .map(|p| (p.pid, Row::new(p.ppid, p.name, p.rss, None)))
        .collect::<HashMap<_, _>>();
    Some(walk(&rows))
}

fn parse_posix(text: &str) -> Vec<ProcEntry> {
    let mut rows = HashMap::new();
    for line in text.lines() {
        let line = line.trim_start();
        if line.is_empty() {
            continue;
        }
        // pid, ppid, rss, time, then the command (which may contain spaces
        // and be padded by the platform's `ps`).
        let mut parts = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(rss), Some(cpu)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let args: String = {
            let rest: Vec<&str> = parts.collect();
            rest.join(" ")
        };
        if args.is_empty() {
            continue;
        }
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        let Ok(ppid) = ppid.parse::<u32>() else {
            continue;
        };
        let rss = rss.parse::<u64>().ok().map(|kib| kib.saturating_mul(1024));
        let cpu = parse_cpu_time(cpu);
        if args.contains(PROBE_MARKER) {
            continue;
        }
        // The command line's first token is the program.
        let program = args.split_whitespace().next().unwrap_or(&args);
        let name = std::path::Path::new(program)
            .file_name()
            .map_or(program, |n| n.to_str().unwrap_or(program));
        if pid > 0 {
            rows.insert(pid, Row::new(ppid, name.to_string(), rss, cpu));
        }
    }
    if rows.is_empty() {
        return Vec::new();
    }
    walk(&rows)
}

fn parse_cpu_time(text: &str) -> Option<u64> {
    // [[dd-]hh:]mm:ss
    let (days, rest) = match text.split_once('-') {
        Some((days, rest)) => (days.parse::<u64>().ok()?, rest),
        None => (0, text),
    };
    let mut parts = rest.split(':').rev();
    let seconds = parts.next()?.parse::<u64>().ok()?;
    let minutes = parts.next()?.parse::<u64>().ok()?;
    let hours = parts
        .next()
        .and_then(|h| h.parse::<u64>().ok())
        .unwrap_or(0);
    let total = days
        .saturating_mul(86_400)
        .saturating_add(hours.saturating_mul(3_600))
        .saturating_add(minutes.saturating_mul(60))
        .saturating_add(seconds);
    Some(total.saturating_mul(1_000_000_000))
}

struct Row {
    ppid: u32,
    name: String,
    rss: Option<u64>,
    cpu_ns: Option<u64>,
}

impl Row {
    fn new(ppid: u32, name: String, rss: Option<u64>, cpu_ns: Option<u64>) -> Self {
        Self {
            ppid,
            name,
            rss,
            cpu_ns,
        }
    }
}

fn walk(rows: &HashMap<u32, Row>) -> Vec<ProcEntry> {
    let Some(root) = find_root(rows) else {
        return Vec::new();
    };
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, row) in rows {
        children.entry(row.ppid).or_default().push(*pid);
    }
    for kids in children.values_mut() {
        kids.sort_unstable();
    }
    let mut out = Vec::new();
    let mut stack = vec![(root, 0u8)];
    while let Some((pid, depth)) = stack.pop() {
        let Some(row) = rows.get(&pid) else {
            continue;
        };
        if out.len() >= MAX_PROCS {
            break;
        }
        out.push(ProcEntry {
            pid,
            name: row.name.clone(),
            depth,
            foreground: false,
            rss: row.rss,
            cpu_ns: row.cpu_ns,
            started: None,
        });
        if depth >= MAX_DEPTH {
            continue;
        }
        if let Some(kids) = children.get(&pid) {
            for kid in kids.iter().rev() {
                stack.push((*kid, depth + 1));
            }
        }
    }
    out
}

/// The newest shell under a per-connection `sshd`. Falls back to the newest
/// shell process at all, so a host whose sshd process name differs still
/// yields a tree rather than an empty panel.
fn find_root(rows: &HashMap<u32, Row>) -> Option<u32> {
    let under_sshd = |row: &Row| {
        rows.get(&row.ppid)
            .is_some_and(|parent| parent.name.to_ascii_lowercase().contains("sshd"))
    };
    rows.iter()
        .filter(|(_, row)| is_shell(&row.name) && under_sshd(row))
        .map(|(pid, _)| *pid)
        .max()
        .or_else(|| {
            rows.iter()
                .filter(|(_, row)| is_shell(&row.name) && !rows.contains_key(&row.ppid))
                .map(|(pid, _)| *pid)
                .max()
        })
        .or_else(|| {
            rows.iter()
                .filter(|(_, row)| is_shell(&row.name))
                .map(|(pid, _)| *pid)
                .max()
        })
}

fn is_shell(name: &str) -> bool {
    let name = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim_end_matches(".exe")
        // A login shell's argv[0] is `-bash`/`-zsh`.
        .trim_start_matches('-')
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "nu" | "pwsh" | "powershell" | "cmd"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_table_builds_the_shell_tree_not_the_sshd_listener() {
        let table = "\
1 0 1000 00:00:01 init
100 1 2000 00:00:10 sshd
200 100 4000 00:00:02 sshd
300 200 8000 00:00:03 /usr/bin/pwsh
301 300 9000 00:00:04 omp
302 301 1000 00:00:00 node
400 200 1000 00:00:00 /bin/bash -c ps -Ao pid= # lyrel-process-probe
401 400 1000 00:00:00 ps -Ao pid= # lyrel-process-probe";
        let procs = parse_posix(table);
        assert_eq!(procs[0].pid, 300);
        assert_eq!(
            procs.iter().map(|p| (p.pid, p.depth)).collect::<Vec<_>>(),
            vec![(300, 0), (301, 1), (302, 2)]
        );
        assert_eq!(procs[0].rss, Some(8_000 * 1024));
        assert_eq!(procs[1].cpu_ns, Some(4_000_000_000));
    }

    #[test]
    fn windows_json_is_walked_from_the_sshd_child() {
        let json = r#"[
            {"ProcessId":100,"ParentProcessId":1,"WorkingSetSize":2048,"Name":"sshd.exe","CommandLine":null},
            {"ProcessId":200,"ParentProcessId":100,"WorkingSetSize":4096,"Name":"pwsh.exe","CommandLine":"pwsh -NoLogo"},
            {"ProcessId":201,"ParentProcessId":200,"WorkingSetSize":8192,"Name":"omp.exe","CommandLine":"omp"},
            {"ProcessId":400,"ParentProcessId":100,"WorkingSetSize":4096,"Name":"pwsh.exe","CommandLine":"pwsh -NoProfile -Command Get-CimInstance # lyrel-process-probe"}
        ]"#;
        let procs = parse_windows(json).unwrap();
        assert_eq!(
            procs.iter().map(|p| (p.pid, p.depth)).collect::<Vec<_>>(),
            vec![(200, 0), (201, 1)]
        );
        assert_eq!(procs[0].rss, Some(4096));
    }

    #[test]
    fn a_single_windows_record_is_accepted() {
        let json =
            r#"{"ProcessId":200,"ParentProcessId":100,"WorkingSetSize":4096,"Name":"pwsh.exe"}"#;
        let procs = parse_windows(json).unwrap();
        assert_eq!(procs.len(), 1);
        assert_eq!(procs[0].pid, 200);
    }

    #[test]
    fn cpu_time_parses_days_hours_and_minutes() {
        assert_eq!(parse_cpu_time("03:04"), Some(184_000_000_000));
        assert_eq!(parse_cpu_time("01:03:04"), Some(3_784_000_000_000));
        assert_eq!(parse_cpu_time("2-01:03:04"), Some(176_584_000_000_000));
    }
}

#[cfg(test)]
mod probe_selection_tests {
    use super::*;

    #[test]
    fn every_probe_carries_the_marker_it_filters_out() {
        for cmd in [POWERSHELL_PWSH, POWERSHELL_DESKTOP, POSIX_PS] {
            assert!(cmd.contains(PROBE_MARKER), "missing marker: {cmd}");
        }
    }

    #[test]
    fn the_reported_login_shell_avoids_a_failed_probe() {
        assert_eq!(commands_for_shell(Some("/bin/bash")), &[POSIX_PS]);
        assert_eq!(
            commands_for_shell(Some("C:\\Program Files\\PowerShell\\7\\pwsh.exe")),
            &[POWERSHELL_PWSH]
        );
        assert_eq!(
            commands_for_shell(Some("powershell.exe")),
            &[POWERSHELL_DESKTOP]
        );
        assert_eq!(
            commands_for_shell(None),
            &[POWERSHELL_PWSH, POWERSHELL_DESKTOP, POSIX_PS]
        );
    }
}
