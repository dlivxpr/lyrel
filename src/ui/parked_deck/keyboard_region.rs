use gpui_kit::*;

/// 键盘/AX 区域不自动把其矩形变成鼠标目标：鼠标只能走卡片的真实逆矩阵。
/// 通过公开 Interactivity 生命周期复用 Tab、焦点和 key dispatch，不复制框架实现。
pub(super) struct KeyboardRegion {
    interactivity: Interactivity,
    child: AnyElement,
    label: SharedString,
}

impl KeyboardRegion {
    pub fn new(child: AnyElement, label: SharedString) -> Self {
        Self { interactivity: Interactivity::default(), child, label }
    }
}

impl IntoElement for KeyboardRegion {
    type Element = Self;
    fn into_element(self) -> Self { self }
}

impl InteractiveElement for KeyboardRegion {
    fn interactivity(&mut self) -> &mut Interactivity { &mut self.interactivity }
}

impl Element for KeyboardRegion {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> { Some("parked-deck-keyboard".into()) }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> { None }
    fn a11y_role(&self) -> Option<Role> { Some(Role::Group) }
    fn write_a11y_info(&self, node: &mut accesskit::Node) { node.set_label(self.label.to_string()); }

    fn request_layout(&mut self, id: Option<&GlobalElementId>, inspector: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let child_layout = self.child.request_layout(window, cx);
        let layout = self.interactivity.request_layout(id, inspector, window, cx, |style, window, cx| {
            window.request_layout(style, [child_layout], cx)
        });
        (layout, ())
    }

    fn prepaint(&mut self, id: Option<&GlobalElementId>, inspector: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) {
        let child = &mut self.child;
        self.interactivity.prepaint(id, inspector, bounds, bounds.size, window, cx, |_, _, _, window, cx| {
            child.prepaint(window, cx);
        });
    }

    fn paint(&mut self, id: Option<&GlobalElementId>, inspector: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window, cx: &mut App) {
        let child = &mut self.child;
        // None 刻意不注册基于矩形 hitbox 的自动鼠标焦点；键盘和 AX listener 仍由框架安装。
        self.interactivity.paint(id, inspector, bounds, None, window, cx, |_, window, cx| {
            child.paint(window, cx);
        });
    }
}
