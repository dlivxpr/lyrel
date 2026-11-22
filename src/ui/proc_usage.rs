//! CPU% and memory for the right panel's process views — the Activity
//! Monitor's tree since the process list left Info (#371).
//!
//! The daemon sends raw CPU time; the percentage needs two samples of the same
//! process, so it is worked out here, where the panel's poll keeps state
//! between rounds and the protocol stays stateless.

use std::collections::HashMap;
use std::time::Instant;

use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::{App, IntoElement, ParentElement as _, SharedString, Styled as _, div, px, rems};

use crate::core::proc::compact_bytes;
use crate::daemon::protocol::ProcEntry;
use crate::ui::right_panel::META_MONO;

/// Column widths, shared with the Activity Monitor's fixed header so the
/// header's sort controls sit exactly over the rows' cells.
pub(crate) const CPU_W: f32 = 40.;
pub(crate) const MEM_W: f32 = 52.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sample {
    started: Option<u64>,
    cpu_ns: u64,
}

#[derive(Default)]
pub(crate) struct CpuTracker {
    prev: HashMap<u32, Sample>,
    at: Option<Instant>,
    pct: HashMap<u32, f64>,
}

impl CpuTracker {
    pub(crate) fn sample(&mut self, procs: &[ProcEntry], now: Instant) {
        let elapsed_ns = self
            .at
            .map_or(0, |at| now.saturating_duration_since(at).as_nanos() as u64);
        let mut next = HashMap::new();
        self.pct.clear();
        for p in procs {
            let Some(cpu_ns) = p.cpu_ns else { continue };
            let cur = Sample {
                started: p.started,
                cpu_ns,
            };
            if let Some(pct) = cpu_percent(self.prev.get(&p.pid).copied(), cur, elapsed_ns) {
                self.pct.insert(p.pid, pct);
            }
            next.insert(p.pid, cur);
        }
        self.prev = next;
        self.at = Some(now);
    }

    pub(crate) fn percent(&self, pid: u32) -> Option<f64> {
        self.pct.get(&pid).copied()
    }
}

/// `ps`'s %CPU over the interval: 100 is one core busy the whole time. `None`
/// for a first sample, a pid now naming a different process, a clock that did
/// not move, or a CPU time that went backwards.
fn cpu_percent(prev: Option<Sample>, cur: Sample, elapsed_ns: u64) -> Option<f64> {
    let prev = prev?;
    if elapsed_ns == 0 || prev.started != cur.started || cur.cpu_ns < prev.cpu_ns {
        return None;
    }
    Some((cur.cpu_ns - prev.cpu_ns) as f64 * 100.0 / elapsed_ns as f64)
}

fn format_cpu(pct: f64) -> String {
    match pct < 10.0 {
        true => format!("{pct:.1}%"),
        false => format!("{pct:.0}%"),
    }
}

/// The CPU cell: blank for a daemon that sends no CPU time, a dash for a
/// process seen once so far.
fn cpu_text(reported: bool, pct: Option<f64>) -> String {
    match (reported, pct) {
        (_, Some(pct)) => format_cpu(pct),
        (true, None) => "–".into(),
        (false, None) => String::new(),
    }
}

fn cell(text: String, w: f32, mono: SharedString, cx: &App) -> impl IntoElement {
    div()
        .flex_none()
        .w(px(w))
        .flex()
        .justify_end()
        .text_size(rems(META_MONO))
        .font_family(mono)
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

/// CPU% and memory, right-aligned in fixed columns so rows line up.
pub(crate) fn row_cells(
    p: &ProcEntry,
    cpu: &CpuTracker,
    mono: SharedString,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .flex_none()
        .gap(px(6.))
        .child(cell(
            cpu_text(p.cpu_ns.is_some(), cpu.percent(p.pid)),
            CPU_W,
            mono.clone(),
            cx,
        ))
        .child(cell(
            p.rss.map(compact_bytes).unwrap_or_default(),
            MEM_W,
            mono,
            cx,
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(started: u64, cpu_ns: u64) -> Sample {
        Sample {
            started: Some(started),
            cpu_ns,
        }
    }

    #[test]
    fn cpu_percent_is_ps_style() {
        assert_eq!(
            cpu_percent(Some(s(1, 0)), s(1, 500_000_000), 1_000_000_000),
            Some(50.0)
        );
        assert_eq!(
            cpu_percent(Some(s(1, 0)), s(1, 2_000_000_000), 1_000_000_000),
            Some(200.0)
        );
    }

    #[test]
    fn no_percent_without_a_fair_comparison() {
        assert_eq!(cpu_percent(None, s(1, 10), 1_000), None, "first sample");
        assert_eq!(
            cpu_percent(Some(s(1, 0)), s(2, 10), 1_000),
            None,
            "pid reused"
        );
        assert_eq!(
            cpu_percent(Some(s(1, 0)), s(1, 10), 0),
            None,
            "zero interval"
        );
        assert_eq!(
            cpu_percent(Some(s(1, 50)), s(1, 10), 1_000),
            None,
            "went backwards"
        );
    }

    #[test]
    fn cpu_cells_read_blank_dash_or_percent() {
        assert_eq!(cpu_text(false, None), "");
        assert_eq!(cpu_text(true, None), "–");
        assert_eq!(cpu_text(true, Some(12.4)), "12%");
        assert_eq!(cpu_text(true, Some(0.34)), "0.3%");
    }
}
