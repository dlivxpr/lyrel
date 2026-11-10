//! Formula workflow owned by Lyrel, not by the upstream GPL application.
use crate::ui::{
    host_ops::HostOps,
    i18n::{L10nKey, t},
};
use gpui_kit::component::{ActiveTheme as _, native_menu::NativeMenu};
use gpui_kit::*;
use lyrel_markdown_editor::math::editor::view::{
    AlignFormulaCenter, AlignFormulaLeft, AlignFormulaRight, CancelFormula, ConfirmFormula,
};
use lyrel_markdown_editor::math::{MathAlign, MathEditor, MathNav, MathTheme, render};
use lyrel_markdown_editor::{DocumentEvent, EditorEvent, EditorState, MathAlign as DocumentAlign};
use std::{cell::RefCell, collections::HashMap, ops::Range, rc::Rc, sync::Arc};

type Bitmap = (Arc<RenderImage>, f32, f32);
type FormulaCache = Rc<RefCell<HashMap<String, Bitmap>>>;

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = lyrel_formula, no_json)]
struct CopyFormula {
    latex: String,
}
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = lyrel_formula, no_json)]
struct ExportFormula {
    latex: String,
    svg: bool,
}
struct FormulaActions;
impl Global for FormulaActions {}

#[derive(Clone, PartialEq)]
struct RenderKey {
    revision: u64,
    em: f32,
    dpr: f32,
    color: Hsla,
}
struct FormulaSession {
    editor: Entity<MathEditor>,
    revision: u64,
    document: EntityId,
    range: Range<usize>,
    original: String,
    body: Range<usize>,
    selection: Range<usize>,
    inline: bool,
    align: MathAlign,
    _subscriptions: Vec<Subscription>,
}
struct FormulaHost {
    presentation: WeakEntity<EditorState>,
    cache: FormulaCache,
    render_key: Option<RenderKey>,
    pending_render: Option<(RenderKey, Arc<String>)>,
    rendering: bool,
    active: Option<FormulaSession>,
    _subscriptions: Vec<Subscription>,
}

/// Install once per presentation. The presentation retains its host until its own release;
/// the host only keeps a weak presentation, so closing a split drops tasks and the seat.
pub(super) fn install(presentation: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
    if !cx.has_global::<FormulaActions>() {
        cx.set_global(FormulaActions);
        cx.on_action(|action: &CopyFormula, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(action.latex.clone()))
        });
        cx.on_action(|action: &ExportFormula, cx| export(action.clone(), cx));
    }
    let host = cx.new(|cx| FormulaHost::new(presentation, window, cx));
    presentation.update(cx, |_, cx| {
        cx.on_release(move |_, cx| {
            let cache = std::mem::take(&mut *host.read(cx).cache.borrow_mut());
            for (_, (image, _, _)) in cache {
                cx.drop_image(image, None);
            }
            drop(host);
        })
        .detach()
    });
}

impl FormulaHost {
    fn new(
        presentation: &Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let cache = FormulaCache::default();
        let provider = cache.clone();
        presentation.update(cx, |view, _| {
            view.set_block_math_provider(move |latex| provider.borrow().get(latex).cloned())
        });
        let document = presentation.read(cx).document().clone();
        let subscriptions = vec![
            cx.subscribe_in(
                presentation,
                window,
                |this, _, event, window, cx| match event {
                    EditorEvent::EditMath {
                        range,
                        source,
                        at_end,
                        inline,
                    } => this.open(range.clone(), source, *at_end, *inline, window, cx),
                    EditorEvent::MathMenu { source, position } => {
                        formula_menu(source.to_string(), *position, false, false, window, cx)
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &document,
                window,
                |this, _, _: &DocumentEvent, window, cx| this.refresh(window, cx),
            ),
            cx.observe_in(presentation, window, |this, _, window, cx| {
                this.refresh(window, cx)
            }),
            cx.observe_global_in::<gpui_kit::component::Theme>(window, |this, window, cx| {
                this.refresh(window, cx)
            }),
            cx.observe_window_bounds(window, |this, window, cx| this.refresh(window, cx)),
        ];
        let mut this = Self {
            presentation: presentation.downgrade(),
            cache,
            render_key: None,
            pending_render: None,
            rendering: false,
            active: None,
            _subscriptions: subscriptions,
        };
        this.refresh(window, cx);
        this
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.presentation.upgrade() else {
            return;
        };
        let state = view.read(cx);
        let document = state.document().read(cx);
        let key = RenderKey {
            revision: document.revision(),
            em: f32::from(cx.theme().font_size) * 1.5,
            dpr: window.scale_factor().max(1.),
            color: cx.theme().foreground,
        };
        if self.active.as_ref().is_some_and(|session| {
            session.document != state.document().entity_id()
                || session.revision != key.revision
                || session.selection != state.selected_range()
                || !state.is_live()
        }) {
            self.dismiss(false, window, cx);
            report(L10nKey::EditorFormulaChanged, window, cx);
        }
        if self.render_key.as_ref() == Some(&key) {
            return;
        }
        let source = view.read(cx).document().read(cx).snapshot().shared_text();
        self.render_key = Some(key.clone());
        let old = std::mem::take(&mut *self.cache.borrow_mut());
        for (_, (image, _, _)) in old {
            cx.drop_image(image, Some(window));
        }
        if let Some(session) = &self.active {
            let theme = cx.theme();
            let theme = MathTheme {
                fg: theme.foreground,
                muted: theme.muted_foreground,
                panel: theme.popover,
                border: theme.border,
                accent: theme.primary,
                accent_bg: theme.list_hover,
            };
            session.editor.update(cx, |editor, cx| {
                editor.set_presentation(
                    key.em / if session.inline { 1.5 } else { 1. },
                    key.dpr,
                    theme,
                    window,
                    cx,
                )
            });
        }
        view.update(cx, |view, cx| {
            view.set_block_math_em(key.em);
            view.refresh_formula_layout(cx);
        });
        self.pending_render = Some((key, source));
        self.render_pending(window, cx);
    }

    fn render_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.rendering {
            return;
        }
        let Some((key, source)) = self.pending_render.take() else {
            return;
        };
        self.rendering = true;
        let request = key.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let mut formulas = lyrel_markdown_editor::math_sources(&source);
                    formulas.extend(lyrel_markdown_editor::inline_math_sources(&source));
                    formulas.sort();
                    formulas.dedup();
                    let mut cache = HashMap::new();
                    let mut failed = false;
                    for latex in formulas {
                        match render::render_latex(&latex, request.em, request.dpr, request.color) {
                            Some(image) => {
                                cache.insert(
                                    latex.to_string(),
                                    (image.image, image.width, image.height),
                                );
                            }
                            None => failed = true,
                        }
                    }
                    (cache, failed)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.rendering = false;
                if this.render_key.as_ref() == Some(&key)
                    && let Some(view) = this.presentation.upgrade()
                    && view.read(cx).document().read(cx).revision() == key.revision
                {
                    *this.cache.borrow_mut() = result.0;
                    if result.1 {
                        report(L10nKey::EditorFormulaRenderFailed, window, cx);
                    }
                    view.update(cx, |view, cx| view.refresh_formula_layout(cx));
                }
                this.render_pending(window, cx);
            });
        })
        .detach();
    }

    fn open(
        &mut self,
        range: Range<usize>,
        latex: &str,
        at_end: bool,
        inline: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.presentation.upgrade() else {
            return;
        };
        let state = view.read(cx);
        if !state.is_live() || state.document().read(cx).has_composition() {
            return;
        }
        let Some(original) = state.text().get(range.clone()).map(str::to_owned) else {
            return;
        };
        // Capture exact delimiters/whitespace once. Commit replaces the body only, not the fences.
        let delimiter = if original.starts_with("$$") {
            "$$"
        } else {
            "$"
        };
        let Some(inner) = original
            .strip_prefix(delimiter)
            .and_then(|s| s.strip_suffix(delimiter))
        else {
            report(L10nKey::EditorFormulaChanged, window, cx);
            return;
        };
        if inner.trim() != latex.trim() {
            report(L10nKey::EditorFormulaChanged, window, cx);
            return;
        }
        let body = if inner.trim().is_empty() {
            let at = delimiter.len() + usize::from(inner.starts_with('\n'));
            at..at
        } else {
            let start = delimiter.len() + inner.len() - inner.trim_start().len();
            start..delimiter.len() + inner.trim_end().len()
        };
        let row = match lyrel_markdown_editor::math::editor::latex::parse_latex_checked(latex) {
            Ok(row) => row,
            Err(_) => {
                report(L10nKey::EditorFormulaUnsupported, window, cx);
                return;
            }
        };
        let revision = state.document().read(cx).revision();
        let document = state.document().entity_id();
        let selection = state.selected_range();
        let align = if inline {
            MathAlign::Left
        } else {
            map_align(state.math_align(range.start))
        };
        let font = f32::from(cx.theme().font_size) * if inline { 1. } else { 1.5 };
        let theme = cx.theme();
        let theme = MathTheme {
            fg: theme.foreground,
            muted: theme.muted_foreground,
            panel: theme.popover,
            border: theme.border,
            accent: theme.primary,
            accent_bg: theme.list_hover,
        };
        self.dismiss(false, window, cx);
        let editor = cx.new(|cx| MathEditor::from_row(row, font, at_end, align, theme, cx));
        editor.update(cx, |editor, _| {
            editor.set_compact(inline);
            editor.set_action_labels(
                t(L10nKey::EditorFormulaCommit).into(),
                t(L10nKey::EditorFormulaCancel).into(),
                [
                    t(L10nKey::EditorFormulaAddRow).into(),
                    t(L10nKey::EditorFormulaRemoveRow).into(),
                    t(L10nKey::EditorFormulaAddColumn).into(),
                    t(L10nKey::EditorFormulaRemoveColumn).into(),
                ],
            );
        });
        let subscriptions = vec![
            cx.subscribe_in(&editor, window, |this, editor, nav, window, cx| {
                if this
                    .active
                    .as_ref()
                    .is_none_or(|session| session.editor != *editor)
                {
                    return;
                }
                match nav {
                    MathNav::Exit { after } => {
                        this.commit(*after, window, cx);
                    }
                    MathNav::Save => {
                        if this.commit(true, window, cx) {
                            window.dispatch_action(Box::new(crate::core::actions::EditorSave), cx);
                        }
                    }
                    MathNav::Cancel => this.dismiss(true, window, cx),
                    MathNav::ContextMenu { position } => formula_menu(
                        editor.read(cx).to_latex(),
                        *position,
                        true,
                        this.active.as_ref().is_some_and(|session| !session.inline),
                        window,
                        cx,
                    ),
                    MathNav::RenderFailed => report(L10nKey::EditorFormulaRenderFailed, window, cx),
                }
            }),
            cx.observe_in(&editor, window, |this, _, window, cx| this.seat(window, cx)),
        ];
        self.active = Some(FormulaSession {
            editor: editor.clone(),
            revision,
            document,
            range,
            original,
            body,
            selection,
            inline,
            align,
            _subscriptions: subscriptions,
        });
        self.seat(window, cx);
        editor.read(cx).focus_handle().focus(window, cx);
    }

    fn seat(&self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.active else {
            return;
        };
        let Some(view) = self.presentation.upgrade() else {
            return;
        };
        let height = session.editor.read(cx).logical_size().1;
        view.update(cx, |view, cx| {
            if session.inline {
                // The display provider is downscaled from display em to body em; its padding scales too.
                let offset = -render::PAD / 3.;
                view.set_editing_inline(
                    session.range.clone(),
                    session.editor.clone().into(),
                    point(px(offset), px(offset)),
                    cx,
                );
            } else {
                view.set_editing_block(
                    session.range.clone(),
                    session.editor.clone().into(),
                    px(height),
                    cx,
                );
            }
        });
    }

    fn commit(&mut self, after: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.active.as_ref() else {
            return false;
        };
        let Some(view) = self.presentation.upgrade() else {
            return false;
        };
        let state = view.read(cx);
        if !state.is_live()
            || state.document().entity_id() != session.document
            || state.document().read(cx).revision() != session.revision
            || state.selected_range() != session.selection
            || state.text().get(session.range.clone()) != Some(session.original.as_str())
        {
            self.dismiss(false, window, cx);
            report(L10nKey::EditorFormulaChanged, window, cx);
            return false;
        }
        let editor = session.editor.read(cx);
        if !editor.is_rendered() || editor.has_pending_input() {
            report(L10nKey::EditorFormulaRenderFailed, window, cx);
            return false;
        }
        let latex = editor.to_latex();
        if lyrel_markdown_editor::math::editor::latex::parse_latex_checked(&latex).is_err() {
            report(L10nKey::EditorFormulaUnsupported, window, cx);
            return false;
        }
        let changed = editor.is_modified();
        let align = editor.align();
        let session = self.active.take().unwrap();
        view.update(cx, |view, cx| {
            view.end_editing_inline(cx);
            view.end_editing_block(cx);
            let mut end = session.range.end;
            let mut start = session.range.start;
            let mut edits = Vec::new();
            if changed {
                let body = session.range.start + session.body.start
                    ..session.range.start + session.body.end;
                end = end - body.len() + latex.len();
                edits.push((body, latex));
            }
            if !session.inline && align != session.align {
                let (extended, prefix) =
                    view.math_marker_edit(session.range.clone(), unmap_align(align));
                let marker = extended.start..session.range.start;
                end = end - marker.len() + prefix.len();
                start = extended.start;
                edits.push((marker, prefix));
            }
            if !edits.is_empty() {
                view.replace_ranges(edits, cx);
            }
            let caret = if after { end } else { start };
            view.select_range(caret..caret, cx);
            view.focus_handle(cx).focus(window, cx);
        });
        true
    }

    fn dismiss(&mut self, restore_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.active.take() else {
            return;
        };
        let Some(view) = self.presentation.upgrade() else {
            return;
        };
        view.update(cx, |view, cx| {
            view.end_editing_inline(cx);
            view.end_editing_block(cx);
            if restore_focus {
                view.select_range(session.selection, cx);
                view.focus_handle(cx).focus(window, cx);
            }
        });
    }
}

fn map_align(align: DocumentAlign) -> MathAlign {
    match align {
        DocumentAlign::Left => MathAlign::Left,
        DocumentAlign::Center => MathAlign::Center,
        DocumentAlign::Right => MathAlign::Right,
    }
}
fn unmap_align(align: MathAlign) -> DocumentAlign {
    match align {
        MathAlign::Left => DocumentAlign::Left,
        MathAlign::Center => DocumentAlign::Center,
        MathAlign::Right => DocumentAlign::Right,
    }
}
fn report(key: L10nKey, window: &mut Window, cx: &mut App) {
    HostOps::notify_err(
        window,
        cx,
        &t(L10nKey::EditorFormulaEditing),
        &std::io::Error::other(t(key)),
    );
}
fn formula_menu(
    latex: String,
    position: Point<Pixels>,
    editing: bool,
    alignment: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let mut menu = NativeMenu::new()
        .menu(
            t(L10nKey::EditorFormulaCopyLatex),
            Box::new(CopyFormula {
                latex: latex.clone(),
            }),
        )
        .menu(
            t(L10nKey::EditorFormulaExportSvg),
            Box::new(ExportFormula {
                latex: latex.clone(),
                svg: true,
            }),
        )
        .menu(
            t(L10nKey::EditorFormulaExportPng),
            Box::new(ExportFormula { latex, svg: false }),
        );
    if alignment {
        menu = menu
            .separator()
            .menu(
                t(L10nKey::EditorFormulaAlignLeft),
                Box::new(AlignFormulaLeft),
            )
            .menu(
                t(L10nKey::EditorFormulaAlignCenter),
                Box::new(AlignFormulaCenter),
            )
            .menu(
                t(L10nKey::EditorFormulaAlignRight),
                Box::new(AlignFormulaRight),
            );
    }
    if editing {
        menu = menu
            .separator()
            .menu(t(L10nKey::EditorFormulaCommit), Box::new(ConfirmFormula))
            .menu(t(L10nKey::EditorFormulaCancel), Box::new(CancelFormula));
    }
    menu.show(position, window, cx);
}

fn export(action: ExportFormula, cx: &mut App) {
    // This is explicitly a local export, not a resource write to the document's Host.
    let answer = cx.prompt_for_new_path(
        std::path::Path::new("."),
        Some(if action.svg {
            "formula.svg"
        } else {
            "formula.png"
        }),
    );
    let window = cx.active_window();
    cx.spawn(async move |cx| {
        let result = match answer.await {
            Ok(Ok(Some(path))) => {
                cx.background_spawn(async move {
                    let bytes = if action.svg {
                        render::render_latex_to_svg(&action.latex, 32.).map(String::into_bytes)
                    } else {
                        render::render_latex_to_png(&action.latex, 32., 2.)
                    };
                    std::fs::write(
                        path,
                        bytes.ok_or_else(|| std::io::Error::other("formula render failed"))?,
                    )
                })
                .await
            }
            Ok(Ok(None)) => return,
            _ => Err(std::io::Error::other("formula export path unavailable")),
        };
        if let Err(error) = result {
            if let Some(window) = window {
                let _ = window.update(cx, |_, window, cx| {
                    HostOps::notify_err(window, cx, &t(L10nKey::EditorFormulaExportFailed), &error)
                });
            }
        }
    })
    .detach();
}

#[cfg(any(test, feature = "native-markdown-tests"))]
mod tests;
#[cfg(feature = "native-markdown-tests")]
pub(super) fn native_cases() -> impl Iterator<Item = crate::native_markdown::Case> {
    tests::native_cases()
}
