//! Read network totals for the exact process from macOS's per-process monitor.
use std::process::Command;
use std::time::{Duration, Instant};

use lyrel_protocol::host::lyrel_usage::ProcessMetricUnavailable;
use lyrel_protocol::host::metrics::NetworkThroughput;

#[derive(Default)]
pub(super) struct Sampler {
    previous: Option<(Instant, u64, u64)>,
}

impl Sampler {
    pub(super) fn sample(
        &mut self,
        pid: u32,
    ) -> Option<Result<NetworkThroughput, ProcessMetricUnavailable>> {
        let mut command = Command::new("/usr/bin/nettop");
        command
            .args(["-n", "-P", "-L", "1", "-J", "bytes_in,bytes_out", "-p"])
            .arg(pid.to_string())
            .env("LC_ALL", "C");
        let counters =
            lyrel_protocol::core::proc::output_within(&mut command, Duration::from_secs(2))
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| read_totals(&output.stdout, pid));
        let Some((received, sent)) = counters else {
            self.previous = None;
            return Some(Err(ProcessMetricUnavailable::CounterReadFailed));
        };
        let now = Instant::now();
        let (then, previous_rx, previous_tx) = self.previous.replace((now, received, sent))?;
        let elapsed = now.duration_since(then).as_secs_f64();
        if elapsed == 0. {
            return None;
        }
        Some(Ok(NetworkThroughput::new(
            received.checked_sub(previous_rx)? as f64 / elapsed,
            sent.checked_sub(previous_tx)? as f64 / elapsed,
        )))
    }
}

fn read_totals(output: &[u8], pid: u32) -> Option<(u64, u64)> {
    let text = std::str::from_utf8(output).ok()?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let mut columns = lines.next()?.split(',');
    columns.next()?;
    if columns.next()?.trim() != "bytes_in" || columns.next()?.trim() != "bytes_out" {
        return None;
    }
    // Parse numeric columns from the end: the process name can contain commas.
    let mut row = lines
        .next()?
        .trim_end()
        .trim_end_matches(',')
        .rsplitn(3, ',');
    let sent = row.next()?.trim().parse().ok()?;
    let received = row.next()?.trim().parse().ok()?;
    let observed_pid = row.next()?.trim_matches(['"', ' ']).rsplit_once('.')?.1;
    (lines.next().is_none() && observed_pid.parse::<u32>().ok()? == pid).then_some((received, sent))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::net::{Ipv4Addr, TcpListener, TcpStream};

    #[test]
    fn nettop_attributes_loopback_traffic_to_the_current_process() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut sender = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut receiver, _) = listener.accept().unwrap();
        let timeout = Some(Duration::from_secs(2));
        sender.set_write_timeout(timeout).unwrap();
        receiver.set_read_timeout(timeout).unwrap();

        let payload = [0x5a; 8192];
        let mut received = [0; 8192];
        let mut exchange = || {
            sender.write_all(&payload).unwrap();
            receiver.read_exact(&mut received).unwrap();
            assert_eq!(received, payload);
        };

        // Keep the same sockets open across both samples so their cumulative
        // counters cannot disappear when the traffic has finished.
        exchange();
        let mut sampler = Sampler::default();
        let pid = std::process::id();
        assert_eq!(
            sampler.sample(pid),
            None,
            "nettop must establish a baseline"
        );
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            exchange();
            let reading = sampler.sample(pid);
            if reading.is_some_and(|result| {
                result
                    .is_ok_and(|rate| rate.rx_bytes_per_sec() > 0. && rate.tx_bytes_per_sec() > 0.)
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "nettop did not attribute the loopback transfer to PID {pid}: {reading:?}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
