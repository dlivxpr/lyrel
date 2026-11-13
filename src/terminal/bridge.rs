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
//!   (`CSI ? 62;4;6;22;52 c`). This terminal answers `CSI ? 6 c`, plus attribute
//!   4 only when this pane's connection negotiated the endpoint's static Sixel
//!   admission. The same bytes reach a detached pane from the daemon
//!   (`lyrel_core::daemon::pane::DA1_REPLY`), and a routed remote endpoint that
//!   lacks the feature answers without it. DA2 keeps the exact bytes the pane
//!   has always answered rather than Rio's version number.
//! - `report_version` (XTVERSION): previously met with silence, and a pane
//!   does not start answering Rio's version because the parser changed.
//! - `cells_size_pixels` / `text_area_size_pixels`: geometry is answered once
//!   by the daemon using the controller's device-pixel resize metrics, including
//!   when no GUI is attached. A local mirror must not send a second reply.
//! - `graphics_attribute`: XTSMGRAPHICS is answered once by the daemon, which
//!   owns the pane's pixel geometry and image admission bounds; a mirror reply
//!   would duplicate or over-promise it.
//! - `xtgettcap_response`: bounded live requests are extracted and answered
//!   once by the daemon before publication. This mirror never replies, including
//!   when replaying snapshots created by an older endpoint.
//! - `kitty_graphics_response`, `glyph_protocol_response`: Kitty requests are
//!   owned by the daemon; Glyph is not an advertised product capability.
//! - `set_mouse_cursor_icon`: named OSC 22 pointer claims reach the pane's
//!   paint-side facts through `presentation`; the empty `OSC 22 ;` reset the
//!   upstream callback never sees is released by the reader's own cut.
//!
//! Kitty keyboard operations use the application's shared protocol state,
//! also used by daemon replay. Rio's internal bounded stack and `set` behavior
//! differ, so only the effective flags are applied through its public Handler.
//! This keeps push/pop, screen swaps and mode queries on one contract without
//! changing the emulator's private stack or copying its implementation.
//!
//! Sixel decoding belongs exclusively to the daemon's bounded preparation path.
//! The mirror uses Handler's inactive Sixel defaults, so an unadmitted or foreign
//! DCS cannot allocate a second pixel buffer while its text grid is locked.
//! iTerm2 still delegates with zero cell dimensions until its own adapter lands.

use lyrel_core::core::term_modes::KeyboardStacks;
use rio_vt::ansi::mode::{Mode, PrivateMode};
use rio_vt::ansi::{
    ClearMode, CursorShape, KeyboardModes, KeyboardModesApplyBehavior, LineClearMode,
    TabulationClearMode,
};
use rio_vt::config::colors::ColorRgb;
use rio_vt::crosswords::Crosswords;
use rio_vt::crosswords::attr::Attr;
use rio_vt::crosswords::grid::row::SemanticPrompt;
use rio_vt::crosswords::pos::{CharsetIndex, Column, Line, StandardCharset};
use rio_vt::crosswords::square::Hyperlink;
use rio_vt::event::{EventListener, RioEvent};
use rio_vt::performer::handler::{Handler, ScpCharPath, ScpUpdateMode};

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
    presentation: Option<&'a std::sync::Mutex<super::presentation::ParsePresentation>>,
    /// Daemon-accepted glyph bodies and the negotiated endpoint authority.
    /// `None` for bridges that are not a live rendering consumer.
    glyph: Option<super::glyph::GlyphApply<'a>>,
    /// Live parse-time mirror of the pane's colour layer. Set only for live
    /// output; replay is excluded so an older set inside the ring cannot
    /// resurrect a slot the daemon later reset.
    colors: Option<&'a std::sync::Mutex<lyrel_core::core::pane_colors::PaneColors>>,
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
            presentation: None,
            glyph: None,
            colors: None,
        }
    }

    pub(super) fn with_colors(
        mut self,
        colors: Option<&'a std::sync::Mutex<lyrel_core::core::pane_colors::PaneColors>>,
    ) -> Self {
        self.colors = colors;
        self
    }

    pub(super) fn with_presentation(
        mut self,
        presentation: Option<&'a std::sync::Mutex<super::presentation::ParsePresentation>>,
    ) -> Self {
        self.presentation = presentation;
        self
    }

    pub(super) fn with_glyph(mut self, glyph: Option<super::glyph::GlyphApply<'a>>) -> Self {
        self.glyph = glyph;
        self
    }

    fn reply(&self, text: impl Into<String>) {
        self.proxy.send_event(
            RioEvent::PtyWrite(self.inner.route_id, text.into()),
            self.inner.window_id,
        );
    }

    fn sync_keyboard(&mut self, retire: bool) {
        if retire && let Some(presentation) = self.presentation {
            presentation.lock().unwrap().invalidate_keyboard();
        }
        Handler::set_keyboard_mode(
            self.inner,
            KeyboardModes::from_bits_truncate(self.keyboard.active()),
            KeyboardModesApplyBehavior::Replace,
        );
    }

    pub(super) fn clear_keyboard(&mut self) {
        self.keyboard.clear();
        self.sync_keyboard(true);
    }

    fn sync_screen(&mut self, was_alt: bool, mode: PrivateMode) {
        let is_alt = self
            .inner
            .mode()
            .contains(rio_vt::crosswords::Mode::ALT_SCREEN);
        if was_alt == is_alt {
            return;
        }
        if let Some(presentation) = self.presentation {
            presentation.lock().unwrap().invalidate_keyboard();
        }
        if is_alt && let Some(presentation) = self.presentation {
            presentation.lock().unwrap().invalidate_reading();
        }
        if mode.raw() == 1049 {
            self.keyboard.swap();
            self.sync_keyboard(false);
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
            None => {
                // The endpoint that decodes and retains the image decides the
                // answer: a routed remote server that never negotiated static
                // Sixel must not inherit this build's claim.
                let sixel = self
                    .presentation
                    .is_some_and(|facts| facts.lock().unwrap().endpoint_sixel);
                self.reply(if sixel { "\x1b[?4;6c" } else { "\x1b[?6c" })
            }
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
        if let Some(presentation) = self.presentation {
            let mut facts = presentation.lock().unwrap();
            facts.ensure_tab_stops(self.inner.columns());
            facts.set_tab_stop(self.inner.grid.cursor.pos.col.0);
        }
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
        let clears_history = matches!(mode, ClearMode::Saved);
        self.inner.clear_screen(mode);
        if clears_history && let Some(presentation) = self.presentation {
            presentation.lock().unwrap().invalidate_reading();
        }
    }

    fn set_tabs(&mut self, interval: u16) {
        self.inner.set_tabs(interval);
    }

    fn clear_tabs(&mut self, mode: TabulationClearMode) {
        if let Some(presentation) = self.presentation {
            let mut facts = presentation.lock().unwrap();
            facts.ensure_tab_stops(self.inner.columns());
            match &mode {
                TabulationClearMode::Current => {
                    facts.clear_tab_stop(self.inner.grid.cursor.pos.col.0)
                }
                TabulationClearMode::All => facts.clear_all_tab_stops(),
            }
        }
        self.inner.clear_tabs(mode);
    }

    fn reset_state(&mut self) {
        *self.keyboard = KeyboardStacks::default();
        self.inner.reset_state();
        if let Some(glyph) = self.glyph {
            glyph.reset();
        }
        if let Some(presentation) = self.presentation {
            let mut facts = presentation.lock().unwrap();
            facts.invalidate_reading();
            facts.invalidate_keyboard();
            facts.reset_tabs(self.inner.columns());
            // A full reset releases any application pointer claim with the
            // rest of the emulator state.
            facts.set_pointer(None);
        }
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
        if let Some(colors) = self.colors {
            colors
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .set(index as u16, [color.r, color.g, color.b]);
        }
        self.inner.set_color(index, color);
    }

    fn dynamic_color_sequence(&mut self, sequence: String, index: usize, terminator: &str) {
        self.inner
            .dynamic_color_sequence(sequence, index, terminator);
    }

    fn reset_color(&mut self, index: usize) {
        if let Some(colors) = self.colors {
            colors
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .reset(index as u16);
        }
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

    /// XTSMGRAPHICS is answered once by the daemon, which owns the pane's
    /// pixel geometry and image admission bounds; this mirror never replies,
    /// attached or detached, so a replayed query cannot claim geometry.
    fn graphics_attribute(&mut self, _: u16, _: u16) {}

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

    /// A named OSC 22 claim becomes the pane's visible pointer override only
    /// when a presentation is attached — the GUI reader always hands one
    /// over; bridges driven without it simply drop the claim like before.
    /// The empty `OSC 22 ;` release never reaches this callback upstream, so
    /// the reader's own OSC tokenizer issues that cut.
    fn set_mouse_cursor_icon(&mut self, icon: cursor_icon::CursorIcon) {
        if let Some(presentation) = self.presentation {
            presentation
                .lock()
                .unwrap()
                .set_pointer(Some(super::pointer::cursor_style(icon)));
        }
    }

    fn set_progress_report(&mut self, report: rio_vt::event::ProgressReport) {
        self.inner.set_progress_report(report);
    }

    fn report_keyboard_mode(&mut self) {
        self.reply(format!("\x1b[?{}u", self.keyboard.active()));
    }

    fn push_keyboard_mode(&mut self, mode: KeyboardModes) {
        self.keyboard.push(mode.bits());
        self.sync_keyboard(true);
    }

    fn pop_keyboard_modes(&mut self, to_pop: u16) {
        self.keyboard.pop(to_pop);
        self.sync_keyboard(true);
    }

    fn set_keyboard_mode(&mut self, mode: KeyboardModes, behavior: KeyboardModesApplyBehavior) {
        let previous = self.keyboard.active();
        let how = match behavior {
            KeyboardModesApplyBehavior::Replace => 1,
            KeyboardModesApplyBehavior::Union => 2,
            KeyboardModesApplyBehavior::Difference => 3,
        };
        self.keyboard.set(mode.bits(), how);
        self.sync_keyboard(previous != self.keyboard.active());
    }

    /// The daemon owns every live discovery reply. Historical snapshots may
    /// still contain queries; a mirror must never inject them into a later PTY.
    fn xtgettcap_response(&mut self, _: String) {}

    fn set_modify_other_keys(&mut self, level: u8) {
        self.inner.set_modify_other_keys(level);
    }

    /// Kitty graphics acknowledgements: previously the APC was discarded
    /// unanswered, and the daemon strips these before they arrive anyway.
    fn kitty_graphics_response(&mut self, _: String) {}

    /// Glyph Protocol replies: the daemon is the only writer of `s`/`r`/`c`
    /// acknowledgements and of the frontend-bound `q` answer. A consumer that
    /// repeated them would double-answer the PTY.
    fn glyph_protocol_response(&mut self, _: String) {}

    /// The daemon already validated this body and re-emitted it into the
    /// ordered stream. Derive the picture here; without the negotiated
    /// `glyph-registry` authority nothing is accepted or claimed.
    fn glyph_register(
        &mut self,
        cp: u32,
        payload: rio_vt::ansi::glyph_protocol::GlyphPayload,
        width: u8,
        layout: rio_graphics::glyph_layout::GlyphLayout,
    ) -> Result<(), rio_vt::ansi::glyph_protocol::RegisterError> {
        if let Some(glyph) = self.glyph
            && let rio_vt::ansi::glyph_protocol::GlyphPayload::Glyf { glyf, upm } = payload
        {
            glyph.register(cp, &glyf, upm, width, layout);
        }
        Ok(())
    }

    fn glyph_clear(&mut self, cp: Option<u32>) {
        if let Some(glyph) = self.glyph {
            glyph.clear(cp);
        }
    }

    /// Queries are answered by the daemon from the controller's font coverage.
    /// The consumer never writes a `q` reply, and an old endpoint's raw query
    /// stays unanswered rather than being answered from parsed bytes.
    fn glyph_query(&mut self, _: u32) {}

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

    #[test]
    fn sixel_mirror_never_starts_an_unadmitted_pixel_decoder() {
        let captured = Replies::default();
        let mut term = Crosswords::new(
            TermSize::new(80, 24),
            CursorShape::Block,
            captured.clone(),
            WindowId::from(0),
            0,
            100,
        );
        let mut keyboard = KeyboardStacks::default();
        let mut processor = Processor::default();
        {
            let mut bridge = ParseBridge::new(&mut term, captured, &mut keyboard, None);
            processor.advance(&mut bridge, b"\x1bPq#0;2;100;0;0~");
            assert!(
                !bridge.is_sixel_graphic_active(),
                "pixels belong exclusively to daemon admission, never the locked text mirror"
            );
            processor.advance(&mut bridge, b"\x1b\\AFTER");
        }
        assert_eq!(term.grid[Line(0)][Column(0)].c(), 'A');
        assert_eq!(term.grid[Line(0)][Column(4)].c(), 'R');
        assert_eq!(term.grid.cursor.pos.col, Column(5));
    }

    /// Probes branch on these bytes — Claude Code's theme probe ends its
    /// batch on the DA1 answer — so they stay byte-for-byte what this pane
    /// has always answered, whatever Rio would advertise.
    #[test]
    fn device_attributes_keep_the_panes_fingerprint() {
        assert_eq!(replies(b"\x1b[c"), ["\x1b[?6c"]);
        assert_eq!(replies(b"\x1b[>c"), [DA2_REPLY]);
    }

    /// The attached client answers DA1 for the endpoint that decodes and
    /// retains the image: an older daemon or a routed remote server that never
    /// negotiated static Sixel must not inherit this build's attribute 4.
    #[test]
    fn da1_sixel_attribute_follows_the_negotiated_endpoint() {
        let facts = std::sync::Mutex::new(super::super::presentation::ParsePresentation::new(80));
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
        {
            let mut bridge = ParseBridge::new(&mut term, captured.clone(), &mut keyboard, None)
                .with_presentation(Some(&facts));
            Processor::default().advance(&mut bridge, b"\x1b[c");
        }
        assert_eq!(
            std::mem::take(&mut *captured.0.lock()),
            ["\x1b[?6c"],
            "an endpoint without the feature is advertised without Sixel"
        );
        facts.lock().unwrap().endpoint_sixel = true;
        {
            let mut bridge = ParseBridge::new(&mut term, captured.clone(), &mut keyboard, None)
                .with_presentation(Some(&facts));
            Processor::default().advance(&mut bridge, b"\x1b[c");
        }
        assert_eq!(
            std::mem::take(&mut *captured.0.lock()),
            ["\x1b[?4;6c"],
            "a negotiated endpoint admits static Sixel"
        );
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

    #[test]
    fn xtgettcap_mirror_never_answers_historical_queries() {
        assert!(replies(b"\x1bP+q436f;524742;544e;4d73\x1b\\historical text").is_empty());
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

    fn colors_after(bytes: &[u8], track: bool) -> lyrel_core::core::pane_colors::PaneColors {
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
        let mirror = std::sync::Mutex::new(lyrel_core::core::pane_colors::PaneColors::default());
        let mut bridge = ParseBridge::new(&mut term, captured.clone(), &mut keyboard, None);
        if track {
            bridge = bridge.with_colors(Some(&mirror));
        }
        Processor::default().advance(&mut bridge, bytes);
        *mirror.lock().unwrap()
    }

    /// Live output updates the pane's colour layer at the exact stream
    /// position; a replay (`track = false`) must not, or an old set inside the
    /// ring could resurrect a slot the daemon later reset.
    #[test]
    fn live_output_tracks_colours_and_replay_does_not() {
        let set = b"\x1b]4;1;#112233\x07\x1b]10;#0a0b0c\x07\x1b]4;1;?\x07";
        let live = colors_after(set, true);
        assert_eq!(live.get(1), Some([0x11, 0x22, 0x33]));
        assert_eq!(live.get(256), Some([0x0a, 0x0b, 0x0c]));

        let replay = colors_after(set, false);
        assert!(
            replay.is_empty(),
            "a replayed set must not mutate the live override layer"
        );

        let reset = colors_after(b"\x1b]104\x07\x1b]110\x07", true);
        assert!(reset.is_empty());
    }
}
