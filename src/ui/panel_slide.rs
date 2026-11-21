//! Boundary slides for the two panels that flank the workbench.
//!
//! A toggle flips its boundary's settled state immediately — the terminal is
//! laid out at its final width in the same frame, so the PTY hears exactly one
//! resize per toggle. What animates is only the painted seam: the panel's
//! edge-anchored clip interpolates from the old boundary to the new one over
//! [`PANEL_SLIDE_MS`] on the shared non-overshooting [`settle`] curve (the
//! pinned Diri behavior in #345). Repeated toggles mid-slide are dropped, not
//! queued or reversed. Under reduced motion there is no slide at all: the
//! settled layout is already the final layout, and the framework renders the
//! animation's end state without scheduling frames.

use std::time::Duration;

use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, Context, TestSupportExt as _, Window, div,
    prelude::*, px,
};

use crate::ui::app::LyrelApp;

/// How long a panel boundary takes to slide open or shut.
pub(crate) const PANEL_SLIDE_MS: u64 = 260;

/// The shared non-overshooting transition curve: the normalized step response
/// of a critically damped spring (stiffness 7), divided through so it spans
/// exactly 0..=1. It leaves fast and settles soft, and it never crosses its
/// target — a boundary that overshot would drag its neighbour's layout back
/// and forth instead of reading as elasticity on one panel.
pub(crate) fn settle(delta: f32) -> f32 {
    const STIFFNESS: f32 = 7.0;
    let delta = delta.clamp(0.0, 1.0);
    let remaining = (1.0 + STIFFNESS * delta) * (-STIFFNESS * delta).exp();
    (1.0 - remaining) / (1.0 - (1.0 + STIFFNESS) * (-STIFFNESS).exp())
}

/// Which panel boundary a slide belongs to. The two flanking panels animate
/// independently: a toggle on one is never blocked by the other's slide.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PanelBoundary {
    Left,
    Right,
}

impl PanelBoundary {
    fn slot(self) -> usize {
        match self {
            PanelBoundary::Left => 0,
            PanelBoundary::Right => 1,
        }
    }
}

/// One panel open/close in flight. The boundary's settled state flipped at
/// toggle time; the visual seam interpolates toward it. `generation` keeps a
/// late-ending cleanup from clearing a newer slide that replaced this one.
/// The boundary itself is implicit in the slot the slide is stored in.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PanelSlide {
    pub(crate) opening: bool,
    generation: u64,
}

/// The seam width at `delta` for a slide toward a settled width of `full`.
fn seam_at(opening: bool, full: f32, delta: f32) -> f32 {
    let seam = if opening {
        full * delta
    } else {
        full * (1.0 - delta)
    };
    seam.max(0.0)
}

fn slide_animation() -> Animation {
    Animation::new(Duration::from_millis(PANEL_SLIDE_MS)).with_easing(settle)
}

/// The settled boundary mounted beside a sliding neighbour: identical geometry
/// to the ordinary flex sibling, but absolutely anchored so the pinned content
/// keeps the width the PTY was told.
fn settled_sidebar_mount(width: f32, sidebar: AnyElement) -> AnyElement {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .w(px(width))
        .child(sidebar)
        .into_any_element()
}

fn settled_right_panel_mount(width: f32, panel: AnyElement) -> AnyElement {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .right_0()
        .w(px(width))
        .child(panel)
        .into_any_element()
}

#[derive(Default)]
pub(crate) struct PanelSlides {
    active: [Option<PanelSlide>; 2],
    // An older slide's timer must not clear its replacement.
    generations: [u64; 2],
}

impl LyrelApp {
    pub(crate) fn panel_slide(&self, boundary: PanelBoundary) -> Option<PanelSlide> {
        self.panel_slides.active[boundary.slot()]
    }

    /// The toggle guard: while a boundary's seam is still travelling, further
    /// toggles of that boundary are dropped rather than queued or reversed.
    pub(crate) fn panel_slide_active(&self, boundary: PanelBoundary) -> bool {
        self.panel_slide(boundary).is_some()
    }

    /// Cancel slides whose settled state no longer matches their direction.
    /// Visibility can also change through direct commands (opening a tool
    /// flips the panel open without a toggle); those snaps must not leave a
    /// contradictory slide painting underneath.
    pub(crate) fn reconcile_panel_slides(&mut self, cx: &mut Context<Self>) {
        for boundary in [PanelBoundary::Left, PanelBoundary::Right] {
            let Some(slide) = self.panel_slide(boundary) else {
                continue;
            };
            let settled_open = match boundary {
                PanelBoundary::Left => self.sidebar_open(cx),
                PanelBoundary::Right => self.right_panel_open(cx),
            };
            if settled_open != slide.opening {
                self.panel_slides.active[boundary.slot()] = None;
            }
        }
    }

    /// Begin the visual slide for a boundary whose settled state just flipped.
    /// Reduced motion takes no slide: the settled layout is final immediately,
    /// and `with_animation` renders the end state without scheduling frames.
    pub(crate) fn begin_panel_slide(
        &mut self,
        boundary: PanelBoundary,
        opening: bool,
        cx: &mut Context<Self>,
    ) {
        if cx.reduce_motion() {
            return;
        }
        let slot = boundary.slot();
        let generation = self.panel_slides.generations[slot] + 1;
        self.panel_slides.generations[slot] = generation;
        self.panel_slides.active[slot] = Some(PanelSlide {
            opening,
            generation,
        });
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(PANEL_SLIDE_MS))
                .await;
            let _ = this.update(cx, |app, cx| {
                if app
                    .panel_slide(boundary)
                    .is_some_and(|slide| slide.generation == generation)
                {
                    app.panel_slides.active[slot] = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The settled width of the left boundary: what its seam settles at.
    pub(crate) fn sidebar_final_px(&self, window: &Window, cx: &gpui_kit::App) -> f32 {
        if self.sidebar_open(cx) {
            self.sidebar_px(window, cx)
        } else {
            0.
        }
    }

    /// The settled width of the right boundary: what its seam settles at.
    pub(crate) fn right_panel_final_px(&self, window: &Window, cx: &gpui_kit::App) -> f32 {
        if self.right_panel_open(cx) {
            self.right_panel_px(window, cx)
        } else {
            0.
        }
    }

    /// The left sidebar, edge-anchored and clipped to the animated seam. The
    /// sidebar itself always paints at its settled width; only the clip
    /// travels, so its rows do not reflow mid-slide.
    pub(crate) fn render_sliding_sidebar(
        &mut self,
        slide: PanelSlide,
        sidebar: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let full = self.sidebar_px(window, cx);
        div()
            .id("left-panel-slide")
            .test_support()
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .overflow_hidden()
            .child(sidebar)
            .with_animation(
                ("left-panel-slide-motion", slide.generation),
                slide_animation(),
                move |el, delta| el.w(px(seam_at(slide.opening, full, delta))),
            )
            .into_any_element()
    }

    /// The right panel, edge-anchored and clipped to the animated seam. The
    /// panel always paints at its settled width; only the clip travels.
    pub(crate) fn render_sliding_right_panel(
        &mut self,
        slide: PanelSlide,
        panel: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let full = self.right_panel_px(window, cx);
        div()
            .id("right-panel-slide")
            .test_support()
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .overflow_hidden()
            .child(panel)
            .with_animation(
                ("right-panel-slide-motion", slide.generation),
                slide_animation(),
                move |el, delta| el.w(px(seam_at(slide.opening, full, delta))),
            )
            .into_any_element()
    }

    /// Mount the left boundary while the workbench is in the in-flight layout:
    /// the animated clip while its own slide runs, otherwise its settled width.
    pub(crate) fn render_left_boundary(
        &mut self,
        slide: Option<PanelSlide>,
        sidebar: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match slide {
            Some(slide) => self.render_sliding_sidebar(slide, sidebar, window, cx),
            None => settled_sidebar_mount(self.sidebar_final_px(window, cx), sidebar),
        }
    }

    /// Mount the right boundary while the workbench is in the in-flight
    /// layout: the animated clip while its own slide runs, otherwise its
    /// settled width.
    pub(crate) fn render_right_boundary(
        &mut self,
        slide: Option<PanelSlide>,
        panel: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match slide {
            Some(slide) => self.render_sliding_right_panel(slide, panel, window, cx),
            None => settled_right_panel_mount(self.right_panel_final_px(window, cx), panel),
        }
    }
}
