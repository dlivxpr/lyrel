use super::projection::{Kind, Leaf, Projection};
use crate::ui::i18n::{L10nKey, t};
use gpui_kit::base::StyledExt as _;
use gpui_kit::component::{ActiveTheme as _, button::ButtonVariants as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
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
    collections::HashMap,
    ops::Range,
    sync::{Arc, LazyLock},
    time::Duration,
};

#[derive(Clone)]
pub(super) struct Style {
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
    pub(super) fn current(cx: &App) -> Self {
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
        let text_system = WindowTextSystem::new(self.text_system.clone());
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
            format!("gpui-kit-0.8.1:{}", self.family),
        )
        .expect("nonempty native measurement version");
        TextMeasurementPolicy::uniform(TextMeasurementProfile::new(identity, measurer))
    }
}
#[derive(Clone, PartialEq)]
struct Key {
    identity: EntityId,
    revision: u64,
    range: Range<usize>,
    source: String,
    style: Style,
}
struct Entry {
    key: Key,
    image: Option<Arc<RenderImage>>,
    error: Option<Failure>,
    pending: bool,
}
pub(super) enum Failure {
    Empty,
    External,
    Limit,
    Diagnostic(String),
}
pub(super) struct Job {
    requests: Vec<(u64, Key)>,
    control: OperationControl,
}
pub(super) struct Batch(Vec<(u64, Key, Result<Arc<RenderImage>, Failure>)>);
#[derive(Default)]
pub(super) struct State {
    entries: HashMap<u64, Entry>,
    control: Option<OperationControl>,
    pending: Option<Job>,
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl State {
    pub(super) fn cancel(&mut self) {
        if let Some(control) = self.control.take() {
            control.cancel();
        }
        self.pending = None;
        // A previous revision may reserve layout height, never successful status.
        for entry in self.entries.values_mut() {
            entry.pending = true;
            entry.error = None;
        }
    }
    pub(super) fn request(&mut self, projection: &Projection, identity: EntityId, style: Style) {
        self.cancel();
        let control = OperationControl::new();
        let mut requests = Vec::new();
        self.entries.retain(|id, _| {
            projection
                .leaves
                .iter()
                .any(|leaf| leaf.id == *id && matches!(leaf.kind, Kind::Mermaid(_)))
        });
        for leaf in &projection.leaves {
            if !matches!(leaf.kind, Kind::Mermaid(_)) {
                continue;
            }
            let key = Key {
                identity,
                revision: projection.revision,
                range: leaf.range.clone(),
                source: content(leaf).to_owned(),
                style: style.clone(),
            };
            let image = self
                .entries
                .get(&leaf.id)
                .and_then(|entry| entry.image.clone());
            self.entries.insert(
                leaf.id,
                Entry {
                    key: key.clone(),
                    image,
                    error: None,
                    pending: true,
                },
            );
            requests.push((leaf.id, key));
        }
        if !requests.is_empty() {
            self.control = Some(control.clone());
            self.pending = Some(Job { requests, control });
        }
    }
    pub(super) fn take_job(&mut self) -> Option<Job> {
        self.pending.take()
    }
    pub(super) fn install(
        &mut self,
        batch: Batch,
        projection: &Projection,
        identity: EntityId,
        style: &Style,
    ) {
        for (id, key, result) in batch.0 {
            if key.identity != identity
                || key.revision != projection.revision
                || &key.style != style
            {
                continue;
            }
            let Some(leaf) = projection.leaves.iter().find(|leaf| {
                leaf.id == id
                    && leaf.range == key.range
                    && matches!(leaf.kind, Kind::Mermaid(_))
                    && content(leaf) == key.source
            }) else {
                continue;
            };
            let Some(entry) = self
                .entries
                .get_mut(&leaf.id)
                .filter(|entry| entry.key == key)
            else {
                continue;
            };
            entry.pending = false;
            match result {
                Ok(image) => {
                    entry.image = Some(image);
                    entry.error = None;
                }
                Err(error) => {
                    entry.error = Some(error);
                }
            }
        }
    }
    pub(super) fn render(
        &self,
        leaf: &Leaf,
        active: bool,
        source: AnyElement,
        edit: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        cx: &App,
    ) -> AnyElement {
        use gpui_kit::component::button::Button;
        let entry = self
            .entries
            .get(&leaf.id)
            .filter(|entry| entry.key.range == leaf.range && entry.key.source == content(leaf));
        let pending = entry.is_none_or(|entry| entry.pending);
        let error = entry.and_then(|entry| entry.error.as_ref());
        let mut block = div().w_full().v_flex().gap_2();
        if let Some(image) = entry
            .filter(|entry| !entry.pending && entry.error.is_none())
            .and_then(|entry| entry.image.clone())
        {
            let size = image.size(0);
            // These pixels are the bounded raster's intrinsic geometry, not UI spacing.
            block = block.child(
                img(image)
                    .object_fit(ObjectFit::Contain)
                    .id(format!("markdown-mermaid-image-{}", leaf.range.start))
                    .test_support()
                    .max_w_full()
                    .w(px(size.width.0 as f32))
                    .h_auto(),
            );
        } else if pending {
            let height = entry
                .and_then(|entry| entry.image.as_ref())
                .map(|image| image.size(0).height.0 as f32)
                .unwrap_or(0.);
            block = block.child(
                div()
                    .min_h(px(height))
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(L10nKey::EditorMarkdownMermaidLoading)),
            );
        }
        if let Some(error) = error {
            let (key, diagnostic) = match error {
                Failure::Empty => (L10nKey::EditorMarkdownMermaidEmpty, None),
                Failure::External => (L10nKey::EditorMarkdownMermaidExternal, None),
                Failure::Limit => (L10nKey::EditorMarkdownMermaidRasterLimit, None),
                Failure::Diagnostic(message) => {
                    (L10nKey::EditorMarkdownMermaidError, Some(message))
                }
            };
            block = block.child(
                div()
                    .id(format!("markdown-mermaid-error-{}", leaf.range.start))
                    .role(Role::Alert)
                    .aria_label(diagnostic.map_or_else(|| t(key).to_owned(), |message| format!("{} {message}", t(key))))
                    .test_support()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(t(key))
                    .children(diagnostic.cloned()),
            );
        }
        block = block.child(
            div()
                // Do not let the source surface move the caret/reflow the
                // button between its press and click. Source text still bubbles.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Button::new(("markdown-mermaid-edit", leaf.id))
                        .ghost()
                        .label(t(L10nKey::EditorMarkdownMermaidEdit))
                        .on_click(edit),
                ),
        );
        if active || error.is_some() || entry.is_none() {
            block = block
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(t(L10nKey::EditorMarkdownMermaidSource)),
                )
                .child(source);
        }
        block.into_any_element()
    }
}

// The AST owns this fence's byte range. Only remove its two fence lines from
// the render input; no reconstructed Mermaid text is ever written to source.
fn content(leaf: &Leaf) -> &str {
    let start = leaf.text.find('\n').map_or(leaf.text.len(), |at| at + 1);
    let mut end = leaf.text.len();
    if let Kind::Mermaid(super::regions::Code::Fenced { marker, width }) = leaf.kind {
        let closing = leaf.text.rfind('\n').map_or(0, |at| at + 1);
        let line = leaf.text[closing..].trim();
        let count = line.bytes().take_while(|byte| *byte == marker).count();
        if closing >= start && count >= width && line[count..].trim().is_empty() {
            end = closing;
        }
    }
    &leaf.text[start..end]
}

impl Job {
    pub(super) fn run(self) -> Batch {
        let rasterizer = SvgRenderer::new(Arc::new(gpui_kit::assets::Assets));
        let mut batch = Vec::new();
        for (id, key) in self.requests {
            if self.control.is_cancelled() {
                break;
            }
            let renderer = Renderer::new()
                .with_engine(key.style.engine())
                .with_parse_options(ParseOptions::strict())
                .with_resource_policy(InputResourcePolicy::for_profile(
                    ResourceProfile::Interactive,
                ));
            let request = SvgRequest {
                environment: SvgEnvironment::deterministic()
                    .with_text_measurement_policy(key.style.measurement()),
                pipeline: Some(
                    SvgPipeline::resvg_safe().with_postprocessor(NativeSvgAdmission),
                ),
                ..Default::default()
            };
            let result = (|| {
                let semantic = renderer
                    .prepare_semantic(
                        &key.source,
                        self.control.child().with_deadline(Duration::from_secs(5)),
                    )
                    .map_err(render_failure)?
                    .ok_or(Failure::Empty)?;
                // `error` is a released primary boundary, not a supported
                // diagram. Never publish its error-picture as successful art.
                if semantic.diagram_type() == "error" {
                    return Err(Failure::Diagnostic(
                        "Merman error diagram is diagnostic output, not a rendered diagram family."
                            .into(),
                    ));
                }
                let output = semantic
                    .render(RenderTarget::Svg(request))
                    .map_err(render_failure)?;
                let RenderOutput::Svg(Some(output)) = output else {
                    return Err(Failure::Empty);
                };
                // This parser is used only for preallocation dimensions. GPUI's
                // public renderer below resolves all labels with its own fonts.
                let dimensions = resvg::usvg::Tree::from_data(
                    output.svg().as_bytes(),
                    &resvg::usvg::Options::default(),
                )
                .map_err(|error| Failure::Diagnostic(error.to_string()))?
                .size();
                let (width, height) = (dimensions.width(), dimensions.height());
                if width > 4096. || height > 4096. || width.ceil() * height.ceil() > 8_000_000. {
                    return Err(Failure::Limit);
                }
                if self.control.is_cancelled() {
                    return Err(Failure::Diagnostic("cancelled".into()));
                }
                let parsed = rasterizer
                    .parse_svg(output.svg().as_bytes())
                    .map_err(|error| Failure::Diagnostic(error.to_string()))?;
                rasterizer
                    .render_parsed(
                        &parsed,
                        SvgSize::ExactSize(size(
                            DevicePixels(width.ceil() as i32),
                            DevicePixels(height.ceil() as i32),
                        )),
                    )
                    .map_err(|error| Failure::Diagnostic(error.to_string()))
            })();
            if self.control.is_cancelled() {
                break;
            }
            batch.push((id, key, result));
        }
        Batch(batch)
    }
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
        if quotes.is_empty() { return Ok(svg); }
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
