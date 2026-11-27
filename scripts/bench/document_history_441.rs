//! #441：生成产品回放与 native consumer 共用的 1 MiB Markdown 和预期保存字节。

pub const STEPS: usize = 100;
pub const INPUT: &str = "x";
pub const IME_COMMIT: &str = "你好";
const SIZE: usize = 1024 * 1024;
const BOM: &[u8] = b"\xef\xbb\xbf";

#[derive(Clone, Copy)]
pub struct Site {
    pub source: usize,
    disk: usize,
}

pub struct Fixture {
    pub original: Vec<u8>,
    pub sites: [Site; STEPS],
}

impl Fixture {
    pub fn new() -> Self {
        let mut original = Vec::with_capacity(SIZE);
        original.extend_from_slice(BOM);
        original.extend_from_slice("A🎉Z\r\n".as_bytes());
        let mut cr_count = 1;
        let sites = std::array::from_fn(|step| {
            let disk = original.len();
            let source = disk - BOM.len() - cr_count;
            original.extend_from_slice(format!("a row {:03}", step + 1).as_bytes());
            if step % 2 == 0 {
                original.push(b'\r');
                cr_count += 1;
            }
            original.push(b'\n');
            Site { source, disk }
        });
        original.push(b'\n');
        let filler =
            b"The workspace records a consistent sequence of edits across a long document.\r\n\n";
        while original.len() + filler.len() < SIZE {
            original.extend_from_slice(filler);
        }
        // End with an unterminated non-blank line.
        original.resize(SIZE, b'z');
        Self { original, sites }
    }

    pub fn replacements(&self, count: usize) -> Vec<u8> {
        let mut expected = self.original.clone();
        for site in &self.sites[..count] {
            expected[site.disk] = INPUT.as_bytes()[0];
        }
        expected
    }

    pub fn typing(&self) -> Vec<u8> {
        let mut expected = self.original.clone();
        let start = self.sites[0].disk;
        expected.splice(
            start..start,
            std::iter::repeat_n(INPUT.as_bytes()[0], STEPS),
        );
        expected
    }

    pub fn ime(&self) -> Vec<u8> {
        let mut expected = self.replacements(STEPS);
        let start = BOM.len() + 1;
        expected.splice(start..start + "🎉".len(), IME_COMMIT.bytes());
        expected
    }
}

#[cfg(not(feature = "native-markdown-tests"))]
fn main() -> std::io::Result<()> {
    use std::{env, fs, io, path::PathBuf};

    let mut args = env::args_os().skip(1);
    let output = args
        .next()
        .filter(|_| args.next().is_none())
        .ok_or_else(|| io::Error::other("usage: document_history_441 <new-output-directory>"))?;
    let output = PathBuf::from(output);
    let fixture = Fixture::new();
    assert_eq!(fixture.sites[0].source, "A🎉Z\n".len());
    fs::create_dir(&output)?;
    let expected = output.join("expected");
    fs::create_dir(&expected)?;
    fs::write(output.join("replace.md"), &fixture.original)?;
    fs::write(output.join("typing.md"), &fixture.original)?;
    fs::write(expected.join("original.bin"), &fixture.original)?;
    fs::write(expected.join("replace-050.bin"), fixture.replacements(50))?;
    fs::write(
        expected.join("replace-100.bin"),
        fixture.replacements(STEPS),
    )?;
    fs::write(expected.join("typing-100.bin"), fixture.typing())?;
    fs::write(expected.join("ime-commit.bin"), fixture.ime())?;
    Ok(())
}
