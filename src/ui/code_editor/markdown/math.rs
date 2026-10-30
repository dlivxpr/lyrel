use crate::ui::i18n::L10nKey;
use gpui_kit::TestSupportExt as _;
use gpui_kit::base::text::editor_host::InlineNode;
use gpui_kit::base::text::{
    InlineElement, InlineRenderContext, MarkdownExtensions, MarkdownNode, MarkdownParseContext,
    MarkdownPlugin,
};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::*;
use gpui_kit::*;
use markdown::mdast::Node;
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Formula {
    pub tex: String,
    pub block: bool,
    raw: String,
}
impl Formula {
    pub(super) fn insertion_offset(&self) -> usize {
        let opening = self.raw.bytes().take_while(|byte| *byte == b'$').count();
        let start = if self.block {
            self.raw.find('\n').map_or(opening, |at| at + 1)
        } else {
            opening
        };
        let end = if self.block {
            self.raw
                .rsplit_once('\n')
                .and_then(|(before, last)| {
                    let closing = last.trim();
                    (closing.len() >= opening && closing.bytes().all(|byte| byte == b'$'))
                        .then_some(before.len())
                })
                .unwrap_or(self.raw.len())
                .max(start)
        } else {
            self.raw.trim_end_matches('$').len().max(start)
        };
        let body = &self.raw[start..end];
        start + body.len() - body.trim_start_matches(char::is_whitespace).len()
    }
    pub fn from_node(node: &Node, raw: &str) -> Option<Self> {
        match node {
            Node::InlineMath(value) => Some(Self {
                tex: value.value.clone(),
                block: false,
                raw: raw.to_owned(),
            }),
            Node::Math(value) => Some(Self {
                tex: value.value.clone(),
                block: true,
                raw: raw.to_owned(),
            }),
            _ => None,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Style {
    rgba: [u32; 4],
    size: u32,
}
impl Style {
    pub fn new(color: Hsla, size: Pixels) -> Self {
        let color = color.to_rgb();
        Self {
            rgba: [
                color.r.to_bits(),
                color.g.to_bits(),
                color.b.to_bits(),
                color.a.to_bits(),
            ],
            size: f32::from(size).to_bits(),
        }
    }
}
#[derive(Clone)]
pub(super) enum Output {
    Ready {
        image: Arc<RenderImage>,
        width: f32,
        height: f32,
        baseline: f32,
    },
    Failed(String),
}
#[derive(Default)]
pub(super) struct State {
    pub style: Option<Style>,
    pub outputs: HashMap<Formula, Output>,
    pub task: Option<Task<()>>,
    pub running: bool,
}
impl State {
    pub fn inline_node(&self, formula: &Formula) -> InlineNode {
        InlineNode::custom(
            MarkdownNode::new(
                "lyrel-math",
                (formula.clone(), self.outputs.get(formula).cloned()),
            )
            .text("\u{fffc}")
            .accessibility_label(crate::ui::i18n::t_fmt(
                L10nKey::EditorMarkdownFormulaLabel,
                &[("tex", &formula.tex)],
            )),
        )
    }
}

pub(super) fn extensions() -> Arc<MarkdownExtensions> {
    static EXTENSIONS: std::sync::LazyLock<Arc<MarkdownExtensions>> =
        std::sync::LazyLock::new(|| Arc::new(MarkdownExtensions::default().plugin(MathPlugin)));
    EXTENSIONS.clone()
}
struct MathPlugin;
impl MarkdownPlugin for MathPlugin {
    fn name(&self) -> &str {
        "lyrel-math"
    }
    fn parse(&self, node: &Node, context: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        Formula::from_node(node, context.node_source(node)?).map(|formula| {
            MarkdownNode::new("lyrel-math", (formula, None::<Output>)).text("\u{fffc}")
        })
    }
    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<InlineElement> {
        let (formula, output) = node.data::<(Formula, Option<Output>)>()?;
        let element = match output {
            Some(Output::Ready {
                image,
                width,
                height,
                baseline,
            }) => {
                let font = f32::from(context.font_size());
                return Some(
                    InlineElement::new(
                        div()
                            .id(SharedString::from(format!(
                                "markdown-math-ready-{}",
                                formula.tex
                            )))
                            .test_support()
                            .role(Role::Image)
                            .aria_label(crate::ui::i18n::t_fmt(
                                L10nKey::EditorMarkdownFormulaLabel,
                                &[("tex", &formula.tex)],
                            ))
                            .child(img(image.clone()).w(px(width * font)).h(px(height * font))),
                    )
                    .with_baseline(px(baseline * font)),
                );
            }
            Some(Output::Failed(error)) => div()
                .id(SharedString::from(format!(
                    "markdown-math-error-{}",
                    formula.tex
                )))
                .test_support()
                .aria_label(format!(
                    "{} {error}",
                    crate::ui::i18n::t(crate::ui::i18n::L10nKey::EditorMarkdownMathError)
                ))
                .text_color(cx.theme().danger)
                .child(format!(
                    "{} {error}",
                    crate::ui::i18n::t(crate::ui::i18n::L10nKey::EditorMarkdownMathError)
                ))
                .into_any_element(),
            None => div()
                .text_color(cx.theme().muted_foreground)
                .child(crate::ui::i18n::t(
                    crate::ui::i18n::L10nKey::EditorMarkdownMathLoading,
                ))
                .into_any_element(),
        };
        Some(InlineElement::new(element))
    }
}

/// This pure computation runs only on the background executor. RaTeX emits
/// embedded glyph outlines; resvg consumes those outlines without OS fonts or I/O.
pub(super) fn render(formula: &Formula, style: Style) -> Output {
    let result = (|| -> anyhow::Result<Output> {
        let nodes =
            ratex_parser::parse(&formula.tex).map_err(|error| anyhow::anyhow!("{error}"))?;
        let mut options = ratex_layout::LayoutOptions::default();
        if !formula.block {
            options.style = options.style.numerator();
        }
        options.color.r = f32::from_bits(style.rgba[0]);
        options.color.g = f32::from_bits(style.rgba[1]);
        options.color.b = f32::from_bits(style.rgba[2]);
        options.color.a = f32::from_bits(style.rgba[3]);
        let layout = ratex_layout::layout(&nodes, &options);
        let list = ratex_layout::to_display_list(&layout);
        let font = f32::from_bits(style.size).max(1.) * 2.;
        let svg = ratex_svg::render_to_svg_with_color_syntax(
            &list,
            &ratex_svg::SvgOptions {
                font_size: font as f64,
                padding: 0.,
                embed_glyphs: true,
                ..Default::default()
            },
            ratex_svg::SvgColorSyntax::Rgb,
        );
        anyhow::ensure!(
            !svg.contains("<text") && !svg.contains("<image") && !svg.contains("<foreignObject"),
            "Formula requires unsupported fonts or external content"
        );
        let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default())?;
        let size = tree.size().to_int_size();
        anyhow::ensure!(
            size.width() <= 16384
                && size.height() <= 16384
                && u64::from(size.width()) * u64::from(size.height()) <= 16_777_216,
            "Formula is too large; split it into smaller formulas"
        );
        let mut pixels = resvg::tiny_skia::Pixmap::new(size.width(), size.height())
            .ok_or_else(|| anyhow::anyhow!("Formula image allocation failed"))?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::default(),
            &mut pixels.as_mut(),
        );
        let mut bytes = pixels.take();
        for pixel in bytes.chunks_exact_mut(4) {
            let alpha = pixel[3] as u32;
            if alpha != 0 {
                for channel in &mut pixel[..3] {
                    *channel = ((*channel as u32 * 255) / alpha).min(255) as u8;
                }
            }
            pixel.swap(0, 2);
        }
        let image = image::RgbaImage::from_raw(size.width(), size.height(), bytes)
            .ok_or_else(|| anyhow::anyhow!("Invalid formula image"))?;
        Ok(Output::Ready {
            image: Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
                image
            )])),
            width: tree.size().width() / font,
            height: tree.size().height() / font,
            baseline: list.height as f32,
        })
    })();
    result.unwrap_or_else(|error| Output::Failed(error.to_string()))
}
