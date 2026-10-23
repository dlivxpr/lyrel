//! #23：真正原生窗口中的共享材质 feasibility probe，不是生产工作台。
//!
//! 运行：`cargo run --locked --example native_material_gate -- --help`。
//! 可通过启动参数直接设定主题、材质、窗口逻辑尺寸、UI zoom 和 opacity；不需要先操作 UI。
//! 不加载 lyrel Config，不连接 session/daemon/PTY，不读写用户配置或主题。
//!
//! 唯一设计来源：主工作树 `docs/research/main-ui-draft.html`：
//! 188-202（整窗底）、211-225（左侧栏）、517-522（透明终端）、981-993（右侧栏）、
//! 1880-1884（原生边界）。Root 只绘制一次窗口底；左右栏分别以 20% / 17% tint
//! 叠在同一个窗口 material 上，terminal-like 区不再铺底。HTML 不是原生效果证据。
//!
//! Opaque：整窗 alpha=1，无桌面透出；Windows 可使用 ClearType。
//! Transparent / Windows Off：普通 alpha，不能期望模糊。
//! Blurred / Windows Blur：Windows WCA acrylic（build >=17763），macOS native blur。
//! Windows Auto：遵循 legacy blur；Mica/Mica Alt/Acrylic 在 build >=22621 使用 DWM
//! 2/4/3，旧系统依照生产 policy 降为 Blur 或 Transparent。alpha=1 时 Windows 的
//! Transparent/Blurred 转 Opaque；DWM 材质仍保留请求但会被不透明内容遮住。
//! macOS 不提供 Windows 模式；锁定后端通过 NSVisualEffectView 的 BehindWindow blur，
//! 不承诺浏览器 blur radius/saturation。此样例不验证真实终端字形栅格化或 PTY 行为。
//!
//! 此处最小 policy 重复仅用于隔离 throwaway 样例；禁止将它接入生产作为第二 policy。
//! 真正 Windows HWND 实现通过 #[path] 复用自有模块。API 请求日志不等于 compositor
//! 成功，必须另有实窗、激活/失活、移动/resize/DPI、Latin/CJK 对比度证据才能判断 gate。

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{Selectable, Sizable, Theme, TitleBar};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

#[path = "native_material_gate/options.rs"]
mod options;
#[path = "native_material_gate/policy.rs"]
mod policy;
#[path = "native_material_gate/tokens.rs"]
mod tokens;

#[cfg(target_os = "windows")]
#[path = "../src/ui/backdrop.rs"]
mod native_backdrop;

// 自有 backdrop.rs 的唯一依赖 seam；不引入完整应用与 Config 全局。
#[cfg(target_os = "windows")]
mod ui {
    pub(crate) mod theme {
        pub(crate) use crate::policy::{BackdropAppearance, MaterialPlan, material_plan};
    }
}

use options::Options;
use policy::{BackdropAppearance, Material};
use tokens::{Palette, ProbeTheme};

actions!(native_material_gate, [
    NextMaterial, NextTheme, NextZoom, NextOpacity, ReplayMaterial,
    ToggleWidth, ToggleLegacyBlur, ToggleFullscreen, FocusProbe, QuitProbe,
]);

struct StderrLogger;
static LOGGER: StderrLogger = StderrLogger;
impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }
    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}", record.level(), record.args());
        }
    }
    fn flush(&self) {}
}

fn main() {
    let options = match Options::parse() {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{}", options::HELP);
            return;
        }
        Err(error) => {
            eprintln!("{error}\n{}", options::HELP);
            std::process::exit(2);
        }
    };
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.bind_keys([
                KeyBinding::new("f1", NextMaterial, Some("NativeMaterialGate")),
                KeyBinding::new("f2", NextTheme, Some("NativeMaterialGate")),
                KeyBinding::new("f3", NextZoom, Some("NativeMaterialGate")),
                KeyBinding::new("f4", NextOpacity, Some("NativeMaterialGate")),
                KeyBinding::new("f5", ReplayMaterial, Some("NativeMaterialGate")),
                KeyBinding::new("f6", ToggleWidth, Some("NativeMaterialGate")),
                KeyBinding::new("f7", ToggleLegacyBlur, Some("NativeMaterialGate")),
                KeyBinding::new("f8", ToggleFullscreen, Some("NativeMaterialGate")),
                KeyBinding::new("escape", FocusProbe, Some("NativeMaterialGate")),
                KeyBinding::new("ctrl-q", QuitProbe, Some("NativeMaterialGate")),
                KeyBinding::new("cmd-q", QuitProbe, Some("NativeMaterialGate")),
            ]);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            }).detach();
            let alpha = if options.material == Material::Opaque { 1.0 } else { options.opacity };
            let appearance = options.material.resolve(options.legacy_blur, alpha);
            let window_options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None, size(px(options.width), px(options.height)), cx,
                ))),
                window_min_size: Some(size(px(840.), px(600.))),
                window_background: appearance.as_gpui(),
                window_decorations: Some(WindowDecorations::Client),
                titlebar: Some(TitlebarOptions {
                    title: Some("lyrel #23 — 原生整窗材质 / Native material gate".into()),
                    ..TitleBar::title_bar_options()
                }),
                ..TitleBar::window_options()
            };
            gpui_kit::open_window(window_options, cx, move |window, cx| {
                cx.new(|cx| MaterialProbe::new(options, window, cx))
            }).expect("failed to open native material probe");
            cx.activate(true);
        });
}

struct MaterialProbe {
    options: Options,
    palette: Palette,
    focus: FocusHandle,
    inputs: [Entity<InputState>; 3],
    appearance: BackdropAppearance,
    last_scale: f32,
    native_requests: u64,
    activation_events: u64,
    bounds_events: u64,
    _activation_watch: Subscription,
    _bounds_watch: Subscription,
}

impl MaterialProbe {
    fn new(options: Options, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let alpha = if options.material == Material::Opaque { 1.0 } else { options.opacity };
        let appearance = options.material.resolve(options.legacy_blur, alpha);
        let palette = Palette::apply(options.theme, alpha, 16. * options.zoom / 100., cx);
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let inputs = ["Left sidebar · 左侧栏", "Terminal-like · 中文输入 ABC", "Right sidebar · 右侧栏"]
            .map(|value| cx.new(|cx| InputState::new(window, cx).default_value(value)));
        let activation_watch = cx.observe_window_activation(window, |this, _window, cx| {
            this.activation_events += 1;
            // 与生产工作台、Settings 一样，Windows 激活与失活都重放；其他平台不写。
            #[cfg(target_os = "windows")]
            this.replay(_window, "activation");
            cx.notify();
        });
        let bounds_watch = cx.observe_window_bounds(window, |this, window, cx| {
            this.bounds_events += 1;
            let scale = window.scale_factor();
            if this.last_scale != scale {
                this.last_scale = scale;
                #[cfg(target_os = "windows")]
                this.replay(window, "display-scale");
            }
            // 普通移动与 resize 只刷新实际几何，不逐帧重设 DWM。
            cx.notify();
        });
        let mut probe = Self {
            options, palette, focus, inputs, appearance,
            last_scale: window.scale_factor(),
            native_requests: 0,
            activation_events: 0,
            bounds_events: 0,
            _activation_watch: activation_watch,
            _bounds_watch: bounds_watch,
        };
        probe.replay(window, "open");
        probe
    }

    fn effective_opacity(&self) -> f32 {
        if self.options.material == Material::Opaque { 1.0 } else { self.options.opacity }
    }

    fn replay(&mut self, window: &Window, reason: &str) {
        policy::apply(window, self.appearance);
        self.native_requests += 1;
        log::info!(
            "material request #{} reason={} requested={} resolved={:?} carrier={:?} alpha={:.2} scale={:.2} active={} (NOT visual acceptance)",
            self.native_requests, reason, self.options.material.id(), self.appearance,
            self.appearance.as_gpui(), self.effective_opacity(), window.scale_factor(),
            window.is_window_active(),
        );
    }

    fn refresh_presentation(&mut self, window: &Window, cx: &mut Context<Self>) {
        let alpha = self.effective_opacity();
        self.palette = Palette::apply(self.options.theme, alpha, 16. * self.options.zoom / 100., cx);
        let next = self.options.material.resolve(self.options.legacy_blur, alpha);
        if next != self.appearance {
            self.appearance = next;
            self.replay(window, "policy-change");
        }
        cx.notify();
    }

    fn next_material(&mut self, window: &Window, cx: &mut Context<Self>) {
        let ix = Material::ALL.iter().position(|material| *material == self.options.material).unwrap();
        self.options.material = Material::ALL[(ix + 1) % Material::ALL.len()];
        self.refresh_presentation(window, cx);
    }

    fn next_theme(&mut self, window: &Window, cx: &mut Context<Self>) {
        let ix = ProbeTheme::ALL.iter().position(|theme| *theme == self.options.theme).unwrap();
        self.options.theme = ProbeTheme::ALL[(ix + 1) % ProbeTheme::ALL.len()];
        self.refresh_presentation(window, cx);
    }

    fn next_zoom(&mut self, cx: &mut Context<Self>) {
        let steps = [75., 100., 125., 150.];
        self.options.zoom = steps.into_iter().find(|zoom| *zoom > self.options.zoom).unwrap_or(75.);
        Theme::update(cx, |theme| theme.font_size = px(16. * self.options.zoom / 100.));
        cx.notify();
    }

    fn next_opacity(&mut self, window: &Window, cx: &mut Context<Self>) {
        let steps = [0.55, 0.76, 0.82, 1.0];
        self.options.opacity = steps.into_iter().find(|alpha| *alpha > self.options.opacity + 0.001).unwrap_or(0.55);
        self.refresh_presentation(window, cx);
    }

    fn toggle_width(&mut self, window: &mut Window) {
        // 逻辑尺寸是 native API 边界，不是产品 UI 的固定像素布局。
        let width = if window.viewport_size().width > px(1200.) { 920. } else { 1440. };
        window.resize(size(px(width), window.viewport_size().height));
    }

    fn toggle_legacy_blur(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.options.legacy_blur = !self.options.legacy_blur;
        self.refresh_presentation(window, cx);
    }

    fn controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div().flex().flex_col().gap_2().p_3().flex_shrink_0()
            .child(div().flex().flex_wrap().gap_2().children(Material::ALL.iter().copied().map(|material| {
                Button::new(material.id()).small().outline().label(material.label())
                    .selected(self.options.material == material)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.options.material = material;
                        this.refresh_presentation(window, cx);
                    }))
            })))
            .child(div().flex().flex_wrap().items_center().gap_2()
                .children(ProbeTheme::ALL.into_iter().map(|theme| {
                    Button::new(theme.id()).small().outline().label(theme.label())
                        .selected(self.options.theme == theme)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.options.theme = theme;
                            this.refresh_presentation(window, cx);
                        }))
                }))
                .child(Button::new("zoom").small().ghost().label(format!("缩放 {:.0}% · F3", self.options.zoom))
                    .on_click(cx.listener(|this, _, _, cx| this.next_zoom(cx))))
                .child(Button::new("opacity").small().ghost().label(format!("底色 α {:.2} · F4", self.options.opacity))
                    .on_click(cx.listener(|this, _, window, cx| this.next_opacity(window, cx))))
                .child(Button::new("replay").small().ghost().label("重放 · F5")
                    .on_click(cx.listener(|this, _, window, cx| { this.replay(window, "manual"); cx.notify(); })))
                .child(Button::new("width").small().ghost().label("宽 / 窄 · F6")
                    .on_click(cx.listener(|this, _, window, _| this.toggle_width(window))))
                .when(cfg!(target_os = "windows"), |row| row.child(
                    Button::new("auto-blur").small().ghost().label(format!("Auto blur {} · F7", self.options.legacy_blur))
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_legacy_blur(window, cx))),
                ))
                .child(Button::new("fullscreen").small().ghost().label("全屏 · F8")
                    .on_click(|_, window, _| window.toggle_fullscreen())))
    }

    fn specimen(&self, ix: usize, title: &'static str, lines: &'static [&'static str]) -> impl IntoElement {
        div().flex().flex_col().gap_3().p_3().min_w_0()
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(title))
            .child(Input::new(&self.inputs[ix]).small())
            .child(div().flex().flex_col().gap_2().text_sm().children(lines.iter().map(|line| {
                div().child(*line)
            })))
    }
}

impl Render for MaterialProbe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let translucent = self.effective_opacity() < 1.0;
        let viewport = window.viewport_size();
        // 缩窄时侧面同比收缩；三面仍都在同一个 window 内，不创建子窗或独立 blur。
        let sidebar_width = if viewport.width / window.rem_size() < 72.0 { 10.0 } else { 14.5 };
        let status = format!(
            "请求 {} → {:?} | {} | {:.0}% | α {:.2} | {:.0}×{:.0} logical | DPI {:.2} | {} | activation {} / bounds {} / native requests {}",
            self.options.material.id(), self.appearance, self.options.theme.id(), self.options.zoom,
            self.effective_opacity(), f32::from(viewport.width), f32::from(viewport.height),
            window.scale_factor(), if window.is_window_active() { "已激活" } else { "未激活" },
            self.activation_events, self.bounds_events, self.native_requests,
        );
        div().id("material-probe").key_context("NativeMaterialGate").track_focus(&self.focus)
            .flex().flex_col().size_full().min_h_0().text_color(self.palette.foreground)
            // 不设置 bg：Kit Root 的 background 是唯一整窗 alpha 底。
            .on_action(cx.listener(|this, _: &NextMaterial, window, cx| this.next_material(window, cx)))
            .on_action(cx.listener(|this, _: &NextTheme, window, cx| this.next_theme(window, cx)))
            .on_action(cx.listener(|this, _: &NextZoom, _, cx| this.next_zoom(cx)))
            .on_action(cx.listener(|this, _: &NextOpacity, window, cx| this.next_opacity(window, cx)))
            .on_action(cx.listener(|this, _: &ReplayMaterial, window, cx| { this.replay(window, "manual"); cx.notify(); }))
            .on_action(cx.listener(|this, _: &ToggleWidth, window, _| this.toggle_width(window)))
            .on_action(cx.listener(|this, _: &ToggleLegacyBlur, window, cx| this.toggle_legacy_blur(window, cx)))
            .on_action(|_: &ToggleFullscreen, window, _| window.toggle_fullscreen())
            .on_action(cx.listener(|this, _: &FocusProbe, window, cx| this.focus.focus(window, cx)))
            .on_action(|_: &QuitProbe, _, cx| cx.quit())
            .child(TitleBar::new().bg(self.palette.background.alpha(0.0))
                .child("lyrel #23 · 拖动此标题栏 / Native material probe"))
            .child(self.controls(cx))
            .child(div().flex().items_stretch().flex_1().min_h_0().min_w_0()
                .child(div().id("left-surface").w(rems(sidebar_width)).h_full().flex_shrink_0().min_h_0()
                    .border_r_1().border_color(self.palette.divider)
                    .bg(self.palette.rail_fill(translucent))
                    .child(self.specimen(0, "左侧栏 · Sidebar-like", &[
                        "一个 window policy", "非独立模糊表面", "Latin / 简体中文 / 繁體中文", "路径 ~/项目/材质检查",
                    ])).overflow_y_scrollbar())
                .child(div().id("terminal-surface").flex_1().h_full().min_w_0().min_h_0()
                    .font_family(Theme::global(cx).mono_font_family.clone())
                    .child(self.specimen(1, "终端式内容 · Terminal-like（无 PTY）", &[
                        "$ printf 'Latin + 中文测试'", "ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz 0123456789", "中文：窗口材质、侧栏、终端、焦点", "日本語：透明なウィンドウ · 한글", "Il1 | O0 | 细线与小字号对比度", "拖动窗口跨越桌面深浅图案，检查三面是否连续。", "上方输入框是真实 Kit Input，仅保存在内存。",
                    ]))
                    .child(div().mx_3().p_2().rounded_md().bg(self.palette.selection).text_sm()
                        .child("选中态 / Selected：正文必须仍可读"))
                    .overflow_y_scrollbar())
                .child(div().id("right-surface").w(rems(sidebar_width)).h_full().flex_shrink_0().min_h_0()
                    .border_l_1().border_color(self.palette.divider)
                    .bg(self.palette.sidebar_fill(translucent))
                    .child(self.specimen(2, "右侧栏 · Inspector-like", &[
                        "与终端共享底层材质", "浅色 / 深色 / 自定义", "F1 / F2 循环材质与主题", "F3 / F4 缩放与 alpha", "Tab 遍历 · Escape 返回根", "激活、失活、resize 后检查残留",
                    ])).overflow_y_scrollbar()))
            .child(div().flex_shrink_0().border_t_1().border_color(self.palette.divider).p_2().text_xs()
                .text_color(self.palette.muted_foreground)
                .child("不透明：不透桌面。透明：透而不模糊。Blur：由系统合成器决定；Mica 不等于实时桌面模糊。alpha=1 会遮住材质；请求成功不是视觉通过。")
                .child("请分别观察三面文字、Kit hover / focus、拖动 / resize / DPI、激活 / 失活；macOS 需独立实机验收，不能由 Windows 推断。"))
            .child(div().flex_shrink_0().border_t_1().border_color(self.palette.divider).px_3().py_1().text_xs()
                .child(status))
    }
}
