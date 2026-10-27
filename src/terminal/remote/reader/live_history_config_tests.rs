use super::*;
use rio_vt::crosswords::pos::{Column, Line, Pos};

fn wait_for(term: &RemoteTerminal, ready: impl Fn(&Crosswords<EventProxy>) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if ready(&term.term.lock()) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "reader did not reach the expected grid state"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn history_limit_changed_in_alternate_screen_applies_before_following_output() {
    crate::core::config::pin_test_config_dir();
    let (client, mut daemon) = crate::terminal::view::test_stream_pair();
    let term = RemoteTerminal::from_stream(client, TermSize::new(40, 3)).unwrap();
    let mut config = crate::core::config::Config::default();
    config.scrollback_limit = 2;
    term.apply_user_config(&config);
    for (limit, prefix) in [(10, "GROW"), (2, "SHRINK")] {
        DaemonMsg::Output(b"\x1b[?1049h".to_vec())
            .encode(&mut daemon)
            .unwrap();
        wait_for(&term, |grid| grid.mode().contains(Mode::ALT_SCREEN));
        config.scrollback_limit = limit;
        term.apply_user_config(&config);
        let mut output = b"\x1b[?1049l".to_vec();
        for row in 1..=20 {
            output.extend_from_slice(format!("{prefix}_{row:02}\r\n").as_bytes());
        }
        DaemonMsg::Output(output).encode(&mut daemon).unwrap();
        wait_for(&term, |grid| {
            !grid.mode().contains(Mode::ALT_SCREEN) && grid.grid.history_size() == limit
        });
        let grid = term.term.lock();
        let text = grid.bounds_to_string(
            Pos::new(Line(-(grid.grid.history_size() as i32)), Column(0)),
            Pos::new(Line(2), Column(39)),
        );
        assert!(text.contains(&format!("{prefix}_20")), "{text:?}");
        let first_kept = 19 - limit;
        assert!(
            text.contains(&format!("{prefix}_{first_kept:02}")),
            "{text:?}"
        );
        assert!(
            !text.contains(&format!("{prefix}_{:02}", first_kept - 1)),
            "{text:?}"
        );
    }
}
