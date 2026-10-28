//! 搜索、页面定位与滚动的单一所有者。窗口只执行跨页协调、focus 和 GPUI 展示。
use super::*;
use crate::ui::settings_window::SettingsLocation;
pub(super) enum SearchActivation {
    Setting(SettingsSection, L10nKey),
    Shortcut(String),
}

pub(crate) struct SettingsNavigation {
    section: SettingsSection,
    search: Entity<InputState>,
    modified_only: bool,
    searching: bool,
    return_offset: gpui_kit::Point<gpui_kit::Pixels>,
    selection: usize,
    rows: RefCell<Option<Vec<(L10nKey, AnyElement)>>>,
    focused_setting: Option<L10nKey>,
    scroll: gpui_kit::ScrollHandle,
    anchor: gpui_kit::ScrollAnchor,
    reveal: Cell<bool>,
    hit_anchored: Cell<bool>,
}

impl SettingsNavigation {
    pub(super) fn new(search: Entity<InputState>, location: SettingsLocation) -> Self {
        let scroll = gpui_kit::ScrollHandle::new();
        scroll.set_offset(location.1);
        let anchor = gpui_kit::ScrollAnchor::for_handle(scroll.clone());
        Self {
            section: location.0,
            search,
            modified_only: false,
            searching: false,
            return_offset: location.1,
            selection: 0,
            rows: RefCell::new(None),
            focused_setting: None,
            scroll,
            anchor,
            reveal: Cell::new(false),
            hit_anchored: Cell::new(false),
        }
    }

    /// 复用已有 query Entity 的身份；每次 build 都是新 session，不增加 Entity 或全局计数器。
    pub(crate) fn session(&self) -> gpui_kit::EntityId {
        self.search.entity_id()
    }

    pub(crate) fn accepts(&self, session: gpui_kit::EntityId) -> bool {
        self.session() == session
    }

    pub(crate) fn section(&self) -> SettingsSection {
        self.section
    }
    pub(crate) fn search(&self) -> &Entity<InputState> {
        &self.search
    }
    pub(crate) fn is_searching(&self) -> bool {
        self.searching
    }
    pub(crate) fn focused_setting(&self) -> Option<L10nKey> {
        self.focused_setting
    }
    pub(super) fn modified_only(&self) -> bool {
        self.modified_only
    }
    pub(super) fn selection(&self) -> usize {
        self.selection
    }
    pub(super) fn scroll(&self) -> &gpui_kit::ScrollHandle {
        &self.scroll
    }
    pub(super) fn anchor(&self) -> gpui_kit::ScrollAnchor {
        self.anchor.clone()
    }

    pub(crate) fn location(&self) -> SettingsLocation {
        (
            self.section,
            if self.searching {
                self.return_offset
            } else {
                self.scroll.offset()
            },
        )
    }

    /// 返回是否换页；page-local retirement 由窗口调用各 owner 的语义 hook。
    pub(super) fn select_section(&mut self, target: SettingsSection) -> bool {
        let moved = self.section != target;
        self.section = target;
        if moved {
            self.reveal.set(true);
        }
        moved
    }

    pub(super) fn search_changed(&mut self, cx: &App) {
        let active = !self.search.read(cx).value().trim().is_empty() || self.modified_only;
        if active && !self.searching {
            self.return_offset = self.scroll.offset();
        }
        if active {
            self.scroll.set_offset(gpui_kit::point(px(0.), px(0.)));
            self.focused_setting = None;
        } else if self.searching {
            self.scroll.set_offset(self.return_offset);
        }
        self.searching = active;
        self.selection = 0;
    }

    pub(super) fn query_edited(&self) {
        self.reveal.set(true);
    }

    pub(super) fn set_modified_only(&mut self, modified: bool) {
        self.modified_only = modified;
    }
    pub(super) fn toggle_modified_only(&mut self) {
        self.modified_only = !self.modified_only;
    }

    pub(super) fn navigate(&mut self, setting: Option<L10nKey>, config: &Config) {
        self.focused_setting = match setting {
            Some(L10nKey::SettingsCustomPath)
                if config.working_directory.strategy != crate::core::config::WdStrategy::Custom =>
            {
                Some(L10nKey::SettingsStartIn)
            }
            Some(L10nKey::SettingsOpenFilesCommand)
                if config.file_open_mode() != LinkFileOpen::Command =>
            {
                Some(L10nKey::OpenFilesWith)
            }
            other => other,
        };
        self.modified_only = false;
        self.searching = false;
        self.reveal.set(self.focused_setting.is_some());
        self.scroll.set_offset(gpui_kit::point(px(0.), px(0.)));
    }

    pub(super) fn move_selection(&mut self, down: bool, count: usize) {
        if count == 0 {
            return;
        }
        let index = self.selection.min(count - 1);
        self.selection = if down {
            (index + 1).min(count - 1)
        } else {
            index.saturating_sub(1)
        };
        self.reveal.set(true);
    }

    pub(super) fn search_key(&mut self, key: &str, cx: &App) -> Option<SearchActivation> {
        let query = self.search.read(cx).value().trim().to_lowercase();
        let entries = search_result_order(&query, self.modified_only, cx.global::<Config>());
        let actions = search_result_actions(&query, self.modified_only);
        let count = entries.len() + actions.len();
        if count == 0 {
            return None;
        }
        let index = self.selection.min(count - 1);
        if key != "enter" {
            self.move_selection(key == "down", count);
            return None;
        }
        if index >= entries.len() {
            Some(SearchActivation::Shortcut(
                actions[index - entries.len()].clone(),
            ))
        } else {
            let entry = entries[index];
            let target = if entry.title == L10nKey::SettingsSearchKeybindingsTitle {
                SettingsSection::Keybindings
            } else {
                entry.section
            };
            Some(SearchActivation::Setting(target, entry.title))
        }
    }

    pub(super) fn begin_frame(&self) {
        self.hit_anchored.set(false);
    }

    pub(super) fn reveal_target(&self, query: &str) -> Option<gpui_kit::ScrollAnchor> {
        if !self.reveal.replace(false) {
            return None;
        }
        (self.searching
            || self.focused_setting.is_some()
            || (!query.is_empty() && section_match_count(self.section, query) > 0))
            .then(|| self.anchor())
    }

    pub(super) fn first_hit_anchor(&self, label: &str, cx: &App) -> Option<gpui_kit::ScrollAnchor> {
        if self.searching || self.is_capturing_rows() {
            return None;
        }
        let hit = if let Some(target) = self.focused_setting {
            t(target) == label
        } else {
            let query = self.search.read(cx).value().trim().to_lowercase();
            !query.is_empty()
                && section_match_count(self.section, &query) > 0
                && row_matches_query(self.section, label, &query)
        };
        (hit && !self.hit_anchored.replace(true)).then(|| self.anchor())
    }

    pub(super) fn is_capturing_rows(&self) -> bool {
        self.rows.borrow().is_some()
    }
    pub(super) fn begin_rows(&self) {
        *self.rows.borrow_mut() = Some(Vec::new());
    }
    pub(super) fn capture_row(&self, key: L10nKey, row: AnyElement) {
        self.rows
            .borrow_mut()
            .as_mut()
            .expect("search row capture is active")
            .push((key, row));
    }
    pub(super) fn finish_rows(&self) -> Vec<(L10nKey, AnyElement)> {
        self.rows.borrow_mut().take().unwrap_or_default()
    }
}

#[cfg(test)]
#[path = "navigation_tests.rs"]
mod tests;
