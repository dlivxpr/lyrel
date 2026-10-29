use super::projection::{Kind, Leaf, Projection};
use crate::ui::i18n::{L10nKey, t};
use gpui_kit::component::{ActiveTheme as _, button::ButtonVariants as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use merman::{Engine, MermaidConfig, OperationControl, ParseOptions, RenderOutput, RenderRequest, Renderer, SvgRequest};
use merman::resources::{InputResourcePolicy, ResourceProfile};
use merman::svg::{SvgPipeline, SvgPostprocessContext, SvgPostprocessor};
use std::{borrow::Cow, collections::HashMap, ops::Range, sync::{Arc, LazyLock}, time::Duration};

#[derive(Clone, PartialEq)]
pub(super) struct Style {
    family: SharedString,
    size: f32,
    foreground: Hsla,
    background: Hsla,
    border: Hsla,
    dark: bool,
}
impl Style {
    pub(super) fn current(cx: &App) -> Self {
        let theme = cx.theme();
        Self { family: theme.font_family.clone(), size: theme.font_size.into(), foreground: theme.foreground,
            background: theme.background, border: theme.border, dark: theme.is_dark() }
    }
    fn engine(&self) -> Engine {
        let css = |color: Hsla| format!("#{:06x}", u32::from(color.to_rgb()) >> 8);
        Engine::new().with_site_config(MermaidConfig::from_value(serde_json::json!({
            "securityLevel": "strict", "theme": "base", "fontFamily": self.family.as_ref(), "fontSize": self.size,
            "secure": ["secure", "securityLevel", "fontFamily", "fontSize", "theme", "themeVariables", "themeCSS"],
            "themeVariables": { "darkMode": self.dark, "fontFamily": self.family.as_ref(),
                "fontSize": format!("{}px", self.size), "primaryColor": css(self.background),
                "primaryTextColor": css(self.foreground), "primaryBorderColor": css(self.border),
                "lineColor": css(self.foreground), "textColor": css(self.foreground),
                "background": css(self.background) }
        })))
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
    fn drop(&mut self) { self.cancel(); }
}
impl State {
    pub(super) fn cancel(&mut self) {
        if let Some(control) = self.control.take() { control.cancel(); }
        self.pending = None;
        // A previous revision may reserve layout height, never successful status.
        for entry in self.entries.values_mut() { entry.pending = true; entry.error = None; }
    }
    pub(super) fn request(&mut self, projection: &Projection, identity: EntityId, style: Style) {
        self.cancel();
        let control = OperationControl::new();
        let mut requests = Vec::new();
        self.entries.retain(|id, _| projection.leaves.iter().any(|leaf| leaf.id == *id && matches!(leaf.kind, Kind::Mermaid(_))));
        for leaf in &projection.leaves {
            if !matches!(leaf.kind, Kind::Mermaid(_)) { continue; }
            let key = Key { identity, revision: projection.revision, range: leaf.range.clone(),
                source: content(leaf).to_owned(), style: style.clone() };
            let image = self.entries.get(&leaf.id).and_then(|entry| entry.image.clone());
            self.entries.insert(leaf.id, Entry { key: key.clone(), image, error: None, pending: true });
            requests.push((leaf.id, key));
        }
        if !requests.is_empty() {
            self.control = Some(control.clone());
            self.pending = Some(Job { requests, control });
        }
    }
    pub(super) fn take_job(&mut self) -> Option<Job> { self.pending.take() }
    pub(super) fn install(&mut self, batch: Batch, projection: &Projection, identity: EntityId, style: &Style) {
        for (id, key, result) in batch.0 {
            if key.identity != identity || key.revision != projection.revision || &key.style != style { continue; }
            let Some(leaf) = projection.leaves.iter().find(|leaf| leaf.id == id && leaf.range == key.range && matches!(leaf.kind, Kind::Mermaid(_)) && content(leaf) == key.source) else { continue; };
            let Some(entry) = self.entries.get_mut(&leaf.id).filter(|entry| entry.key == key) else { continue; };
            entry.pending = false;
            match result {
                Ok(image) => { entry.image = Some(image); entry.error = None; }
                Err(error) => { entry.error = Some(error); }
            }
        }
    }
    pub(super) fn render(
        &self, leaf: &Leaf, active: bool, source: AnyElement,
        edit: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        cx: &App,
    ) -> AnyElement {
        use gpui_kit::component::button::Button;
        let entry = self.entries.get(&leaf.id).filter(|entry| entry.key.range == leaf.range && entry.key.source == content(leaf));
        let pending = entry.is_none_or(|entry| entry.pending);
        let error = entry.and_then(|entry| entry.error.as_ref());
        let mut block = div().w_full().v_flex().gap_2();
        if let Some(image) = entry.filter(|entry| !entry.pending && entry.error.is_none()).and_then(|entry| entry.image.clone()) {
            let size = image.size(0);
            // These pixels are the bounded raster's intrinsic geometry, not UI spacing.
            block = block.child(img(image).id(format!("markdown-mermaid-image-{}", leaf.range.start)).test_support()
                .max_w_full().w(px(size.width.0 as f32)).h_auto().object_fit(ObjectFit::Contain));
        } else if pending {
            let height = entry.and_then(|entry| entry.image.as_ref()).map(|image| image.size(0).height.0 as f32).unwrap_or(0.);
            block = block.child(div().min_h(px(height)).text_sm().text_color(cx.theme().muted_foreground).child(t(L10nKey::EditorMarkdownMermaidLoading)));
        }
        if let Some(error) = error {
            let (key, diagnostic) = match error {
                Failure::Empty => (L10nKey::EditorMarkdownMermaidEmpty, None),
                Failure::External => (L10nKey::EditorMarkdownMermaidExternal, None),
                Failure::Limit => (L10nKey::EditorMarkdownMermaidRasterLimit, None),
                Failure::Diagnostic(message) => (L10nKey::EditorMarkdownMermaidError, Some(message)),
            };
            block = block.child(div().id(format!("markdown-mermaid-error-{}", leaf.range.start)).test_support()
                .text_sm().text_color(cx.theme().danger).child(t(key)).children(diagnostic.cloned()));
        }
        block = block.child(Button::new(("markdown-mermaid-edit", leaf.id)).ghost().label(t(L10nKey::EditorMarkdownMermaidEdit)).on_click(edit));
        if active || error.is_some() || entry.is_none() {
            block = block.child(div().text_xs().text_color(cx.theme().muted_foreground).child(t(L10nKey::EditorMarkdownMermaidSource))).child(source);
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
        if closing >= start && count >= width && line[count..].trim().is_empty() { end = closing; }
    }
    &leaf.text[start..end]
}

impl Job {
    pub(super) fn run(self) -> Batch {
        let rasterizer = SvgRenderer::new(Arc::new(gpui_kit::assets::Assets));
        let mut batch = Vec::new();
        for (id, key) in self.requests {
            if self.control.is_cancelled() { break; }
            let renderer = Renderer::new().with_engine(key.style.engine())
                .with_parse_options(ParseOptions::strict())
                .with_resource_policy(InputResourcePolicy::for_profile(ResourceProfile::Interactive));
            let request = SvgRequest { pipeline: Some(SvgPipeline::resvg_safe().with_postprocessor(RejectExternalResources)), ..Default::default() };
            let result = (|| {
                let output = renderer.render(RenderRequest::svg(&key.source, self.control.child().with_deadline(Duration::from_secs(5)), request)).map_err(|error| {
                    let message = match error {
                        merman::RenderError::Parse(diagnostic) => diagnostic.terminal_safe_message(),
                        error => merman::normalize_terminal_diagnostic(&error.to_string()),
                    };
                    if message.contains("lyrel-external-resource") { Failure::External }
                    else { Failure::Diagnostic(message) }
                })?;
                let RenderOutput::Svg(Some(output)) = output else { return Err(Failure::Empty); };
                // This parser is used only for preallocation dimensions. GPUI's
                // public renderer below resolves all labels with its own fonts.
                let dimensions = resvg::usvg::Tree::from_data(output.svg().as_bytes(), &resvg::usvg::Options::default())
                    .map_err(|error| Failure::Diagnostic(error.to_string()))?.size();
                let (width, height) = (dimensions.width(), dimensions.height());
                if width > 4096. || height > 4096. || width.ceil() * height.ceil() > 8_000_000. { return Err(Failure::Limit); }
                if self.control.is_cancelled() { return Err(Failure::Diagnostic("cancelled".into())); }
                let parsed = rasterizer.parse_svg(output.svg().as_bytes()).map_err(|error| Failure::Diagnostic(error.to_string()))?;
                rasterizer.render_parsed(&parsed, SvgSize::ExactSize(size(DevicePixels(width.ceil() as i32), DevicePixels(height.ceil() as i32))))
                    .map_err(|error| Failure::Diagnostic(error.to_string()))
            })();
            if self.control.is_cancelled() { break; }
            batch.push((id, key, result));
        }
        Batch(batch)
    }
}

struct RejectExternalResources;
impl SvgPostprocessor for RejectExternalResources {
    fn name(&self) -> &'static str { "lyrel-resource-admission" }
    fn process<'a>(&self, svg: Cow<'a, str>, _: &SvgPostprocessContext<'_>) -> merman::svg::RenderResult<Cow<'a, str>> {
        let document = resvg::usvg::roxmltree::Document::parse(&svg)
            .map_err(|error| merman::svg::RenderError::svg_postprocess(self.name(), error.to_string()))?;
        let external = |value: &str| {
            let value = value.trim();
            !value.is_empty() && !value.starts_with('#') && !value.starts_with("data:image/")
        };
        static URL: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r#"(?i)url\(\s*[\"']?([^)'\"]+)"#).expect("static CSS resource expression"));
        let css_external = |css: &str| css.to_ascii_lowercase().contains("@import") || URL.captures_iter(css).any(|capture| external(&capture[1]));
        for node in document.descendants().filter(|node| node.is_element()) {
            let tag = node.tag_name().name();
            for attribute in node.attributes() {
                if (matches!(attribute.name(), "href" | "src") && external(attribute.value()))
                    || (matches!(attribute.name(), "style" | "fill" | "stroke" | "filter" | "mask" | "clip-path" | "cursor" | "marker-start" | "marker-mid" | "marker-end") && css_external(attribute.value())) {
                    return Err(merman::svg::RenderError::svg_postprocess(self.name(), "lyrel-external-resource"));
                }
            }
            if tag == "style" && node.text().is_some_and(css_external) {
                return Err(merman::svg::RenderError::svg_postprocess(self.name(), "lyrel-external-resource"));
            }
        }
        Ok(svg)
    }
}
