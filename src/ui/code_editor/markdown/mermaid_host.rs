//! Mermaid 的原生 Host：同一 Document 派生 cache、异步 request lease 与逐 view provider。
use crate::ui::i18n::{L10nKey, t};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use lyrel_markdown_editor::{
    Document, DocumentEvent, EditorState, MermaidPresentation, mermaid_blocks,
};
use merman::resources::{InputResourcePolicy, ResourceProfile};
use merman::svg::{
    DeterministicTextMeasurer, MeasurementProfileId, SvgPipeline, SvgPostprocessContext,
    SvgPostprocessor, TextMeasurementPolicy, TextMeasurementProfile,
    TextMeasurementProfileIdentity,
};
use merman::{
    Engine, MermaidConfig, OperationControl, ParseOptions, RenderOutput, RenderTarget, Renderer,
    SvgEnvironment, SvgRequest,
};
use std::{
    borrow::Cow,
    cell::RefCell,
    ops::Range,
    rc::Rc,
    sync::{Arc, LazyLock},
    time::Duration,
};

const CACHE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone)]
struct Style {
    family: SharedString,
    size: f32,
    foreground: Hsla,
    background: Hsla,
    border: Hsla,
    dark: bool,
    text_system: Arc<TextSystem>,
}
impl PartialEq for Style {
    fn eq(&self, other: &Self) -> bool {
        self.family == other.family
            && self.size == other.size
            && self.foreground == other.foreground
            && self.background == other.background
            && self.border == other.border
            && self.dark == other.dark
            && Arc::ptr_eq(&self.text_system, &other.text_system)
    }
}
impl Style {
    fn current(cx: &App) -> Self {
        let theme = cx.theme();
        Self {
            family: theme.font_family.clone(),
            size: theme.font_size.into(),
            foreground: theme.foreground,
            background: theme.background,
            border: theme.border,
            dark: theme.is_dark(),
            text_system: cx.text_system().clone(),
        }
    }
    fn engine(&self) -> Engine {
        let css = |color: Hsla| format!("#{:06x}", u32::from(color.to_rgb()) >> 8);
        let family = self.svg_family();
        Engine::new().with_site_config(MermaidConfig::from_value(serde_json::json!({
            "securityLevel": "strict", "theme": "base", "fontFamily": family, "fontSize": self.size,
            "secure": ["secure", "securityLevel", "fontFamily", "fontSize", "theme", "themeVariables", "themeCSS"],
            "themeVariables": { "darkMode": self.dark, "fontFamily": family,
                "fontSize": format!("{}px", self.size), "primaryColor": css(self.background),
                "primaryTextColor": css(self.foreground), "primaryBorderColor": css(self.border),
                "lineColor": css(self.foreground), "textColor": css(self.foreground),
                "background": css(self.background) }
        })))
    }
    fn svg_family(&self) -> String {
        use std::fmt::Write as _;
        // GPUI's virtual system family is not an SVG/CSS family. Match the
        // existing parked-deck resolver and quote before Merman XML-escapes it.
        #[cfg(target_os = "windows")]
        let system = "Segoe UI";
        #[cfg(target_os = "macos")]
        let system = ".AppleSystemUIFont";
        let family = font_name_with_fallbacks(&self.family, system);
        let mut css = String::with_capacity(family.len() + 64);
        css.push('\'');
        for ch in family.chars() {
            match ch {
                '\\' | '\'' => {
                    css.push('\\');
                    css.push(ch);
                }
                ch if ch.is_control() => {
                    write!(css, "\\{:x} ", ch as u32).expect("writing to String");
                }
                ch => css.push(ch),
            }
        }
        css.push_str("', 'Segoe UI', 'Microsoft YaHei', 'PingFang SC', sans-serif");
        css
    }
    fn measurement(&self) -> TextMeasurementPolicy {
        // Shaping and font discovery stay on the existing background job.
        // This uses the host text stack (including Unicode fallback), not a
        // per-character width guess. SVG uses the matching named system font;
        // browser bbox/baseline and custom CSS font differences remain limits.
        // Merman requires a Send + Sync callback. Keep its non-Sync layout
        // cache local to this job and serialize callback access, not UI work.
        let text_system = parking_lot::Mutex::new(WindowTextSystem::new(self.text_system.clone()));
        let family = self.family.clone();
        let measurer =
            DeterministicTextMeasurer::default().with_width_callback(move |text, style| {
                if text.is_empty() {
                    return 0.;
                }
                let mut font = font(family.clone());
                font.weight = match style.font_weight.as_deref() {
                    Some("bold" | "bolder") => FontWeight::BOLD,
                    Some(value) => value
                        .parse::<f32>()
                        .map(FontWeight)
                        .unwrap_or(FontWeight::NORMAL),
                    None => FontWeight::NORMAL,
                };
                font.style = match style.font_style.as_deref() {
                    Some("italic") => FontStyle::Italic,
                    Some("oblique") => FontStyle::Oblique,
                    _ => FontStyle::Normal,
                };
                let text_system = text_system.lock();
                text.split('\n')
                    .map(|line| {
                        text_system
                            .shape_line(
                                SharedString::from(line),
                                px(style.font_size as f32),
                                &[TextRun {
                                    len: line.len(),
                                    font: font.clone(),
                                    ..Default::default()
                                }],
                                None,
                            )
                            .width
                            .as_f32() as f64
                    })
                    .fold(0., f64::max)
            });
        let identity = TextMeasurementProfileIdentity::new(
            MeasurementProfileId::new("lyrel.native-text").expect("static profile id"),
            format!("gpui-kit-fast-0.9.1:{}", self.family),
        )
        .expect("nonempty native measurement version");
        TextMeasurementPolicy::uniform(TextMeasurementProfile::new(identity, measurer))
    }
}
#[derive(Clone, PartialEq)]
struct Key {
    document: EntityId,
    owner: u64,
    request: u64,
    revision: u64,
    source: String,
    style: Style,
}
struct Entry {
    key: Key,
    range: Range<usize>,
    row: usize,
    state: MermaidPresentation,
    control: OperationControl,
}
impl Entry {
    fn release_image(&self, cx: &mut App) {
        if let MermaidPresentation::Ready(image, ..) = &self.state {
            cx.drop_image(image.clone(), None);
        }
    }
}
struct Job {
    key: Key,
    control: OperationControl,
}
#[derive(Default)]
struct Store {
    entries: Vec<Entry>,
    next_owner: u64,
    next_request: u64,
    revision: u64,
}
impl Drop for Store {
    fn drop(&mut self) {
        for entry in &self.entries {
            entry.control.cancel();
        }
    }
}
impl Store {
    fn presentation(&self, row: usize, source: &str) -> MermaidPresentation {
        self.entries
            .iter()
            .find(|entry| entry.row == row && entry.key.source == source)
            .map(|entry| entry.state.clone())
            .unwrap_or_else(|| {
                MermaidPresentation::Loading(t(L10nKey::EditorMarkdownMermaidLoading).into())
            })
    }
    fn sync(
        &mut self,
        document: EntityId,
        revision: u64,
        source: &str,
        event: Option<&DocumentEvent>,
        style: Style,
        cx: &mut App,
    ) -> Vec<Job> {
        // Ordered LF/UTF-8 edits move an unchanged owner, including insertion
        // exactly before its opening fence. Removing the whole owner revokes it
        // even if an identical block is inserted at that offset afterwards.
        if let Some(event) = event {
            for edit in event.edits() {
                let removed = edit.range();
                self.entries.retain_mut(|entry| {
                    if removed.start <= entry.range.start
                        && removed.end >= entry.range.end
                        && !removed.is_empty()
                    {
                        entry.control.cancel();
                        entry.release_image(cx);
                        return false;
                    }
                    if removed.end <= entry.range.start {
                        let delta = edit.inserted().len() as isize - removed.len() as isize;
                        entry.range = entry.range.start.saturating_add_signed(delta)
                            ..entry.range.end.saturating_add_signed(delta);
                    } else if removed.start < entry.range.end {
                        entry.range.end = edit.map_offset(entry.range.end);
                    }
                    true
                });
            }
        }
        let mut previous = std::mem::take(&mut self.entries);
        let mut jobs = Vec::new();
        for block in mermaid_blocks(source) {
            let range = block.range();
            let existing = previous
                .iter()
                .position(|entry| entry.range.start == range.start);
            let old = existing.map(|ix| previous.swap_remove(ix));
            if let Some(mut entry) = old {
                if entry.key.document == document
                    && entry.key.source == block.source()
                    && entry.key.style == style
                {
                    entry.range = range;
                    entry.row = block.rows().start;
                    self.entries.push(entry);
                    continue;
                }
                entry.control.cancel();
                entry.release_image(cx);
                self.next_request += 1;
                entry.key.request = self.next_request;
                entry.key.revision = revision;
                entry.key.source = block.source().to_owned();
                entry.key.style = style.clone();
                entry.range = range;
                entry.row = block.rows().start;
                entry.control = OperationControl::new();
                entry.state =
                    MermaidPresentation::Loading(t(L10nKey::EditorMarkdownMermaidLoading).into());
                jobs.push(Job {
                    key: entry.key.clone(),
                    control: entry.control.clone(),
                });
                self.entries.push(entry);
            } else {
                self.next_owner += 1;
                self.next_request += 1;
                let key = Key {
                    document,
                    owner: self.next_owner,
                    request: self.next_request,
                    revision,
                    source: block.source().to_owned(),
                    style: style.clone(),
                };
                let control = OperationControl::new();
                jobs.push(Job {
                    key: key.clone(),
                    control: control.clone(),
                });
                self.entries.push(Entry {
                    key,
                    range,
                    row: block.rows().start,
                    state: MermaidPresentation::Loading(
                        t(L10nKey::EditorMarkdownMermaidLoading).into(),
                    ),
                    control,
                });
            }
        }
        for entry in previous {
            entry.control.cancel();
            entry.release_image(cx);
        }
        self.revision = revision;
        jobs
    }
    fn complete(&mut self, key: &Key, result: Result<Rendered, Failure>, cx: &mut App) -> bool {
        let resident: usize = self
            .entries
            .iter()
            .filter_map(|entry| match &entry.state {
                MermaidPresentation::Ready(image, ..) => Some(image_bytes(image)),
                _ => None,
            })
            .sum();
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.key == *key && !entry.control.is_cancelled())
        else {
            return false;
        };
        entry.release_image(cx);
        entry.state = match result {
            Ok(rendered)
                if image_bytes(&rendered.image).saturating_add(resident) <= CACHE_BYTES =>
            {
                MermaidPresentation::Ready(rendered.image, rendered.width, rendered.height)
            }
            Ok(_) => {
                MermaidPresentation::Failed(t(L10nKey::EditorMarkdownMermaidRasterLimit).into())
            }
            Err(error) => MermaidPresentation::Failed(error.message()),
        };
        true
    }
}

struct MermaidHost {
    document: Entity<Document>,
    editor: WeakEntity<EditorState>,
    store: Rc<RefCell<Store>>,
    _subscriptions: Vec<Subscription>,
}
impl MermaidHost {
    fn new(
        document: Entity<Document>,
        editor: WeakEntity<EditorState>,
        store: Rc<RefCell<Store>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let document_watch = cx.subscribe(&document, |this, _, event: &DocumentEvent, cx| {
            this.refresh(Some(event), cx)
        });
        let theme_watch =
            cx.observe_global::<gpui_kit::component::Theme>(|this, cx| this.refresh(None, cx));
        cx.on_release(|this, cx| {
            for entry in &this.store.borrow().entries {
                entry.control.cancel();
                entry.release_image(cx);
            }
        })
        .detach();
        let mut host = Self {
            document,
            editor,
            store,
            _subscriptions: vec![document_watch, theme_watch],
        };
        host.refresh(None, cx);
        host
    }
    fn refresh(&mut self, event: Option<&DocumentEvent>, cx: &mut Context<Self>) {
        let document = self.document.read(cx);
        if event.is_some_and(|event| event.revision() <= self.store.borrow().revision) {
            return;
        }
        if event.is_none()
            && !self.store.borrow().entries.is_empty()
            && self.store.borrow().revision != document.revision()
        {
            let host = cx.weak_entity();
            cx.defer(move |cx| {
                let _ = host.update(cx, |this, cx| this.refresh(None, cx));
            });
            return;
        }
        let (revision, snapshot) = event.map_or_else(
            || (document.revision(), document.snapshot()),
            |event| (event.revision(), event.snapshot().clone()),
        );
        let jobs = self.store.borrow_mut().sync(
            self.document.entity_id(),
            revision,
            snapshot.text(),
            event,
            Style::current(cx),
            cx,
        );
        if jobs.is_empty() {
            return;
        }
        let editor = self.editor.clone();
        cx.defer(move |cx| {
            let _ = editor.update(cx, |_, cx| cx.notify());
        });
        // Actual rendering is never executed in a GPUI render callback or on
        // the UI thread, including the deterministic native consumer platform.
        let failures: Vec<_> = jobs.iter().map(|job| job.key.clone()).collect();
        cx.spawn(async move |this, cx| {
            let results = crate::ui::host_ops::off_thread(move || run_jobs(jobs))
                .await
                .unwrap_or_else(|| {
                    failures
                        .into_iter()
                        .map(|key| {
                            (
                                key,
                                Err(Failure::Diagnostic(
                                    "Native rendering worker stopped".into(),
                                )),
                            )
                        })
                        .collect()
                });
            let _ = this.update(cx, |this, cx| {
                if this.store.borrow().revision != this.document.read(cx).revision() {
                    // Document notifications queued before this completion own
                    // rebasing. Re-enter after they have applied, never publish
                    // against an unobserved revision.
                    let host = cx.weak_entity();
                    cx.defer(move |cx| {
                        let _ = host.update(cx, |this, cx| this.complete(results, cx));
                    });
                } else {
                    this.complete(results, cx);
                }
            });
        })
        .detach();
    }
    fn complete(&mut self, results: Vec<(Key, Result<Rendered, Failure>)>, cx: &mut Context<Self>) {
        if self.store.borrow().revision != self.document.read(cx).revision() {
            return;
        }
        let style = Style::current(cx);
        let mut changed = false;
        for (key, result) in results {
            if key.document == self.document.entity_id() && key.style == style {
                changed |= self.store.borrow_mut().complete(&key, result, cx);
            }
        }
        if changed {
            let _ = self.editor.update(cx, |_, cx| cx.notify());
        }
    }
}

pub(super) fn install(editor: &mut EditorState, cx: &mut Context<EditorState>) {
    let document = editor.document().clone();
    let weak = cx.weak_entity();
    let store = Rc::new(RefCell::new(Store::default()));
    let host = cx.new(|cx| MermaidHost::new(document, weak, store.clone(), cx));
    editor.set_block_mermaid_provider(move |row, source| {
        let _retain_host = &host;
        store.borrow().presentation(row, source)
    });
}

struct Rendered {
    image: Arc<RenderImage>,
    width: f32,
    height: f32,
}
enum Failure {
    Empty,
    External,
    Limit,
    Diagnostic(String),
}
impl Failure {
    fn message(&self) -> SharedString {
        match self {
            Self::Empty => t(L10nKey::EditorMarkdownMermaidEmpty).into(),
            Self::External => t(L10nKey::EditorMarkdownMermaidExternal).into(),
            Self::Limit => t(L10nKey::EditorMarkdownMermaidRasterLimit).into(),
            Self::Diagnostic(message) => {
                format!("{} {}", t(L10nKey::EditorMarkdownMermaidError), message).into()
            }
        }
    }
}
fn image_bytes(image: &RenderImage) -> usize {
    let size = image.size(0);
    (size.width.0 as usize)
        .saturating_mul(size.height.0 as usize)
        .saturating_mul(4)
}
fn run_jobs(jobs: Vec<Job>) -> Vec<(Key, Result<Rendered, Failure>)> {
    let rasterizer = SvgRenderer::new(Arc::new(gpui_kit::assets::Assets));
    let mut remaining = CACHE_BYTES;
    jobs.into_iter()
        .filter_map(|job| {
            if job.control.is_cancelled() {
                return None;
            }
            let result = render_native(&job.key, &job.control, &rasterizer, remaining);
            if let Ok(rendered) = &result {
                remaining = remaining.saturating_sub(image_bytes(&rendered.image));
            }
            #[cfg(feature = "native-markdown-tests")]
            native_tests::hold_completion(&job.key);
            Some((job.key, result))
        })
        .collect()
}
fn render_native(
    key: &Key,
    control: &OperationControl,
    rasterizer: &SvgRenderer,
    remaining: usize,
) -> Result<Rendered, Failure> {
    let renderer = Renderer::new()
        .with_engine(key.style.engine())
        .with_parse_options(ParseOptions::strict())
        .with_resource_policy(InputResourcePolicy::for_profile(
            ResourceProfile::Interactive,
        ));
    let request = SvgRequest {
        environment: SvgEnvironment::deterministic()
            .with_text_measurement_policy(key.style.measurement()),
        pipeline: Some(SvgPipeline::resvg_safe().with_postprocessor(NativeSvgAdmission)),
        ..Default::default()
    };
    let semantic = renderer
        .prepare_semantic(
            &key.source,
            control.child().with_deadline(Duration::from_secs(5)),
        )
        .map_err(render_failure)?
        .ok_or(Failure::Empty)?;
    if semantic.diagram_type() == "error" {
        return Err(Failure::Diagnostic(
            "Merman error diagram is diagnostic output, not a rendered diagram family.".into(),
        ));
    }
    let output = semantic
        .render(RenderTarget::Svg(request))
        .map_err(render_failure)?;
    let RenderOutput::Svg(Some(output)) = output else {
        return Err(Failure::Empty);
    };
    #[cfg(feature = "native-markdown-tests")]
    native_tests::capture_svg(key, output.svg());
    let dimensions =
        resvg::usvg::Tree::from_data(output.svg().as_bytes(), &resvg::usvg::Options::default())
            .map_err(|error| Failure::Diagnostic(error.to_string()))?
            .size();
    let (width, height) = (dimensions.width(), dimensions.height());
    let bytes = (width.ceil() as usize)
        .saturating_mul(height.ceil() as usize)
        .saturating_mul(4);
    if width > 4096.
        || height > 4096.
        || width.ceil() * height.ceil() > 8_000_000.
        || bytes > remaining
    {
        return Err(Failure::Limit);
    }
    if control.is_cancelled() {
        return Err(Failure::Diagnostic("cancelled".into()));
    }
    let parsed = rasterizer
        .parse_svg(output.svg().as_bytes())
        .map_err(|error| Failure::Diagnostic(error.to_string()))?;
    let image = rasterizer
        .render_parsed(
            &parsed,
            SvgSize::ExactSize(size(
                DevicePixels(width.ceil() as i32),
                DevicePixels(height.ceil() as i32),
            )),
        )
        .map_err(|error| Failure::Diagnostic(error.to_string()))?;
    Ok(Rendered {
        image,
        width,
        height,
    })
}

fn render_failure(error: merman::RenderError) -> Failure {
    let message = match error {
        merman::RenderError::Parse(diagnostic) => diagnostic.terminal_safe_message(),
        error => merman::normalize_terminal_diagnostic(&error.to_string()),
    };
    if message.contains("lyrel-external-resource") {
        Failure::External
    } else {
        Failure::Diagnostic(message)
    }
}

struct NativeSvgAdmission;
impl SvgPostprocessor for NativeSvgAdmission {
    fn name(&self) -> &'static str {
        "lyrel-native-svg-admission"
    }
    fn process<'a>(
        &self,
        svg: Cow<'a, str>,
        _: &SvgPostprocessContext<'_>,
    ) -> merman::svg::RenderResult<Cow<'a, str>> {
        let document = resvg::usvg::roxmltree::Document::parse(&svg).map_err(|error| {
            merman::svg::RenderError::svg_postprocess(self.name(), error.to_string())
        })?;
        let external = |value: &str| {
            let value = value.trim();
            !value.is_empty() && !value.starts_with('#') && !value.starts_with("data:image/")
        };
        static URL: LazyLock<regex::Regex> = LazyLock::new(|| {
            regex::Regex::new(r#"(?i)url\(\s*[\"']?([^)'\"]+)"#)
                .expect("static CSS resource expression")
        });
        let css_external = |css: &str| {
            css.to_ascii_lowercase().contains("@import")
                || URL.captures_iter(css).any(|capture| external(&capture[1]))
        };
        let mut quotes = Vec::new();
        for node in document.descendants().filter(|node| node.is_element()) {
            let tag = node.tag_name().name();
            for attribute in node.attributes() {
                if (matches!(attribute.name(), "href" | "src") && external(attribute.value()))
                    || (matches!(
                        attribute.name(),
                        "style"
                            | "fill"
                            | "stroke"
                            | "filter"
                            | "mask"
                            | "clip-path"
                            | "cursor"
                            | "marker-start"
                            | "marker-mid"
                            | "marker-end"
                    ) && css_external(attribute.value()))
                {
                    return Err(merman::svg::RenderError::svg_postprocess(
                        self.name(),
                        "lyrel-external-resource",
                    ));
                }
            }
            if tag == "style" && node.text().is_some_and(css_external) {
                return Err(merman::svg::RenderError::svg_postprocess(
                    self.name(),
                    "lyrel-external-resource",
                ));
            }
            if tag == "style" && !svg[node.range()].contains("<![CDATA[") {
                // Merman 0.9.2's Usecase writer XML-escapes CSS string quotes,
                // but its CSS sanitizer tokenizes the serialized XML text before
                // decoding entities. The entity semicolon then ends font-family
                // prematurely, and final validation sees an unclosed CSS string.
                // Literal quotes are valid XML character data. Restore only the
                // writer's quote references, before the full native-safe preset;
                // never decode attributes, markup, ampersands or active content.
                for text in node.children().filter(|child| child.is_text()) {
                    let range = text.range();
                    for (offset, _) in svg[range.clone()].match_indices('&') {
                        let start = range.start + offset;
                        if svg[start..range.end].starts_with("&quot;") {
                            quotes.push((start..start + "&quot;".len(), '"'));
                        } else if svg[start..range.end].starts_with("&#39;") {
                            quotes.push((start..start + "&#39;".len(), '\''));
                        }
                    }
                }
            }
        }
        if quotes.is_empty() {
            return Ok(svg);
        }
        let mut normalized = String::with_capacity(svg.len());
        let mut cursor = 0;
        for (range, quote) in quotes {
            normalized.push_str(&svg[cursor..range.start]);
            normalized.push(quote);
            cursor = range.end;
        }
        normalized.push_str(&svg[cursor..]);
        Ok(Cow::Owned(normalized))
    }
}

#[cfg(feature = "native-markdown-tests")]
#[path = "mermaid_host_tests.rs"]
mod native_tests;

#[cfg(feature = "native-markdown-tests")]
pub(super) fn native_cases() -> impl Iterator<Item = crate::native_markdown::Case> {
    [
        crate::native_markdown::Case::new(
            "ui::code_editor::markdown::mermaid_host::native_edit_refresh_history_save_reopen",
            native_tests::lifecycle,
        ),
        crate::native_markdown::Case::new(
            "ui::code_editor::markdown::mermaid_host::native_stale_completion",
            native_tests::stale_completion,
        ),
        crate::native_markdown::Case::new(
            "ui::code_editor::markdown::mermaid_host::native_theme_zoom_scroll_geometry",
            native_tests::theme_zoom_scroll,
        ),
    ]
    .into_iter()
}
