//! macOS platform wrapper: the home of lyrel's `font_thicken` switch now that
//! the published gpui 0.3.9 has no `set_glyph_dilation` entry point (the fork
//! carried one; gpui-pre-apple only reads the user's `AppleFontSmoothing`
//! preference).
//!
//! The switch must reach the rasterizer to mean anything: gpui asks the
//! platform text system for a dilation per glyph color
//! (`Window::paint_glyph` → `TextSystem::glyph_dilation_for_color`,
//! gpui-pre-0.3.9 window.rs:4847), the value rides in `RenderGlyphParams`
//! into both the sprite-atlas cache key and `rasterize_glyph`, so returning 0
//! there is exactly the fork's `GLYPH_DILATION = false` behavior — stroke
//! thickening off for lyrel's text, nothing else touched. In particular the
//! `AppleFontSmoothing` preference is never written: doing so would also
//! un-smooth AppKit's own menus and alerts (#1119).
//!
//! Injection goes through the one public seam gpui offers,
//! `Application::with_platform`: `LyrelPlatform` delegates every `Platform`
//! method — including the ones the trait gives a default body for, so nothing
//! silently degrades to a no-op — and answers `text_system()` with
//! `LyrelTextSystem`, which delegates every `PlatformTextSystem` method and
//! overrides only `glyph_dilation_for_color`.

use std::borrow::Cow;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use futures_channel::oneshot;
use gpui_kit::{
    Action, ActivationPolicy, ActivityGuard, AnyWindowHandle, AppLifecyclePhase, Application,
    BackgroundExecutor, Bounds, ClipboardItem, ClipboardReadError, CursorStyle, DevicePixels, Font,
    FontId, FontMetrics, FontRun, ForegroundExecutor, GlyphId, GraphicalEnvironment, Hsla, Keymap,
    LineLayout, Menu, MenuItem, MissingGlyphSink, OwnedMenu, PathPromptOptions, Pixels, Platform,
    PlatformDisplay, PlatformGestures, PlatformKeyboardLayout, PlatformKeyboardMapper,
    PlatformTextSystem, PlatformWindow, RenderGlyphParams, Result, ScreenCaptureSource, Size,
    SystemNotification, SystemNotificationResponse, Task, TextRenderingMode, ThermalState,
    WindowAppearance, WindowButtonLayout, WindowParams, WindowingRequest,
};
use smallvec::SmallVec;

/// Builds the application platform with lyrel's overrides applied.
///
/// `font_thicken` is baked in here — handed over before any text is drawn, as
/// the setting has always required a restart to move (it is saved only, see
/// `LyrelApp::set_font_thicken`). Everything else about the platform is the
/// published macOS backend, unmodified.
pub(crate) fn application(font_thicken: bool) -> Application {
    let inner = gpui_kit::platform::current_platform(false);
    Application::with_platform(Rc::new(LyrelPlatform::new(inner, font_thicken)))
}

struct LyrelPlatform {
    inner: Rc<dyn Platform>,
    text_system: Arc<LyrelTextSystem>,
}

impl LyrelPlatform {
    fn new(inner: Rc<dyn Platform>, font_thicken: bool) -> Self {
        let text_system = Arc::new(LyrelTextSystem {
            inner: inner.text_system(),
            thicken: font_thicken,
        });
        Self { inner, text_system }
    }
}

/// Delegates every text-system call to the published macOS text system except
/// `glyph_dilation_for_color`, which is where `font_thicken: false` actually
/// lands: 0 dilation means CoreGraphics strokes stay as drawn instead of
/// being emboldened by the luminance-based amount the system would pick.
struct LyrelTextSystem {
    inner: Arc<dyn PlatformTextSystem>,
    thicken: bool,
}

impl PlatformTextSystem for LyrelTextSystem {
    fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        self.inner.add_fonts(fonts)
    }

    fn set_missing_glyph_sink(&self, sink: Option<Arc<dyn MissingGlyphSink>>) {
        self.inner.set_missing_glyph_sink(sink)
    }

    fn all_font_names(&self) -> Vec<String> {
        self.inner.all_font_names()
    }

    fn font_id(&self, descriptor: &Font) -> Result<FontId> {
        self.inner.font_id(descriptor)
    }

    fn prewarm_fonts(&self, font_ids: &[FontId]) {
        self.inner.prewarm_fonts(font_ids)
    }

    fn font_metrics(&self, font_id: FontId) -> FontMetrics {
        self.inner.font_metrics(font_id)
    }

    fn typographic_bounds(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Bounds<f32>> {
        self.inner.typographic_bounds(font_id, glyph_id)
    }

    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        self.inner.advance(font_id, glyph_id)
    }

    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        self.inner.glyph_for_char(font_id, ch)
    }

    fn glyph_raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        self.inner.glyph_raster_bounds(params)
    }

    fn rasterize_glyph(
        &self,
        params: &RenderGlyphParams,
        raster_bounds: Bounds<DevicePixels>,
    ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
        self.inner.rasterize_glyph(params, raster_bounds)
    }

    fn layout_line(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> LineLayout {
        self.inner.layout_line(text, font_size, runs)
    }

    fn recommended_rendering_mode(&self, font_id: FontId, font_size: Pixels) -> TextRenderingMode {
        self.inner.recommended_rendering_mode(font_id, font_size)
    }

    fn glyph_dilation_for_color(&self, color: Hsla) -> u8 {
        // Off means 0 regardless of what the user preference reports; on
        // defers to the inner system, which still honors `AppleFontSmoothing`
        // being 0 — this switch can only ever take dilation away, matching
        // the fork's GLYPH_DILATION flag.
        if self.thicken {
            self.inner.glyph_dilation_for_color(color)
        } else {
            0
        }
    }
}

impl Platform for LyrelPlatform {
    fn background_executor(&self) -> BackgroundExecutor {
        self.inner.background_executor()
    }

    fn foreground_executor(&self) -> ForegroundExecutor {
        self.inner.foreground_executor()
    }

    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.text_system.clone()
    }

    fn run(&self, on_finish_launching: Box<dyn 'static + FnOnce()>) {
        self.inner.run(on_finish_launching)
    }

    fn quit(&self) {
        self.inner.quit()
    }

    fn set_initial_windowing(&self, request: WindowingRequest) {
        self.inner.set_initial_windowing(request)
    }

    fn request_windowing(&self, request: WindowingRequest) -> Task<Result<()>> {
        self.inner.request_windowing(request)
    }

    fn restart(&self, binary_path: Option<PathBuf>, arguments: Vec<OsString>) {
        self.inner.restart(binary_path, arguments)
    }

    fn activate(&self, ignoring_other_apps: bool) {
        self.inner.activate(ignoring_other_apps)
    }

    fn set_activation_policy(&self, policy: ActivationPolicy) {
        self.inner.set_activation_policy(policy)
    }

    fn hide(&self) {
        self.inner.hide()
    }

    fn hide_other_apps(&self) {
        self.inner.hide_other_apps()
    }

    fn unhide_other_apps(&self) {
        self.inner.unhide_other_apps()
    }

    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        self.inner.displays()
    }

    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        self.inner.primary_display()
    }

    fn on_displays_changed(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_displays_changed(callback)
    }

    fn active_window(&self) -> Option<AnyWindowHandle> {
        self.inner.active_window()
    }

    fn window_stack(&self) -> Option<Vec<AnyWindowHandle>> {
        self.inner.window_stack()
    }

    fn is_screen_capture_supported(&self) -> bool {
        self.inner.is_screen_capture_supported()
    }

    fn screen_capture_sources(
        &self,
    ) -> oneshot::Receiver<Result<Vec<Rc<dyn ScreenCaptureSource>>>> {
        self.inner.screen_capture_sources()
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        options: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>> {
        self.inner.open_window(handle, options)
    }

    fn window_appearance(&self) -> WindowAppearance {
        self.inner.window_appearance()
    }

    fn set_window_appearance(&self, appearance: Option<WindowAppearance>) {
        self.inner.set_window_appearance(appearance)
    }

    fn button_layout(&self) -> Option<WindowButtonLayout> {
        self.inner.button_layout()
    }

    fn open_url(&self, url: &str) {
        self.inner.open_url(url)
    }

    fn on_open_urls(&self, callback: Box<dyn FnMut(Vec<String>)>) {
        self.inner.on_open_urls(callback)
    }

    fn register_url_scheme(&self, url: &str) -> Task<Result<()>> {
        self.inner.register_url_scheme(url)
    }

    fn prompt_for_paths(
        &self,
        options: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        self.inner.prompt_for_paths(options)
    }

    fn prompt_for_new_path(
        &self,
        directory: &Path,
        suggested_name: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        self.inner.prompt_for_new_path(directory, suggested_name)
    }

    fn can_select_mixed_files_and_dirs(&self) -> bool {
        self.inner.can_select_mixed_files_and_dirs()
    }

    fn reveal_path(&self, path: &Path) {
        self.inner.reveal_path(path)
    }

    fn open_with_system(&self, path: &Path) {
        self.inner.open_with_system(path)
    }

    fn on_quit(&self, callback: Box<dyn FnMut() -> bool>) {
        self.inner.on_quit(callback)
    }

    fn on_reopen(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_reopen(callback)
    }

    fn on_system_sleep(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_system_sleep(callback)
    }

    fn on_system_wake(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_system_wake(callback)
    }

    fn on_app_lifecycle(&self, callback: Box<dyn FnMut(AppLifecyclePhase)>) {
        self.inner.on_app_lifecycle(callback)
    }

    fn on_memory_warning(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_memory_warning(callback)
    }

    fn gestures(&self) -> Option<Rc<dyn PlatformGestures>> {
        self.inner.gestures()
    }

    fn set_menus(&self, menus: Vec<Menu>, keymap: &Keymap) {
        self.inner.set_menus(menus, keymap)
    }

    fn get_menus(&self) -> Option<Vec<OwnedMenu>> {
        self.inner.get_menus()
    }

    fn set_dock_menu(&self, menu: Vec<MenuItem>, keymap: &Keymap) {
        self.inner.set_dock_menu(menu, keymap)
    }

    fn perform_dock_menu_action(&self, action: usize) {
        self.inner.perform_dock_menu_action(action)
    }

    fn add_recent_document(&self, path: &Path) {
        self.inner.add_recent_document(path)
    }

    fn update_jump_list(
        &self,
        menus: Vec<MenuItem>,
        entries: Vec<SmallVec<[PathBuf; 2]>>,
    ) -> Task<Vec<SmallVec<[PathBuf; 2]>>> {
        self.inner.update_jump_list(menus, entries)
    }

    fn on_app_menu_action(&self, callback: Box<dyn FnMut(&dyn Action)>) {
        self.inner.on_app_menu_action(callback)
    }

    fn on_will_open_app_menu(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_will_open_app_menu(callback)
    }

    fn on_validate_app_menu_command(&self, callback: Box<dyn FnMut(&dyn Action) -> bool>) {
        self.inner.on_validate_app_menu_command(callback)
    }

    fn thermal_state(&self) -> ThermalState {
        self.inner.thermal_state()
    }

    fn on_thermal_state_change(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_thermal_state_change(callback)
    }

    fn prevent_idle_sleep(&self, reason: &str) -> Task<Result<ActivityGuard>> {
        self.inner.prevent_idle_sleep(reason)
    }

    fn set_app_identity(&self, identifier: &str, name: &str) {
        self.inner.set_app_identity(identifier, name)
    }

    fn show_system_notification(&self, notification: SystemNotification) {
        self.inner.show_system_notification(notification)
    }

    fn dismiss_system_notification(&self, tag: &str) {
        self.inner.dismiss_system_notification(tag)
    }

    fn on_system_notification_response(
        &self,
        callback: Box<dyn FnMut(SystemNotificationResponse)>,
    ) {
        self.inner.on_system_notification_response(callback)
    }

    fn compositor_name(&self) -> &'static str {
        self.inner.compositor_name()
    }

    fn graphical_environment(&self) -> Option<GraphicalEnvironment> {
        self.inner.graphical_environment()
    }

    fn app_path(&self) -> Result<PathBuf> {
        self.inner.app_path()
    }

    fn path_for_auxiliary_executable(&self, name: &str) -> Result<PathBuf> {
        self.inner.path_for_auxiliary_executable(name)
    }

    fn set_cursor_style(&self, style: CursorStyle) {
        self.inner.set_cursor_style(style)
    }

    fn hide_cursor_until_mouse_moves(&self) {
        self.inner.hide_cursor_until_mouse_moves()
    }

    fn is_cursor_visible(&self) -> bool {
        self.inner.is_cursor_visible()
    }

    fn should_auto_hide_scrollbars(&self) -> bool {
        self.inner.should_auto_hide_scrollbars()
    }

    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        self.inner.read_from_clipboard()
    }

    fn write_to_clipboard(&self, item: ClipboardItem) {
        self.inner.write_to_clipboard(item)
    }

    fn read_from_clipboard_async(&self) -> Task<Result<Option<ClipboardItem>, ClipboardReadError>> {
        self.inner.read_from_clipboard_async()
    }

    fn read_from_find_pasteboard(&self) -> Option<ClipboardItem> {
        self.inner.read_from_find_pasteboard()
    }

    fn write_to_find_pasteboard(&self, item: ClipboardItem) {
        self.inner.write_to_find_pasteboard(item)
    }

    fn write_credentials(&self, url: &str, username: &str, password: &[u8]) -> Task<Result<()>> {
        self.inner.write_credentials(url, username, password)
    }

    fn read_credentials(&self, url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        self.inner.read_credentials(url)
    }

    fn delete_credentials(&self, url: &str) -> Task<Result<()>> {
        self.inner.delete_credentials(url)
    }

    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        self.inner.keyboard_layout()
    }

    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        self.inner.keyboard_mapper()
    }

    fn on_keyboard_layout_change(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_keyboard_layout_change(callback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::NoopTextSystem;
    use objc2_foundation::{NSArgumentDomain, NSUserDefaults, ns_string};

    /// A text system whose dilation answer is non-zero, so the wrapper's gate
    /// is distinguishable from a passthrough of 0. Everything else goes to
    /// gpui's no-op implementation; only the dilation answer is asserted on.
    struct ThickenedTextSystem(NoopTextSystem);

    impl PlatformTextSystem for ThickenedTextSystem {
        fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
            self.0.add_fonts(fonts)
        }

        fn all_font_names(&self) -> Vec<String> {
            self.0.all_font_names()
        }

        fn font_id(&self, descriptor: &Font) -> Result<FontId> {
            self.0.font_id(descriptor)
        }

        fn font_metrics(&self, font_id: FontId) -> FontMetrics {
            self.0.font_metrics(font_id)
        }

        fn typographic_bounds(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Bounds<f32>> {
            self.0.typographic_bounds(font_id, glyph_id)
        }

        fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
            self.0.advance(font_id, glyph_id)
        }

        fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
            self.0.glyph_for_char(font_id, ch)
        }

        fn glyph_raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
            self.0.glyph_raster_bounds(params)
        }

        fn rasterize_glyph(
            &self,
            params: &RenderGlyphParams,
            raster_bounds: Bounds<DevicePixels>,
        ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
            self.0.rasterize_glyph(params, raster_bounds)
        }

        fn layout_line(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> LineLayout {
            self.0.layout_line(text, font_size, runs)
        }

        fn recommended_rendering_mode(
            &self,
            font_id: FontId,
            font_size: Pixels,
        ) -> TextRenderingMode {
            self.0.recommended_rendering_mode(font_id, font_size)
        }

        fn glyph_dilation_for_color(&self, _color: Hsla) -> u8 {
            3
        }
    }

    fn white() -> Hsla {
        gpui_kit::hsla(0., 0., 1., 1.)
    }

    /// The switch is what the rasterizer sees: off forces 0 — the glyph cache
    /// keys and `rasterize_glyph` both take the value from
    /// `RenderGlyphParams.dilation`, so this is the fork's GLYPH_DILATION=off
    /// behavior — and on defers to the system's own per-color answer.
    #[test]
    fn the_thicken_switch_gates_dilation() {
        let inner = Arc::new(ThickenedTextSystem(NoopTextSystem::new()));

        let thinned = LyrelTextSystem {
            inner: inner.clone(),
            thicken: false,
        };
        assert_eq!(thinned.glyph_dilation_for_color(white()), 0);

        let thickened = LyrelTextSystem {
            inner,
            thicken: true,
        };
        assert_eq!(thickened.glyph_dilation_for_color(white()), 3);
    }

    /// #1119: turning thickening off must not reach AppKit. `AppleFontSmoothing`
    /// in any domain this process reads restyles its native menus and alerts,
    /// so the switch may only ever talk to gpui's rasterizer — which is now
    /// the wrapper above, and it must stay free of preference writes.
    #[test]
    fn turning_thickening_off_leaves_the_smoothing_preference_alone() {
        let defaults = NSUserDefaults::standardUserDefaults();
        let key = ns_string!("AppleFontSmoothing");
        let before = defaults.objectForKey(key);

        let system = LyrelTextSystem {
            inner: Arc::new(ThickenedTextSystem(NoopTextSystem::new())),
            thicken: false,
        };
        assert_eq!(system.glyph_dilation_for_color(white()), 0);

        // SAFETY: a Foundation constant, initialized before `main` runs.
        let arguments = defaults.volatileDomainForName(unsafe { NSArgumentDomain });
        assert!(arguments.objectForKey(key).is_none());
        assert_eq!(defaults.objectForKey(key).is_some(), before.is_some());
    }
}
