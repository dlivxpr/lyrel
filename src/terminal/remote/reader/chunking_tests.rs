use super::*;
use rio_vt::crosswords::pos::{Column, Line};
use rio_vt::event::VoidListener;

const COLS: usize = 24;
const ROWS: usize = 8;

/// Feeds `chunks` through the same `Processor` the reader runs, then
/// closes the way the reader closes a fully fed stream: if Rio is still
/// holding a synchronized update, the consumer ends it (the replay path
/// does this after every snapshot, the live loop at the armed deadline).
/// Rio arms the timeout mid-`advance` when the parser completes a BSU,
/// while buffering only engages at the next `advance` — so a cut inside
/// a frame's closing `\x1b[?2026l` parks the escape's head in the parser
/// and its tail in the sync buffer, whose scan can no longer match it.
/// That update ends at the consumer's close-out, not with a lost ESU:
/// the tail replays through the same parser and draws nothing.
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
    if processor.sync_timeout().sync_timeout().is_some() {
        processor.stop_sync(&mut ParseBridge::new(
            &mut term,
            VoidListener,
            &mut keyboard,
            None,
        ));
    }
    assert!(
        processor.sync_timeout().sync_timeout().is_none(),
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
        &AtomicBool::new(true),
        |_, _| {},
    );
    assert!(fed.is_none());
    assert_eq!(screen(&term.lock()), before);
}
