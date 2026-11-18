use std::io::{self, Read};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const REAP_POLL: Duration = Duration::from_millis(25);

pub fn hide_console(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

pub fn output_within(cmd: &mut Command, timeout: Duration) -> io::Result<Output> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let out = std::thread::spawn({
        let pipe = child.stdout.take();
        move || drain(pipe)
    });
    let err = std::thread::spawn({
        let pipe = child.stderr.take();
        move || drain(pipe)
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = out.join();
            let _ = err.join();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "{} did not finish within {timeout:?}",
                    cmd.get_program().to_string_lossy()
                ),
            ));
        }
        std::thread::sleep(REAP_POLL);
    };

    Ok(Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut buf);
    }
    buf
}

/// `512 KB`, `38 MB`, `1.2 GB`: binary units under the short names `ps` and
/// Activity Monitor print.
pub fn compact_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64 / 1024.0;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    match v < 10.0 {
        true => format!("{v:.1} {}", UNITS[unit]),
        false => format!("{v:.0} {}", UNITS[unit]),
    }
}

#[cfg(test)]
mod formatting_tests {
    use super::compact_bytes;

    #[test]
    fn bytes_read_compactly() {
        assert_eq!(compact_bytes(512), "512 B");
        assert_eq!(compact_bytes(512 * 1024), "512 KB");
        assert_eq!(compact_bytes(38 * 1024 * 1024), "38 MB");
        assert_eq!(compact_bytes(1_288_490_189), "1.2 GB");
        assert_eq!(compact_bytes(5 * 1024 * 1024 + 300 * 1024), "5.3 MB");
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_command_that_finishes_hands_back_its_output() {
        let mut cmd = Command::new("echo");
        cmd.arg("hello");
        let out = output_within(&mut cmd, Duration::from_secs(10)).expect("echo runs");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello");
    }

    #[test]
    fn a_command_that_hangs_is_killed_and_reported_as_timed_out() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let started = Instant::now();
        let err = output_within(&mut cmd, Duration::from_millis(200))
            .expect_err("sleep 30 must not finish");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the call should return as soon as the deadline passes, took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn output_larger_than_a_pipe_buffer_does_not_deadlock() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "head -c 400000 /dev/zero | tr '\\0' 'x'"]);
        let out = output_within(&mut cmd, Duration::from_secs(20)).expect("sh runs");
        assert_eq!(out.stdout.len(), 400_000);
    }
}
