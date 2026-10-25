//! The parse-time seam between `rio-vt`'s `Processor` and a pane's grid.
//!
//! `Processor::advance` drives a [`Handler`]; `Crosswords` implements it, but
//! a handful of its answers are Rio-the-product's rather than this terminal's,
//! and this struct is where that is corrected. Everything else delegates
//! untouched, one method per callback, so a behavior difference cannot hide
//! behind a blanket default.
//!
//! What is intercepted, and why:
//!
//! - `identify_terminal`: Rio's own DA1 answer advertises sixel and OSC 52
//!   (`CSI ? 62;4;6;22;52 c`). This terminal's answer stays `CSI ? 6 c`, the
//!   same bytes the daemon writes for a detached pane
//!   (`lyrel_core::daemon::pane::DA1_REPLY`) and the previous emulator sent:
//!   graphics protocols are owned out-of-band by the daemon's sniffer, and
//!   the query's waiter (Claude Code's theme probe ends its batch on DA1)
//!   must not read capabilities nothing here renders. DA2 keeps the exact
//!   bytes the pane has always answered rather than Rio's version number.
//! - `report_version` (XTVERSION): previously met with silence, and a pane
//!   does not start answering "rio 0.5.28" because the parser changed.
//! - `cells_size_pixels` / `text_area_size_pixels`: geometry is answered once
//!   by the daemon using the controller's device-pixel resize metrics, including
//!   when no GUI is attached. A local mirror must not send a second reply.
//! - `graphics_attribute`: the reply would describe sixel geometry this
//!   terminal does not render; previously silence.
//! - `xtgettcap_response`, `kitty_graphics_response`,
//!   `glyph_protocol_response`: the reply channels for XTGETTCAP, kitty
//!   graphics and the Glyph Protocol. Kitty transmissions never reach here —
//!   the daemon lifts them out of the stream — and the other two were
//!   previously dropped unacknowledged; none of the three is a protocol this
//!   product advertises, so the parse completes and the reply is not sent.
//! - `set_mouse_cursor_icon` (left at the trait default): OSC 22 pointer
//!   shapes were never surfaced to the painter and still are not.
//!
//! Kitty keyboard operations use the application's shared protocol state,
//! also used by daemon replay. Rio's internal bounded stack and `set` behavior
//! differ, so only the effective flags are applied through its public Handler.
//! This keeps push/pop, screen swaps and mode queries on one contract without
//! changing the emulator's private stack or copying its implementation.
//!
//! Sixel and iTerm2 payloads delegate like everything else: with zero cell
//! dimensions `Crosswords::insert_graphic` returns before touching a cell or
//! the cursor, so the bytes are consumed by the real parser — not swallowed
//! mid-sequence — and the grid stays text-only.

use lyrel_core::core::term_modes::KeyboardStacks;
use rio_vt::ansi::mode::{Mode, PrivateMode};
use rio_vt::ansi::{
    ClearMode, CursorShape, KeyboardModes, KeyboardModesApplyBehavior, LineClearMode,
    TabulationClearMode, sixel,
};
use rio_vt::config::colors::ColorRgb;
use rio_vt::crosswords::Crosswords;
use rio_vt::crosswords::attr::Attr;
use rio_vt::crosswords::grid::row::SemanticPrompt;
use rio_vt::crosswords::pos::{CharsetIndex, Column, Line, StandardCharset};
use rio_vt::crosswords::square::Hyperlink;
use rio_vt::event::{EventListener, RioEvent};
use rio_vt::performer::handler::{Handler, ScpCharPath, ScpUpdateMode};
use rio_vt::performer::parser::Params;

/// The secondary device attributes this pane has always answered (`CSI > c`)
/// — the previous emulator's own version number, kept byte-for-byte so a
/// probe's fingerprint of the pane does not change with the parser.
const DA2_REPLY: &str = "\x1b[>0;2601;1c";

/// Borrowed per parse batch; nothing here outlives an `advance`.
pub(super) struct ParseBridge<'a, T: EventListener> {
    inner: &'a mut Crosswords<T>,
    /// Where device-attribute replies go: the same listener the inner grid
    /// emits through, cloned out because `Crosswords` keeps its own private.
    proxy: T,
    keyboard: &'a mut KeyboardStacks,
    history_limit: Option<usize>,
}

impl<'a, T: EventListener> ParseBridge<'a, T> {
    pub(super) fn new(
        inner: &'a mut Crosswords<T>,
        proxy: T,
        keyboard: &'a mut KeyboardStacks,
        history_limit: Option<usize>,
    ) -> Self {
        Self {
            inner,
            proxy,
            keyboard,
            history_limit,
        }
    }

    fn reply(&self, text: impl Into<String>) {
        self.proxy.send_event(
            RioEvent::PtyWrite(self.inner.route_id, text.into()),
            self.inner.window_id,
        );
    }

    fn sync_keyboard(&mut self) {
        Handler::set_keyboard_mode(
            self.inner,
            KeyboardModes::from_bits_truncate(self.keyboard.active()),
            KeyboardModesApplyBehavior::Replace,
        );
    }

    fn sync_screen(&mut self, was_alt: bool, mode: PrivateMode) {
        let is_alt = self
            .inner
            .mode()
            .contains(rio_vt::crosswords::Mode::ALT_SCREEN);
        if was_alt == is_alt {
            return;
        }
        if mode.raw() == 1049 {
            self.keyboard.swap();
            self.sync_keyboard();
        }
        if !is_alt && let Some(limit) = self.history_limit {
            self.inner.grid.update_history(limit);
        }
    }
}

impl<T: EventListener> Handler for ParseBridge<'_, T> {
    fn set_title(&mut self, title: Option<String>) {
        self.inner.set_title(title);
    }

    fn set_current_directory(&mut self, path: std::path::PathBuf) {
        self.inner.set_current_directory(path);
    }

    fn set_semantic_prompt(&mut self, prompt: SemanticPrompt) {
        self.inner.set_semantic_prompt(prompt);
    }

    fn set_user_var(&mut self, name: String, value: String) {
        self.inner.set_user_var(name, value);
    }

    fn set_cursor_style(&mut self, style: Option<CursorShape>, blinking: bool) {
        self.inner.set_cursor_style(style, blinking);
    }

    fn set_cursor_shape(&mut self, shape: CursorShape) {
        self.inner.set_cursor_shape(shape);
    }

    fn input(&mut self, c: char) {
        self.inner.input(c);
    }

    fn input_str(&mut self, s: &str) {
        self.inner.input_str(s);
    }

    fn input_ascii_str(&mut self, s: &str) {
        self.inner.input_ascii_str(s);
    }

    fn input_codepoints(&mut self, codepoints: &[u32]) {
        self.inner.input_codepoints(codepoints);
    }

    fn goto(&mut self, line: Line, col: Column) {
        self.inner.goto(line, col);
    }

    fn goto_line(&mut self, line: Line) {
        self.inner.goto_line(line);
    }

    fn goto_col(&mut self, col: Column) {
        self.inner.goto_col(col);
    }

    fn insert_blank(&mut self, count: usize) {
        self.inner.insert_blank(count);
    }

    fn move_up(&mut self, rows: usize) {
        self.inner.move_up(rows);
    }

    fn move_down(&mut self, rows: usize) {
        self.inner.move_down(rows);
    }

    fn identify_terminal(&mut self, intermediate: Option<char>) {
        match intermediate {
            None => self.reply("\x1b[?6c"),
            Some('>') => self.reply(DA2_REPLY),
            other => self.inner.identify_terminal(other),
        }
    }

    fn device_status(&mut self, arg: usize) {
        self.inner.device_status(arg);
    }

    fn move_forward(&mut self, cols: Column) {
        self.inner.move_forward(cols);
    }

    fn move_backward(&mut self, cols: Column) {
        self.inner.move_backward(cols);
    }

    fn move_down_and_cr(&mut self, rows: usize) {
        self.inner.move_down_and_cr(rows);
    }

    fn move_up_and_cr(&mut self, rows: usize) {
        self.inner.move_up_and_cr(rows);
    }

    fn put_tab(&mut self, count: u16) {
        self.inner.put_tab(count);
    }

    fn backspace(&mut self) {
        self.inner.backspace();
    }

    fn carriage_return(&mut self) {
        self.inner.carriage_return();
    }

    fn linefeed(&mut self) {
        self.inner.linefeed();
    }

    fn bell(&mut self) {
        self.inner.bell();
    }

    /// The pane's desktop notifications are owned by the daemon-side OSC
    /// sniffer; the emulator still finishes the parse (and its event is
    /// dropped by the pane's proxy) so a notification cannot corrupt what
    /// follows it.
    fn desktop_notification(&mut self, title: String, body: String) {
        self.inner.desktop_notification(title, body);
    }

    fn substitute(&mut self) {
        self.inner.substitute();
    }

    fn newline(&mut self) {
        self.inner.newline();
    }

    fn set_horizontal_tabstop(&mut self) {
        self.inner.set_horizontal_tabstop();
    }

    fn scroll_up(&mut self, rows: usize) {
        self.inner.scroll_up(rows);
    }

    fn scroll_down(&mut self, rows: usize) {
        self.inner.scroll_down(rows);
    }

    fn insert_blank_lines(&mut self, count: usize) {
        self.inner.insert_blank_lines(count);
    }

    fn delete_lines(&mut self, count: usize) {
        self.inner.delete_lines(count);
    }

    fn erase_chars(&mut self, count: Column) {
        self.inner.erase_chars(count);
    }

    fn delete_chars(&mut self, count: usize) {
        self.inner.delete_chars(count);
    }

    fn move_backward_tabs(&mut self, count: u16) {
        self.inner.move_backward_tabs(count);
    }

    fn move_forward_tabs(&mut self, count: u16) {
        self.inner.move_forward_tabs(count);
    }

    fn save_cursor_position(&mut self) {
        self.inner.save_cursor_position();
    }

    fn restore_cursor_position(&mut self) {
        self.inner.restore_cursor_position();
    }

    fn clear_line(&mut self, mode: LineClearMode) {
        self.inner.clear_line(mode);
    }

    fn clear_screen(&mut self, mode: ClearMode) {
        self.inner.clear_screen(mode);
    }

    fn set_tabs(&mut self, interval: u16) {
        self.inner.set_tabs(interval);
    }

    fn clear_tabs(&mut self, mode: TabulationClearMode) {
        self.inner.clear_tabs(mode);
    }

    fn reset_state(&mut self) {
        *self.keyboard = KeyboardStacks::default();
        self.inner.reset_state();
        if let Some(limit) = self.history_limit {
            self.inner.grid.update_history(limit);
        }
    }

    fn reverse_index(&mut self) {
        self.inner.reverse_index();
    }

    fn terminal_attribute(&mut self, attr: Attr) {
        self.inner.terminal_attribute(attr);
    }

    fn set_mode(&mut self, mode: Mode) {
        self.inner.set_mode(mode);
    }

    fn unset_mode(&mut self, mode: Mode) {
        self.inner.unset_mode(mode);
    }

    fn report_mode(&mut self, mode: Mode) {
        self.inner.report_mode(mode);
    }

    fn set_private_mode(&mut self, mode: PrivateMode) {
        let was_alt = self
            .inner
            .mode()
            .contains(rio_vt::crosswords::Mode::ALT_SCREEN);
        self.inner.set_private_mode(mode);
        self.sync_screen(was_alt, mode);
    }

    fn unset_private_mode(&mut self, mode: PrivateMode) {
        let was_alt = self
            .inner
            .mode()
            .contains(rio_vt::crosswords::Mode::ALT_SCREEN);
        self.inner.unset_private_mode(mode);
        self.sync_screen(was_alt, mode);
    }

    fn report_private_mode(&mut self, mode: PrivateMode) {
        self.inner.report_private_mode(mode);
    }

    /// XTVERSION stays unanswered: the pane never disclosed a version and
    /// does not start answering with Rio's now.
    fn report_version(&mut self) {}

    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        self.inner.set_scrolling_region(top, bottom);
    }

    fn set_keypad_application_mode(&mut self) {
        self.inner.set_keypad_application_mode();
    }

    fn unset_keypad_application_mode(&mut self) {
        self.inner.unset_keypad_application_mode();
    }

    fn set_active_charset(&mut self, index: CharsetIndex) {
        self.inner.set_active_charset(index);
    }

    fn configure_charset(&mut self, index: CharsetIndex, charset: StandardCharset) {
        self.inner.configure_charset(index, charset);
    }

    fn set_color(&mut self, index: usize, color: ColorRgb) {
        self.inner.set_color(index, color);
    }

    fn dynamic_color_sequence(&mut self, sequence: String, index: usize, terminator: &str) {
        self.inner
            .dynamic_color_sequence(sequence, index, terminator);
    }

    fn reset_color(&mut self, index: usize) {
        self.inner.reset_color(index);
    }

    fn clipboard_store(&mut self, clipboard: u8, data: &[u8]) {
        self.inner.clipboard_store(clipboard, data);
    }

    fn clipboard_load(&mut self, clipboard: u8, terminator: &str) {
        self.inner.clipboard_load(clipboard, terminator);
    }

    fn decaln(&mut self) {
        self.inner.decaln();
    }

    fn push_title(&mut self) {
        self.inner.push_title();
    }

    fn pop_title(&mut self) {
        self.inner.pop_title();
    }

    fn text_area_size_pixels(&mut self) {}

    fn cells_size_pixels(&mut self) {}

    fn text_area_size_chars(&mut self) {
        self.inner.text_area_size_chars();
    }

    /// The reply would quote sixel geometry nothing here renders.
    fn graphics_attribute(&mut self, _: u16, _: u16) {}

    fn sixel_graphic_start(&mut self, params: &Params) {
        self.inner.sixel_graphic_start(params);
    }

    fn is_sixel_graphic_active(&self) -> bool {
        self.inner.is_sixel_graphic_active()
    }

    fn sixel_graphic_put(&mut self, byte: u8) -> Result<(), sixel::Error> {
        self.inner.sixel_graphic_put(byte)
    }

    fn sixel_graphic_reset(&mut self) {
        self.inner.sixel_graphic_reset();
    }

    fn sixel_graphic_finish(&mut self) {
        self.inner.sixel_graphic_finish();
    }

    fn insert_graphic(
        &mut self,
        data: rio_graphics::GraphicData,
        palette: Option<Vec<ColorRgb>>,
        cursor_movement: Option<u8>,
    ) {
        self.inner.insert_graphic(data, palette, cursor_movement);
    }

    fn store_graphic(&mut self, data: rio_graphics::GraphicData) {
        self.inner.store_graphic(data);
    }

    fn kitty_transmit_and_display(
        &mut self,
        data: rio_graphics::GraphicData,
        placement: rio_vt::ansi::kitty_graphics_protocol::PlacementRequest,
    ) {
        self.inner.kitty_transmit_and_display(data, placement);
    }

    fn place_graphic(
        &mut self,
        placement: rio_vt::ansi::kitty_graphics_protocol::PlacementRequest,
    ) -> bool {
        self.inner.place_graphic(placement)
    }

    fn delete_graphics(&mut self, delete: rio_vt::ansi::kitty_graphics_protocol::DeleteRequest) {
        self.inner.delete_graphics(delete);
    }

    fn set_hyperlink(&mut self, hyperlink: Option<Hyperlink>) {
        self.inner.set_hyperlink(hyperlink);
    }

    // `set_mouse_cursor_icon` (OSC 22 pointer shapes) is deliberately left at
    // the trait's default no-op: the painter never surfaced it, and skipping
    // the override keeps the `cursor_icon` crate out of the dependency set —
    // the parse completes and the icon goes nowhere, exactly as before.

    fn set_progress_report(&mut self, report: rio_vt::event::ProgressReport) {
        self.inner.set_progress_report(report);
    }

    fn report_keyboard_mode(&mut self) {
        self.reply(format!("\x1b[?{}u", self.keyboard.active()));
    }

    fn push_keyboard_mode(&mut self, mode: KeyboardModes) {
        self.keyboard.push(mode.bits());
        self.sync_keyboard();
    }

    fn pop_keyboard_modes(&mut self, to_pop: u16) {
        self.keyboard.pop(to_pop);
        self.sync_keyboard();
    }

    fn set_keyboard_mode(&mut self, mode: KeyboardModes, behavior: KeyboardModesApplyBehavior) {
        let how = match behavior {
            KeyboardModesApplyBehavior::Replace => 1,
            KeyboardModesApplyBehavior::Union => 2,
            KeyboardModesApplyBehavior::Difference => 3,
        };
        self.keyboard.set(mode.bits(), how);
        self.sync_keyboard();
    }

    /// XTGETTCAP replies are a capability advertisement this product never
    /// made; the request's bytes are fully consumed by the parser.
    fn xtgettcap_response(&mut self, _: String) {}

    fn set_modify_other_keys(&mut self, level: u8) {
        self.inner.set_modify_other_keys(level);
    }

    /// Kitty graphics acknowledgements: previously the APC was discarded
    /// unanswered, and the daemon strips these before they arrive anyway.
    fn kitty_graphics_response(&mut self, _: String) {}

    /// Glyph Protocol replies: same — not a protocol this pane speaks.
    fn glyph_protocol_response(&mut self, _: String) {}

    fn glyph_register(
        &mut self,
        cp: u32,
        payload: rio_vt::ansi::glyph_protocol::GlyphPayload,
        width: u8,
    ) -> Result<(), rio_vt::ansi::glyph_protocol::RegisterError> {
        self.inner.glyph_register(cp, payload, width)
    }

    fn glyph_clear(&mut self, cp: Option<u32>) {
        self.inner.glyph_clear(cp);
    }

    fn glyph_query(&mut self, cp: u32) {
        self.inner.glyph_query(cp);
    }

    fn kitty_chunking_state_mut(
        &mut self,
    ) -> Option<&mut rio_vt::ansi::kitty_graphics_protocol::KittyGraphicsState> {
        self.inner.kitty_chunking_state_mut()
    }

    fn set_scp(&mut self, char_path: ScpCharPath, update_mode: ScpUpdateMode) {
        self.inner.set_scp(char_path, update_mode);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;
    use rio_vt::event::{RioEvent, WindowId};
    use rio_vt::performer::handler::Processor;

    use super::super::size::TermSize;
    use super::*;

    /// A listener that keeps what a parse answered to the pty, in order.
    #[derive(Clone, Default)]
    struct Replies(Arc<Mutex<Vec<String>>>);

    impl EventListener for Replies {
        fn send_event(&self, event: RioEvent, _id: WindowId) {
            if let RioEvent::PtyWrite(_, text) = event {
                self.0.lock().push(text);
            }
        }
    }

    /// Feeds `bytes` through the parser and the bridge the way the pane's
    /// reader does, and returns the replies that reached the pty.
    fn replies(bytes: &[u8]) -> Vec<String> {
        let captured = Replies::default();
        let mut term = Crosswords::new(
            TermSize::new(80, 24),
            CursorShape::Block,
            captured.clone(),
            WindowId::from(0),
            0,
            10_000,
        );
        let mut keyboard = KeyboardStacks::default();
        Processor::default().advance(
            &mut ParseBridge::new(&mut term, captured.clone(), &mut keyboard, None),
            bytes,
        );
        std::mem::take(&mut *captured.0.lock())
    }

    /// Probes branch on these bytes — Claude Code's theme probe ends its
    /// batch on the DA1 answer — so they stay byte-for-byte what this pane
    /// has always answered, whatever Rio would advertise.
    #[test]
    fn device_attributes_keep_the_panes_fingerprint() {
        assert_eq!(replies(b"\x1b[c"), ["\x1b[?6c"]);
        assert_eq!(replies(b"\x1b[>c"), [DA2_REPLY]);
    }

    /// Capability queries this product never answered stay silent: the
    /// version string, the cell pixel size a windowless mirror does not
    /// have, and sixel geometry nothing renders.
    #[test]
    fn unowned_capability_queries_stay_silent() {
        assert!(replies(b"\x1b[>0q").is_empty(), "XTVERSION");
        assert!(replies(b"\x1b[16t").is_empty(), "cell pixel size");
        assert!(replies(b"\x1b[?2;1S").is_empty(), "sixel geometry");
    }

    /// Kitty keyboard pop uses `CSI < n u`; bare `CSI n u` restores
    /// the cursor instead. The query must report the remaining stack top.
    #[test]
    fn keyboard_pop_counts_match_the_protocol_reading() {
        assert_eq!(
            replies(b"\x1b[>3u\x1b[>5u\x1b[<1u\x1b[?u"),
            ["\x1b[?3u"],
            "one entry pops, and the one below takes effect"
        );
        assert_eq!(
            replies(b"\x1b[>3u\x1b[>5u\x1b[<2u\x1b[?u"),
            ["\x1b[?0u"],
            "popping both entries returns to legacy keyboard mode"
        );
    }

    /// A `CSI =` set changes the flags in effect without touching the stack,
    /// and the query says what is in effect.
    #[test]
    fn keyboard_set_is_reported_as_the_flags_in_effect() {
        assert_eq!(replies(b"\x1b[>7u\x1b[=3;1u\x1b[?u"), ["\x1b[?3u"]);
        assert_eq!(replies(b"\x1b[>7u\x1b[=1;3u\x1b[?u"), ["\x1b[?6u"]);
    }

    /// Each screen has a keyboard stack of its own, swapped with it on
    /// `?1049`: what a full-screen program pushed leaves with it, and the
    /// shell's flags come back.
    #[test]
    fn each_screen_has_its_own_keyboard_stack() {
        assert_eq!(
            replies(b"\x1b[>5u\x1b[?1049h\x1b[>3u\x1b[?u\x1b[?1049l\x1b[?u"),
            ["\x1b[?3u", "\x1b[?5u"]
        );
    }

    /// RIS puts both screens' stacks back to nothing, with the rest of the
    /// emulator state.
    #[test]
    fn a_full_reset_clears_the_keyboard_stacks() {
        assert_eq!(replies(b"\x1b[>5u\x1bc\x1b[?u"), ["\x1b[?0u"]);
    }
}
