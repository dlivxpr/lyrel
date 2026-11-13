use super::*;
use rio_vt::crosswords::pos::{Column, Line};
use rio_vt::event::VoidListener;

const COLS: usize = 24;
const ROWS: usize = 8;

/// Feed exactly the same parser/bridge as the reader. Fully closed responses
/// must finish at every split without a timeout or synthetic close-out masking
/// a lost ESU; incoming bytes always reach the live grid immediately.
fn parse(stage: &str, chunks: &[&[u8]]) -> Vec<String> {
    let mut term = test_term(TermSize::new(COLS, ROWS));
    let mut processor = Processor::default();
    let mut keyboard = KeyboardStacks::default();
    for chunk in chunks {
        processor.advance(
            &mut ParseBridge::new(&mut term, VoidListener, &mut keyboard, None),
            chunk,
        );
    }
    assert!(
        !term.synchronized_output(),
        "{stage}: a fully fed response leaves no synchronized update armed"
    );
    let grid = &term.grid;
    let mut rows: Vec<String> = (0..grid.screen_lines() as i32)
        .map(|line| {
            (0..COLS)
                .map(|col| {
                    let cell = &grid[Line(line)][Column(col)];
                    let mut s = format!("{}{:?}", cell.c(), grid.style_of(cell).flags);
                    if let Some(extra) = cell
                        .extras_id_checked()
                        .and_then(|id| grid.extras_table.get(id))
                    {
                        s.extend(extra.zerowidth.iter());
                    }
                    s
                })
                .collect()
        })
        .collect();
    rows.push(format!("{:?}", grid.cursor.pos));
    rows
}

/// Three frames of a response streaming in, each redrawing the lines it
/// touched the way Pi's main-screen renderer does.
fn stream() -> Vec<u8> {
    let mark = "\x1b]133;B\x07\x1b]133;C\x07";
    let frames = [
        format!("\x1b[?2026h\r\x1b[2K例{mark}Ex\x1b[?2026l"),
        format!(
            "\x1b[?2026h\r\x1b[2K例子：Example sentence.\r\n\r\n\x1b[2K## 资源{mark}N\x1b[?2026l"
        ),
        format!(
            "\x1b[?2026h\x1b[2A\r\x1b[2K例子：Example.❤️\r\n\r\n\x1b[2K## 资源\r\n\
             \x1b[2K{mark}下一步：Next step.\x1b[?2026l"
        ),
    ];
    frames.concat().into_bytes()
}

#[test]
fn a_streamed_response_parses_the_same_at_every_split() {
    let bytes = stream();
    let whole = parse("whole stream", &[&bytes]);
    assert!(
        whole.iter().all(|row| !row.contains("33;")),
        "no mark reaches the grid as text"
    );
    for cut in 0..=bytes.len() {
        assert_eq!(
            parse(
                &format!("split at byte {cut}"),
                &[&bytes[..cut], &bytes[cut..]]
            ),
            whole,
            "split at byte {cut}"
        );
    }
    let bytewise: Vec<&[u8]> = bytes.chunks(1).collect();
    assert_eq!(
        parse("byte at a time", &bytewise),
        whole,
        "a byte at a time"
    );
}

/// A batch several lock-holds long, colored and multi-byte so slices end
/// mid-sequence and mid-character.
fn long_batch() -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut n = 0;
    while bytes.len() < 3 * MAX_LOCKED_FEED + 777 {
        bytes.extend_from_slice(format!("\x1b[3{}m行{n}\x1b[0m\r\n", n % 8).as_bytes());
        n += 1;
    }
    bytes
}

fn fresh() -> Crosswords<VoidListener> {
    test_term(TermSize::new(COLS, ROWS))
}

/// Where the cursor stood at each cut, and the screen after the whole batch.
type Seen = (Vec<(i32, usize)>, Vec<String>);

fn screen(term: &Crosswords<VoidListener>) -> Vec<String> {
    let grid = &term.grid;
    (0..ROWS as i32)
        .map(|line| {
            (0..COLS)
                .map(|col| grid[Line(line)][Column(col)].c())
                .collect()
        })
        .collect()
}

fn cursor(term: &Crosswords<VoidListener>) -> (i32, usize) {
    let point = term.grid.cursor.pos;
    (point.row.0, point.col.0)
}

/// The reader lets go of the grid every `MAX_LOCKED_FEED` bytes, and not
/// one of those slices may show: the screen, and the state each cut acts
/// on, come out as if the batch had been parsed in one hold — including a
/// cut that lands exactly on a slice boundary.
#[test]
fn a_batch_fed_in_slices_parses_as_one() {
    let bytes = long_batch();
    let offsets = [
        0,
        5,
        MAX_LOCKED_FEED,
        MAX_LOCKED_FEED + 1,
        2 * MAX_LOCKED_FEED + 9,
        bytes.len(),
    ];

    let whole: Seen = {
        let mut term = fresh();
        let mut processor = Processor::default();
        let mut keyboard = KeyboardStacks::default();
        let mut at = 0;
        let mut stops = Vec::new();
        for &off in &offsets {
            processor.advance(
                &mut ParseBridge::new(&mut term, VoidListener, &mut keyboard, None),
                &bytes[at..off],
            );
            at = off;
            stops.push(cursor(&term));
        }
        (stops, screen(&term))
    };

    let term = FairMutex::new(fresh());
    let mut processor = Processor::default();
    let cuts = offsets
        .iter()
        .map(|&off| (off, ReaderCut::Command(CommandMark::Finished)))
        .collect();
    let mut stops = Vec::new();
    let fed = feed_grid(
        &term,
        &mut processor,
        VoidListener,
        &mut KeyboardStacks::default(),
        &bytes,
        cuts,
        None,
        None,
        None,
        &AtomicBool::new(false),
        |term, _| stops.push(cursor(term)),
    );
    assert!(fed.is_some());
    assert_eq!((stops, screen(&term.lock())), whole);
}

/// A reader retired while it waited for the grid writes nothing into it:
/// the grid belongs to its successor now.
#[test]
fn a_retired_reader_leaves_the_grid_alone() {
    let term = FairMutex::new(fresh());
    let before = screen(&term.lock());
    let mut processor = Processor::default();
    let fed = feed_grid(
        &term,
        &mut processor,
        VoidListener,
        &mut KeyboardStacks::default(),
        b"stale",
        Vec::new(),
        None,
        None,
        None,
        &AtomicBool::new(true),
        |_, _| {},
    );
    assert!(fed.is_none());
    assert_eq!(screen(&term.lock()), before);
}

#[test]
fn unfinished_updates_parse_modes_and_unicode_at_every_byte_boundary() {
    let bytes = "\x1b[?2026h\x1b[?1h\x1b[?2004h\x1b[H宽e\u{301}".as_bytes();
    for cut in 0..=bytes.len() {
        let mut term = fresh();
        let mut processor = Processor::default();
        let mut keyboard = KeyboardStacks::default();
        for chunk in [&bytes[..cut], &bytes[cut..]] {
            processor.advance(
                &mut ParseBridge::new(&mut term, VoidListener, &mut keyboard, None),
                chunk,
            );
        }
        assert!(term.synchronized_output(), "split {cut}");
        assert!(
            term.mode()
                .contains(Mode::APP_CURSOR | Mode::BRACKETED_PASTE)
        );
        assert_eq!(term.grid[Line(0)][Column(0)].c(), '宽');
        assert_eq!(term.grid[Line(0)][Column(2)].c(), 'e');
        assert_eq!(term.grid.cursor.pos.col.0, 3);
        let start = term.sync_update_started.unwrap();
        assert_eq!(
            presentation_deadline(&term),
            Some(start + std::time::Duration::from_millis(150))
        );
        processor.advance(
            &mut ParseBridge::new(&mut term, VoidListener, &mut keyboard, None),
            b"\x1b[?2026l!",
        );
        assert!(!term.synchronized_output());
        assert_eq!(term.grid[Line(0)][Column(3)].c(), '!');
    }
}

#[test]
fn synchronized_presentation_deadline_restarts_and_lifecycle_boundaries_release_it() {
    let mut term = fresh();
    let mut processor = Processor::default();
    let mut keyboard = KeyboardStacks::default();
    processor.advance(
        &mut ParseBridge::new(&mut term, VoidListener, &mut keyboard, None),
        b"\x1b[?2026hfirst",
    );
    let first = term.sync_update_started.unwrap();
    term.sync_update_started = first.checked_sub(std::time::Duration::from_secs(2));
    processor.advance(
        &mut ParseBridge::new(&mut term, VoidListener, &mut keyboard, None),
        b"\x1b[?2026hsecond",
    );
    assert!(
        term.sync_update_started.unwrap() >= first,
        "every BSU re-arms the hold"
    );
    assert_eq!(term.grid[Line(0)][Column(5)].c(), 's');
    term.resize(TermSize::new(COLS + 1, ROWS));
    assert!(
        !term.synchronized_output(),
        "real resize releases live output"
    );
    assert!(presentation_deadline(&term).is_none());
    processor.advance(
        &mut ParseBridge::new(&mut term, VoidListener, &mut keyboard, None),
        b"\x1b[?2026hhidden\x1bcreset",
    );
    assert!(!term.synchronized_output(), "RIS clears the old hold");
    assert!(screen(&term)[0].starts_with("reset"));
}

#[test]
fn presentation_timeout_releases_at_150ms_and_accepts_later_output() {
    let mut term = fresh();
    let mut processor = Processor::default();
    let mut keyboard = KeyboardStacks::default();
    processor.advance(
        &mut ParseBridge::new(&mut term, VoidListener, &mut keyboard, None),
        b"\x1b[?2026hquiet",
    );
    let started = term.sync_update_started.unwrap();
    assert!(!expire_presentation(
        &mut term,
        started + std::time::Duration::from_millis(149)
    ));
    assert!(term.synchronized_output());
    assert!(expire_presentation(
        &mut term,
        started + std::time::Duration::from_millis(150)
    ));
    assert!(!term.synchronized_output());
    assert!(presentation_deadline(&term).is_none());
    processor.advance(
        &mut ParseBridge::new(&mut term, VoidListener, &mut keyboard, None),
        b"-after",
    );
    assert!(screen(&term)[0].starts_with("quiet-after"));
}
