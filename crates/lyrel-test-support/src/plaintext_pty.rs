//! Deterministic real-PTY producer used by the terminal echo consumer and bench.
use std::fs::File;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::time::{Duration, Instant};

const LINES: usize = 9_600;
const LINE: usize = 84;
const BATCH: usize = 16;
const PAYLOAD: &[u8; 71] =
    b"abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz012345678";

fn numbered_line(number: usize) -> [u8; LINE] {
    let mut line = [b' '; LINE];
    line[..6].copy_from_slice(b"PLAIN ");
    for (i, power) in [1000, 100, 10, 1].into_iter().enumerate() {
        line[6 + i] = b'0' + (number / power % 10) as u8;
    }
    line[11..82].copy_from_slice(PAYLOAD);
    line[82..].copy_from_slice(b"\r\n");
    line
}

fn echo_line(mut sample: usize, key: u8) -> [u8; 10] {
    let mut line = *b">aaaa>a>\r\n";
    for digit in line[1..5].iter_mut().rev() {
        *digit = b'a' + (sample % 4) as u8;
        sample /= 4;
    }
    line[6] = key;
    line
}

fn send(bytes: &[u8]) -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(bytes)?;
    out.flush()
}

fn produce(dir: &Path, rate: u32) -> io::Result<()> {
    let trigger = dir.join("start");
    while !trigger.try_exists()? {
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut report = File::create_new(dir.join("producer.csv"))?;
    writeln!(report, "batch,lines,scheduled_ms,begin_ms,flushed_ms")?;
    let start = Instant::now();
    let mut bytes = [0; LINE * BATCH];
    for first in (1..=LINES).step_by(BATCH) {
        for (offset, line) in bytes.chunks_exact_mut(LINE).enumerate() {
            line.copy_from_slice(&numbered_line(first + offset));
        }
        let scheduled = Duration::from_secs_f64((first - 1) as f64 / f64::from(rate));
        std::thread::sleep((start + scheduled).saturating_duration_since(Instant::now()));
        let begin = start.elapsed();
        send(&bytes)?;
        writeln!(
            report,
            "{},{},{:.3},{:.3},{:.3}",
            first.div_ceil(BATCH),
            BATCH,
            scheduled.as_secs_f64() * 1000.,
            begin.as_secs_f64() * 1000.,
            start.elapsed().as_secs_f64() * 1000.,
        )?;
    }
    report.sync_all()?;
    send(b"PLAIN_DONE\r\n")
}

/// A pane-local child. The caller needs an empty, owned directory per cycle.
pub fn run(dir: &Path, rate: u32) -> io::Result<()> {
    let pane = std::env::var("LYREL_PANE").map_err(io::Error::other)?;
    if !(16..=9600).contains(&rate) || !dir.is_dir() {
        return Err(io::Error::other(
            "plaintext needs an owned fixture directory and rate 16..=9600",
        ));
    }
    writeln!(File::create_new(dir.join("pane.txt"))?, "{pane}")?;
    writeln!(
        File::create_new(dir.join("pid.txt"))?,
        "{}",
        std::process::id()
    )?;
    crate::pty::raw_pty();
    send(b"\x1b[?25l>>>>>>>>\r\naaaaaaaa\r\nbbbbbbbb\r\ncccccccc\r\ndddddddd\r\nPLAIN_READY\r\n")?;
    let dir = dir.to_owned();
    std::thread::spawn(move || {
        if let Err(error) = produce(&dir, rate) {
            eprintln!("plaintext producer: {error}");
            std::process::exit(1);
        }
    });
    let mut sample = 0;
    let mut buffered = [0; 4];
    let mut used = 0;
    let mut input = io::stdin().lock();
    loop {
        let bytes = input.fill_buf()?;
        if bytes.is_empty() {
            return Ok(());
        }
        for &byte in bytes {
            buffered[used] = byte;
            used += 1;
            match std::str::from_utf8(&buffered[..used]) {
                Ok(text) => {
                    let character = text.chars().next().expect("one UTF-8 scalar");
                    match character {
                        'a'..='d' if sample < 255 => {
                            sample += 1;
                            send(&echo_line(sample, byte))?;
                        }
                        ch if !ch.is_ascii() => send(format!("IME {ch}\r\n").as_bytes())?,
                        ch => send(format!("UNEXPECTED U+{:04X}\r\n", u32::from(ch)).as_bytes())?,
                    }
                    used = 0;
                }
                Err(error) if error.error_len().is_none() => {}
                Err(error) => return Err(io::Error::new(io::ErrorKind::InvalidData, error)),
            }
        }
        let count = bytes.len();
        input.consume(count);
    }
}

/// Check the terminal/CLI consumer's retained text (CRLF and blank rows are normalized).
pub fn verify_output(output: &str, loaded: bool, keys: &str, commit: &str) -> io::Result<()> {
    let mut numbered = 0;
    let mut echoed = 0;
    let mut committed = String::new();
    let mut ready = 0;
    let mut done = 0;
    for line in output.lines().map(str::trim_end) {
        if line.starts_with("PLAIN ") {
            numbered += 1;
            if numbered > LINES
                || done != 0
                || line.as_bytes() != &numbered_line(numbered)[..LINE - 2]
            {
                return Err(io::Error::other(format!(
                    "incorrect numbered output at line {numbered}: {line:?}"
                )));
            }
        } else if line == "PLAIN_READY" {
            ready += 1;
        } else if line == "PLAIN_DONE" {
            done += 1;
            if numbered != LINES {
                return Err(io::Error::other(format!(
                    "early completion after {numbered} lines"
                )));
            }
        } else if line.starts_with('>') && line != ">>>>>>>>" {
            let expected = keys
                .as_bytes()
                .get(echoed)
                .map(|&key| echo_line(echoed + 1, key));
            if expected.as_ref().map(|row| &row[..8]) != Some(line.as_bytes()) {
                return Err(io::Error::other(format!(
                    "incorrect echo at sample {}: {line:?}",
                    echoed + 1
                )));
            }
            echoed += 1;
        } else if let Some(text) = line.strip_prefix("IME ") {
            committed.push_str(text);
        } else if !matches!(
            line,
            "" | ">>>>>>>>" | "aaaaaaaa" | "bbbbbbbb" | "cccccccc" | "dddddddd"
        ) {
            return Err(io::Error::other(format!(
                "unexpected PTY transcript row: {line:?}"
            )));
        }
    }
    if ready != 1
        || numbered != if loaded { LINES } else { 0 }
        || done != usize::from(loaded)
        || echoed != keys.len()
        || committed != commit
    {
        return Err(io::Error::other(format!(
            "incomplete transcript: ready={ready}, lines={numbered}, done={done}, echo={echoed}, IME={committed:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_and_echo_certificates_reject_loss_reordering_and_duplicates() {
        assert_eq!(&numbered_line(9600)[..11], b"PLAIN 9600 ");
        assert_eq!(&echo_line(61, b'c'), b">addb>c>\r\n");
        let mut transcript = String::from("PLAIN_READY\r\n");
        let first = transcript.len();
        for line in 1..=9600 {
            transcript.push_str(std::str::from_utf8(&numbered_line(line)).unwrap());
        }
        assert_eq!(transcript.len() - first, 806_400);
        transcript.push_str("PLAIN_DONE\r\n>aaab>a>\r\n>aaac>b>\r\nIME 中\r\nIME 文\r\n");
        assert!(verify_output(&transcript, true, "ab", "中文").is_ok());
        assert!(
            verify_output(
                &transcript.replacen("0002 ", "0003 ", 1),
                true,
                "ab",
                "中文"
            )
            .is_err()
        );
        assert!(verify_output(&transcript.replace("PLAIN_DONE", ""), true, "ab", "中文").is_err());
        assert!(verify_output(&(transcript + ">aaac>b>\r\n"), true, "ab", "中文").is_err());
    }
}
