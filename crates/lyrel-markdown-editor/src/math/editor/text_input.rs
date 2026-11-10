//! Native text-system boundary for the structural seat. Preedit is ephemeral and never enters Document.
use super::*;
use std::ops::Range;

impl MathEditor {
    fn replace_composition(&mut self, range: Option<Range<usize>>, text: &str) -> (String, usize) {
        let mut value = self.composition.take().unwrap_or_default();
        let range = range.map(|range| crate::utf16_to_byte(&value, range.start)..crate::utf16_to_byte(&value, range.end))
            .unwrap_or(0..value.len());
        let base = value[..range.start].encode_utf16().count();
        value.replace_range(range, text);
        (value, base)
    }
    fn insert_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let before = (self.root.clone(), self.cursor.clone());
        for c in text.chars().filter(|c| !c.is_control()) {
            let key = Keystroke {
                modifiers: Modifiers::default(),
                key: if c == ' ' { "space".into() } else { c.to_string() },
                key_char: Some(c.to_string()),
            };
            if self.pending.is_some() { self.handle_pending(&key); }
            else { self.handle_normal(&key); }
        }
        self.commit_edit(before, window, cx);
    }
}

impl EntityInputHandler for MathEditor {
    fn text_for_range(&mut self, range: Range<usize>, actual: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let text = self.composition.as_deref().unwrap_or("");
        let start = crate::utf16_to_byte(text, range.start);
        let end = crate::utf16_to_byte(text, range.end).max(start);
        *actual = Some(text[..start].encode_utf16().count()..text[..end].encode_utf16().count());
        Some(text[start..end].to_owned())
    }
    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.composition_selection.clone(), reversed: false })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.composition.as_ref().map(|text| 0..text.encode_utf16().count())
    }
    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.composition.take() { self.insert_text(&text, window, cx); }
        self.composition_selection = 0..0;
        cx.notify();
    }
    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (text, _) = self.replace_composition(range, text);
        self.composition_selection = 0..0;
        self.insert_text(&text, window, cx);
    }
    fn replace_and_mark_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, selected: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        let (value, base) = self.replace_composition(range, text);
        let len = text.encode_utf16().count();
        let selected = selected.unwrap_or(len..len);
        self.composition_selection = base + selected.start.min(len)..base + selected.end.min(len);
        self.composition = (!value.is_empty()).then_some(value);
        cx.notify();
    }
    fn bounds_for_range(&mut self, _: Range<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let image = self.img_bounds.get()?;
        let (x, y, height) = self.caret_px()?;
        Some(Bounds::new(point(image.left() + px(x), image.top() + px(y)), size(px(2.), px(height))))
    }
    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.composition_selection.end)
    }
}
