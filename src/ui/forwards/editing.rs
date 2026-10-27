use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{App, AppContext as _, Context, Entity, Window};

use super::route::ForwardRoute;
use super::{ForwardFields, Forwarding, added_forward, rule_of};
use crate::daemon::protocol::{ForwardStatus, ManagedForward, SshForwardKind, SshForwardRule};
use crate::ui::i18n::{L10nKey, t};

/// Rejection permits the short form's alternate port; no answer never does.
enum PlaceOutcome {
    Placed,
    Rejected(String),
    Unreachable(String),
}

impl Forwarding {
    pub(crate) fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            form_pane_id: None,
            managed: Vec::new(),
            mf_kind: SshForwardKind::Local,
            mf_bind_host: cx.new(|cx| InputState::new(window, cx).default_value("127.0.0.1")),
            mf_bind_port: cx.new(|cx| InputState::new(window, cx).placeholder("8080")),
            mf_target_host: cx.new(|cx| InputState::new(window, cx).placeholder("127.0.0.1")),
            mf_target_port: cx.new(|cx| InputState::new(window, cx).placeholder("80")),
            mf_description: cx.new(|cx| {
                InputState::new(window, cx).placeholder(t(L10nKey::AppPlaceholderDescription))
            }),
            mf_editing: None,
            mf_error: None,
            mf_subs: Vec::new(),
            mf_advanced: false,
        }
    }

    pub(crate) fn managed(&self) -> &[ManagedForward] {
        &self.managed
    }

    pub(crate) fn is_open_for(&self, pane_id: u64) -> bool {
        self.form_pane_id == Some(pane_id)
    }

    /// Only replies may replace known registry contents.
    pub(crate) fn accept_list(&mut self, reply: Option<Vec<ManagedForward>>) {
        if let Some(list) = reply {
            self.managed = list;
        }
    }

    pub(crate) fn clear_panel(&mut self) {
        self.managed.clear();
    }

    pub(crate) fn refresh(&mut self, route: &ForwardRoute) {
        self.managed = route.list();
    }

    pub(super) fn fields(&self, cx: &App) -> ForwardFields {
        let val = |input: &Entity<InputState>| input.read(cx).value().to_string();
        ForwardFields {
            advanced: self.mf_advanced,
            kind: self.mf_kind,
            bind_host: val(&self.mf_bind_host),
            bind_port: val(&self.mf_bind_port),
            target_host: val(&self.mf_target_host),
            target_port: val(&self.mf_target_port),
            description: val(&self.mf_description),
        }
    }

    pub(super) fn set_kind(&mut self, kind: SshForwardKind) {
        self.mf_kind = kind;
    }

    pub(super) fn toggle_advanced(&mut self) {
        self.mf_advanced = !self.mf_advanced;
        self.mf_error = None;
    }

    /// Apply a create or replacement; a failed save keeps every typed value.
    pub(crate) fn apply(&mut self, route: &ForwardRoute, window: &mut Window, cx: &mut App) {
        let fields = self.fields(cx);
        let Some(mut rule) = fields.collect() else {
            return;
        };
        let previous = self.mf_editing.clone();
        if let Some(old) = &previous {
            rule.enabled = old.enabled;
            // Remove before adding: ordinary edits retain their occupied bind port.
            let Some(list) = route.remove(old.id) else {
                self.mf_error = Some(t(L10nKey::ForwardRequestFailed).to_string());
                return;
            };
            self.managed = list;
        }

        let retry_free_port = !fields.advanced && rule.bind_port != 0;
        let failure = match self.place(route, rule.clone()) {
            PlaceOutcome::Placed => None,
            PlaceOutcome::Unreachable(msg) => Some(msg),
            PlaceOutcome::Rejected(msg) if !retry_free_port => Some(msg),
            PlaceOutcome::Rejected(_) => match self.place(route, SshForwardRule { bind_port: 0, ..rule }) {
                PlaceOutcome::Placed => None,
                PlaceOutcome::Rejected(msg) | PlaceOutcome::Unreachable(msg) => Some(msg),
            },
        };

        if let Some(msg) = failure {
            if let Some(old) = &previous {
                let before: Vec<u64> = self.managed.iter().map(|m| m.id).collect();
                if let Some(list) = route.add(rule_of(old)) {
                    // Rollback creates a new identity. The next save removes that
                    // actual rule, not the retired id, while keeping the draft.
                    if let Some(restored) = added_forward(&before, &list) {
                        self.mf_editing = Some(restored.clone());
                    }
                    self.managed = list;
                }
            }
            self.mf_error = Some(msg);
            return;
        }

        self.mf_editing = None;
        self.mf_error = None;
        self.form_pane_id = None;
        self.clear_values(window, cx);
    }

    fn place(&mut self, route: &ForwardRoute, rule: SshForwardRule) -> PlaceOutcome {
        let before: Vec<u64> = self.managed.iter().map(|m| m.id).collect();
        let Some(list) = route.add(rule) else {
            return PlaceOutcome::Unreachable(t(L10nKey::ForwardRequestFailed).to_string());
        };
        let broken = added_forward(&before, &list)
            .filter(|added| added.enabled)
            .and_then(|added| match &added.status {
                ForwardStatus::Error(msg) => Some((added.id, msg.clone())),
                ForwardStatus::Listening => None,
            });
        self.managed = list;
        let Some((id, msg)) = broken else {
            return PlaceOutcome::Placed;
        };
        self.accept_list(route.remove(id));
        PlaceOutcome::Rejected(msg)
    }

    pub(crate) fn edit(&mut self, forward: ManagedForward, window: &mut Window, cx: &mut App) {
        self.mf_kind = forward.kind;
        self.form_pane_id = Some(forward.pane_id);
        self.mf_error = None;
        // The short form cannot represent a complete existing mapping.
        self.mf_advanced = true;
        let target_port = if forward.target_port == 0 {
            String::new()
        } else {
            forward.target_port.to_string()
        };
        let fields = [
            (&self.mf_bind_host, forward.bind_host.clone()),
            (&self.mf_bind_port, forward.bind_port.to_string()),
            (&self.mf_target_host, forward.target_host.clone()),
            (&self.mf_target_port, target_port),
            (&self.mf_description, forward.description.clone().unwrap_or_default()),
        ];
        for (input, value) in fields {
            input.update(cx, |input, cx| input.set_value(&value, window, cx));
        }
        self.mf_editing = Some(forward);
    }

    fn clear_values(&self, window: &mut Window, cx: &mut App) {
        for input in [&self.mf_bind_port, &self.mf_target_host, &self.mf_target_port, &self.mf_description] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    fn cancel_edit(&mut self, window: &mut Window, cx: &mut App) {
        self.mf_editing = None;
        self.mf_error = None;
        self.clear_values(window, cx);
        self.mf_bind_host.update(cx, |input, cx| input.set_value("127.0.0.1", window, cx));
    }

    pub(crate) fn remove(&mut self, route: &ForwardRoute, forward_id: u64) {
        self.accept_list(route.remove(forward_id));
    }

    /// Return the previous rule only after a successful switch, for the
    /// workbench's cross-feature persistence into its SSH profile.
    pub(crate) fn set_enabled(
        &mut self,
        route: &ForwardRoute,
        forward_id: u64,
        enabled: bool,
    ) -> anyhow::Result<Option<ManagedForward>> {
        let forward = self.managed.iter().find(|m| m.id == forward_id).cloned();
        match route.set_enabled(forward_id, enabled) {
            Ok(list) => {
                self.managed = list;
                Ok(forward)
            }
            Err(e) => {
                self.refresh(route);
                Err(e)
            }
        }
    }

    pub(crate) fn open<T: 'static>(
        &mut self,
        pane_id: u64,
        route: &ForwardRoute,
        window: &mut Window,
        cx: &mut Context<T>,
        on_submit: fn(&mut T, u64, &mut Window, &mut Context<T>),
    ) {
        self.form_pane_id = Some(pane_id);
        self.mf_advanced = false;
        self.mf_kind = SshForwardKind::Local;
        self.cancel_edit(window, cx);
        self.refresh(route);
        let inputs = [
            self.mf_bind_host.clone(), self.mf_bind_port.clone(),
            self.mf_target_host.clone(), self.mf_target_port.clone(), self.mf_description.clone(),
        ];
        self.mf_subs = inputs.iter().map(|input| {
            cx.subscribe_in(input, window, move |owner, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    on_submit(owner, pane_id, window, cx);
                }
            })
        }).collect();
        inputs[0].update(cx, |input, cx| input.focus(window, cx));
    }

    /// The workbench restores content focus only when a form actually closed.
    pub(crate) fn close(&mut self, window: &mut Window, cx: &mut App) -> bool {
        let was_open = self.form_pane_id.take().is_some();
        self.mf_subs.clear();
        self.cancel_edit(window, cx);
        was_open
    }
}
