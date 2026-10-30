//! Quiet directory revalidation uses the browser's existing polling lifecycle.
use super::{SftpEntry, SftpPanelState};

pub(super) struct SftpRevalidation {
    pane_id: u64,
    poll_gen: u64,
    nav_gen: u64,
    refresh_gen: u64,
    cwd: String,
}

impl SftpRevalidation {
    pub(super) fn path(&self) -> &str {
        &self.cwd
    }
}

impl SftpPanelState {
    pub(super) fn revalidation_request(&self) -> Option<SftpRevalidation> {
        if self.loading || self.editing.is_some() || self.editing_path.is_some() {
            return None;
        }
        Some(SftpRevalidation {
            pane_id: self.open_pane_id?,
            poll_gen: self.poll_gen,
            nav_gen: self.nav_gen,
            refresh_gen: self.refresh_gen,
            cwd: self.cwd.clone(),
        })
    }

    /// Returns whether presentation changed. A failed or retired request never
    /// replaces usable rows, and this path owns no navigation, input or scroll.
    pub(super) fn apply_revalidation(
        &mut self,
        request: &SftpRevalidation,
        reply: Result<Vec<SftpEntry>, String>,
    ) -> bool {
        if !self.navigation_is_current(request.pane_id, request.nav_gen)
            || self.poll_gen != request.poll_gen
            || self.refresh_gen != request.refresh_gen
            || self.cwd != request.cwd
            || self.loading
            || self.editing.is_some()
            || self.editing_path.is_some()
        {
            return false;
        }
        let Ok(mut entries) = reply else { return false };
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        if self.entries == entries && self.error.is_none() {
            return false;
        }
        self.entries = entries;
        self.error = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::protocol::SftpEntryKind;
    use crate::ui::app::test_window::harness;
    use gpui_kit::TestAppContext;

    fn row() -> SftpEntry {
        SftpEntry {
            name: "kept.txt".into(),
            kind: SftpEntryKind::File,
            size: 17,
            mtime: 1,
            permissions: 0o100644,
            target_is_dir: false,
        }
    }

    // These assert state/epoch boundaries only; external-file discovery is
    // covered separately through the ignored real native SSH consumer.
    #[gpui_kit::test]
    fn unchanged_or_failed_revalidation_keeps_rows_without_a_notification(cx: &mut TestAppContext) {
        let (app, mut vcx) = harness(cx);
        app.update_in(&mut vcx, |app, _, _| {
            let panel = &mut app.sftp_panel;
            panel.open_pane_id = Some(42);
            panel.entries = vec![row()];
            let request = panel.revalidation_request().unwrap();
            assert!(!panel.apply_revalidation(&request, Ok(vec![row()])));
            assert!(!panel.apply_revalidation(&request, Err("offline".into())));
            assert_eq!(panel.entries, [row()]);
            assert!(panel.error.is_none());
            assert!(!panel.loading);
        });
    }

    #[gpui_kit::test]
    fn navigation_stop_hide_and_reconnect_retire_pending_revalidation(cx: &mut TestAppContext) {
        let (app, mut vcx) = harness(cx);
        app.update_in(&mut vcx, |app, _, cx| {
            app.sftp_panel.open_pane_id = Some(42);
            app.sftp_panel.entries = vec![row()];
            let navigation = app.sftp_panel.revalidation_request().unwrap();
            app.sftp_panel.nav_gen = app.sftp_panel.nav_gen.wrapping_add(1);
            assert!(
                !app.sftp_panel
                    .apply_revalidation(&navigation, Ok(Vec::new()))
            );
            let hidden = app.sftp_panel.revalidation_request().unwrap();
            app.sftp_retire_revalidation();
            assert!(!app.sftp_panel.apply_revalidation(&hidden, Ok(Vec::new())));
            let pane_switch = app.sftp_panel.revalidation_request().unwrap();
            app.sftp_panel.open_pane_id = Some(43);
            assert!(
                !app.sftp_panel
                    .apply_revalidation(&pane_switch, Ok(Vec::new()))
            );
            app.sftp_panel.open_pane_id = Some(42);
            let stopped = app.sftp_panel.revalidation_request().unwrap();
            app.sftp_close_browser(cx);
            app.sftp_panel.open_pane_id = Some(42);
            app.sftp_panel.entries = vec![row()];
            assert!(!app.sftp_panel.apply_revalidation(&stopped, Ok(Vec::new())));
            assert_eq!(app.sftp_panel.entries, [row()]);
        });
    }

    #[gpui_kit::test]
    fn editing_or_navigation_loading_defers_revalidation(cx: &mut TestAppContext) {
        let (app, mut vcx) = harness(cx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.sftp_panel.open_pane_id = Some(42);
            let pending = app.sftp_panel.revalidation_request().unwrap();
            app.sftp_panel.loading = true;
            assert!(app.sftp_panel.revalidation_request().is_none());
            assert!(!app.sftp_panel.apply_revalidation(&pending, Ok(vec![row()])));
            app.sftp_panel.loading = false;
            app.sftp_begin_new_file(window, cx);
            assert!(app.sftp_panel.revalidation_request().is_none());
            assert!(!app.sftp_panel.apply_revalidation(&pending, Ok(vec![row()])));
            app.sftp_cancel_edit(window, cx);
            app.sftp_begin_edit_path(window, cx);
            assert!(app.sftp_panel.revalidation_request().is_none());
            assert!(!app.sftp_panel.apply_revalidation(&pending, Ok(vec![row()])));
            app.sftp_cancel_edit_path(cx);
            assert!(app.sftp_panel.revalidation_request().is_some());
        });
    }
}
