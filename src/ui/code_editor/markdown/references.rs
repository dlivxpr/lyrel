//! Presentation-local completion lease and the standard Kit list consumer.
use super::*;
use super::super::links::references::Candidate;
use gpui_kit::component::{IndexPath, Sizable as _};
use gpui_kit::component::list::{List, ListDelegate, ListEvent, ListItem, ListState};

#[derive(Default)]
pub(super) struct State {
    pub(super) generation: u64,
    pub(super) target: Option<Target>,
    list: Option<Entity<ListState<Candidates>>>,
    subscriptions: Vec<Subscription>,
    status: Option<L10nKey>,
}
pub(super) struct Target {
    range: Range<usize>,
    cursor: usize,
    revision: u64,
    resources: super::super::resources::ResourceContext,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl Drop for Target {
    fn drop(&mut self) {
        self.cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}
pub(super) struct Candidates { items: Vec<Candidate> }
impl ListDelegate for Candidates {
    type Item = ListItem;
    fn items_count(&self, _: usize, _: &App) -> usize { self.items.len() }
    fn set_selected_index(&mut self, _: Option<IndexPath>, _: &mut Window, cx: &mut Context<ListState<Self>>) { cx.notify(); }
    fn render_item(&mut self, ix: IndexPath, _: &mut Window, cx: &mut Context<ListState<Self>>) -> Option<Self::Item> {
        let item = self.items.get(ix.row)?;
        Some(ListItem::new(SharedString::from(item.destination().to_owned())).child(
            gpui_kit::component::h_flex().gap_2().child(item.label().to_owned()).child(
                div().truncate().text_color(cx.theme().muted_foreground).child(item.destination().to_owned()))))
    }
}
impl SourcePresentation {
    pub(super) fn dismiss_references(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.references.target.take().is_some();
        self.references.generation = self.references.generation.wrapping_add(1);
        self.references.list = None;
        self.references.subscriptions.clear();
        self.references.status = None;
        if open { cx.notify(); }
        open
    }
    pub(super) fn request_references(&mut self, cx: &mut Context<Self>) {
        self.dismiss_references(cx);
        if !self.live || self.state.composition.is_some() || !self.state.selection.range().is_empty() { return; }
        let document = self.document.read(cx);
        let Some(resources) = document.resources().cloned() else { return; };
        if resources.path().is_none() { return; }
        let end = self.state.selection.head();
        let start = document.source().line_start_offset(document.source().offset_to_point(end).row);
        let line = document.source().slice(start..end).to_string();
        let trigger = [("[[", false), ("((", true), ("#", false)].into_iter().filter_map(|(delimiter, anchors)| {
            let at = line.rfind(delimiter)?;
            if at > 0 && matches!(line.as_bytes()[at - 1], b'\\' | b'!') { return None; }
            let query = &line[at + delimiter.len()..];
            if delimiter == "#" {
                if at > 0 && !line[..at].chars().next_back().is_some_and(char::is_whitespace) { return None; }
                if query.is_empty() { return None; }
            }
            if query.contains(['\n', ']', ')']) || query.starts_with(' ') { return None; }
            if anchors && query.chars().count() < 2 { return None; }
            Some((at, delimiter, anchors))
        }).min_by_key(|(at, _, _)| *at);
        let Some((at, delimiter, anchors)) = trigger else { return; };
        // Positioned code/math/object regions retain their ordinary input behavior.
        if self.projection.leaf_at(end).is_some_and(|leaf| !leaf.accepts_reference(end)) { return; }
        let mut query = line[at + delimiter.len()..].to_owned();
        if delimiter == "#" { query.insert(0, '#'); }
        let revision = document.revision();
        let resource_epoch = Some((resources.host().id(), resources.epoch()));
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let closing = match delimiter { "[[" => "]]", "((" => "))", _ => "" };
        let suffix = document.source().chars_at(end).zip(closing.chars()).take_while(|(a, b)| a == b).count();
        self.references.target = Some(Target { range: start + at..end + suffix, cursor: end, revision, resources, cancelled: cancelled.clone() });
        self.references.status = Some(L10nKey::MarkdownReferenceLoading);
        let presentation = cx.weak_entity();
        let generation = self.references.generation;
        self.document.update(cx, |_, cx| cx.emit(DocumentEvent::RequestReferences { presentation, generation, revision, resource_epoch, cancelled, query, anchors }));
        cx.notify();
    }
    pub(crate) fn discard_reference_results(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self.references.generation == generation { self.dismiss_references(cx); }
    }
    pub(crate) fn install_reference_results(&mut self, generation: u64, result: Result<Vec<Candidate>, L10nKey>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.reference_lease_current(generation, cx) { return; }
        if !self.focus.is_focused(window) { self.dismiss_references(cx); return; }
        let items = match result {
            Ok(items) => items,
            Err(reason) => { self.references.status = Some(reason); cx.notify(); return; }
        };
        self.references.status = items.is_empty().then_some(L10nKey::MarkdownReferenceNoMatches);
        if items.is_empty() { cx.notify(); return; }
        let list = cx.new(|cx| ListState::new(Candidates { items }, window, cx));
        list.update(cx, |list, cx| list.set_selected_index(Some(IndexPath::new(0)), window, cx));
        self.references.subscriptions = vec![cx.subscribe_in(&list, window, |this, _, event, window, cx| {
            match event {
                ListEvent::Confirm(_) => { this.accept_reference(window, cx); }
                ListEvent::Cancel => { this.dismiss_references(cx); this.focus.focus(window, cx); }
                _ => {}
            }
        })];
        self.references.list = Some(list);
        cx.notify();
    }
    pub(super) fn reference_lease_current(&self, generation: u64, cx: &App) -> bool {
        self.references.generation == generation && self.references.target.as_ref().is_some_and(|target| {
            let document = self.document.read(cx);
            document.revision() == target.revision && target.resources.is_current(document.resources())
                && self.state.selection == Selection::caret(target.cursor)
        })
    }
    pub(super) fn accept_reference(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.references.target.is_none() { return false; }
        if !self.reference_lease_current(self.references.generation, cx) { self.dismiss_references(cx); return false; }
        let Some(list) = &self.references.list else { return self.references.target.is_some(); };
        let Some(item) = list.read(cx).selected_index().and_then(|ix| list.read(cx).delegate().items.get(ix.row)).cloned() else { return true; };
        let range = self.references.target.as_ref().unwrap().range.clone();
        let mut text = String::with_capacity(item.label().len() + item.destination().len() + 4);
        text.push('[');
        for ch in item.label().chars() {
            if matches!(ch, '\\' | '[' | ']' | '*' | '_' | '`' | '~' | '<' | '>' | '&' | '|') { text.push('\\'); }
            text.push(ch);
        }
        text.push_str("](");
        text.push_str(item.destination());
        text.push(')');
        self.dismiss_references(cx);
        self.command(Command::Replace { range, text, kind: EditKind::Command }, window, cx);
        self.focus.focus(window, cx);
        true
    }
    pub(super) fn move_reference(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.references.target.is_none() { return false; }
        if !self.reference_lease_current(self.references.generation, cx) { self.dismiss_references(cx); return false; }
        let Some(list) = &self.references.list else { return self.references.target.is_some(); };
        list.update(cx, |list, cx| {
            let count = list.delegate().items.len();
            let ix = list.selected_index().map_or(0, |ix| ix.row);
            let ix = if down { (ix + 1) % count } else { (ix + count - 1) % count };
            list.set_selected_index(Some(IndexPath::new(ix)), window, cx);
            list.scroll_to_selected_item(window, cx);
        });
        cx.notify();
        true
    }
    pub(super) fn render_references(&self, window: &Window, cx: &App) -> AnyElement {
        if !self.reference_lease_current(self.references.generation, cx) || !self.focus.contains_focused(window, cx) { return Empty.into_any_element(); }
        let mut surface = div().id("markdown-reference-surface").test_support().flex_none().max_h_48()
            .bg(cx.theme().popover).text_color(cx.theme().popover_foreground).border_1().border_color(cx.theme().border)
            .rounded(cx.theme().radius).p_1();
        if let Some(status) = self.references.status {
            surface = surface.child(div().id("markdown-reference-status").test_support().role(Role::Status).aria_label(t(status)).text_sm().p_2().child(t(status)));
        }
        if let Some(list) = &self.references.list { surface = surface.child(div().id("markdown-reference-completions").test_support().child(List::new(list).small().max_h_40())); }
        surface.into_any_element()
    }
}
