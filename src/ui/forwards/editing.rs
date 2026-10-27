use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{App, AppContext as _, Context, Entity, Window};

use super::route::{ForwardOwnerKey, ForwardRoute};
use super::{ForwardFields, Forwarding, rule_of};
use crate::daemon::protocol::{ForwardStatus, ManagedForward, SshForwardKind, SshForwardRule};
use crate::ui::i18n::{L10nKey, t};

/// Rejection permits the short form's alternate port; no answer never does.
enum PlaceOutcome {
    Placed,
    Rejected(String),
    Unreachable(String),
}

/// The request may have committed even when its reply was lost. Keep enough
/// identity to reconcile against the same registry before another submission.
pub(super) struct Recovery {
    owner: ForwardOwnerKey,
    previous: Option<ManagedForward>,
    draft: SshForwardRule,
    mutation: Unanswered,
}

enum Unanswered {
    Add {
        rule: SshForwardRule,
        before: Vec<u64>,
        rollback: bool,
    },
    Remove {
        id: u64,
        rejected: bool,
    },
}

enum Reconciled {
    Continue(Option<ManagedForward>),
    Saved,
}

fn matches_rule(rule: &SshForwardRule, forward: &ManagedForward) -> bool {
    rule.kind == forward.kind
        && rule.bind_host == forward.bind_host
        && (rule.bind_port == 0 || rule.bind_port == forward.bind_port)
        && rule.target_host == forward.target_host
        && rule.target_port == forward.target_port
        && rule.description == forward.description
        && rule.enabled == forward.enabled
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
            mf_recovery: None,
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
        self.accept_list(route.list());
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

    /// Apply only after the last unanswered mutation has an authoritative owner
    /// and identity. A lost reply never authorizes a second mutation by itself.
    pub(crate) fn apply(&mut self, route: &ForwardRoute, window: &mut Window, cx: &mut App) {
        let fields = self.fields(cx);
        let Some(mut rule) = fields.collect() else {
            return;
        };
        if let Some(old) = self
            .mf_recovery
            .as_ref()
            .and_then(|recovery| recovery.previous.as_ref())
            .or(self.mf_editing.as_ref())
        {
            rule.enabled = old.enabled;
        }
        let previous = match self.reconcile(route, &rule) {
            Some(Reconciled::Saved) => {
                self.saved(window, cx);
                return;
            }
            Some(Reconciled::Continue(previous)) => previous,
            None => {
                self.mf_error = Some(t(L10nKey::ForwardRequestFailed).to_string());
                return;
            }
        };
        if let Some(old) = self.mf_editing.clone() {
            let Some(list) = route.remove(old.id) else {
                self.mf_recovery = Some(Recovery {
                    owner: route.owner_key(),
                    previous,
                    draft: rule,
                    mutation: Unanswered::Remove {
                        id: old.id,
                        rejected: false,
                    },
                });
                self.mf_error = Some(t(L10nKey::ForwardRequestFailed).to_string());
                return;
            };
            self.managed = list;
            self.mf_editing = None;
        }

        let outcome = match self.place(route, rule.clone(), &previous, &rule) {
            PlaceOutcome::Rejected(_) if !fields.advanced && rule.bind_port != 0 => self.place(
                route,
                SshForwardRule {
                    bind_port: 0,
                    ..rule.clone()
                },
                &previous,
                &rule,
            ),
            outcome => outcome,
        };
        match outcome {
            PlaceOutcome::Placed => self.saved(window, cx),
            PlaceOutcome::Unreachable(msg) => self.mf_error = Some(msg),
            PlaceOutcome::Rejected(msg) => {
                if let Some(old) = &previous {
                    let before: Vec<u64> = self.managed.iter().map(|m| m.id).collect();
                    let restored_rule = rule_of(old);
                    let restored = route.add(restored_rule.clone()).and_then(|list| {
                        let mut candidates = list.iter().filter(|forward| {
                            !before.contains(&forward.id) && matches_rule(&restored_rule, forward)
                        });
                        let restored = candidates.next().cloned();
                        let unique = candidates.next().is_none();
                        self.managed = list;
                        restored.filter(|_| unique)
                    });
                    if let Some(restored) = restored {
                        self.mf_editing = Some(restored);
                    } else {
                        self.mf_recovery = Some(Recovery {
                            owner: route.owner_key(),
                            previous,
                            draft: rule,
                            mutation: Unanswered::Add {
                                rule: restored_rule,
                                before,
                                rollback: true,
                            },
                        });
                    }
                }
                self.mf_error = Some(msg);
            }
        }
    }

    fn saved(&mut self, window: &mut Window, cx: &mut App) {
        self.mf_editing = None;
        self.mf_recovery = None;
        self.mf_error = None;
        self.form_pane_id = None;
        self.clear_values(window, cx);
    }

    fn reconcile(&mut self, route: &ForwardRoute, draft: &SshForwardRule) -> Option<Reconciled> {
        let Some(mut recovery) = self.mf_recovery.take() else {
            return Some(Reconciled::Continue(self.mf_editing.clone()));
        };
        if recovery.owner != route.owner_key() {
            self.mf_recovery = Some(recovery);
            return None;
        }
        let Some(list) = route.list() else {
            self.mf_recovery = Some(recovery);
            return None;
        };
        self.managed = list;
        match &recovery.mutation {
            Unanswered::Add {
                rule,
                before,
                rollback,
            } => {
                let mut candidates = self
                    .managed
                    .iter()
                    .filter(|forward| !before.contains(&forward.id) && matches_rule(rule, forward));
                let added = candidates.next().cloned();
                if candidates.next().is_some() {
                    // Without request ids, identical concurrent additions are
                    // ambiguous. Do not pick one to remove or silently accept.
                    self.mf_recovery = Some(recovery);
                    return None;
                }
                self.mf_editing = None;
                if let Some(added) = added {
                    if *rollback {
                        self.mf_editing = Some(added.clone());
                        recovery.previous = Some(added);
                    } else if added.enabled && matches!(added.status, ForwardStatus::Error(_)) {
                        let Some(list) = route.remove(added.id) else {
                            recovery.mutation = Unanswered::Remove {
                                id: added.id,
                                rejected: true,
                            };
                            self.mf_recovery = Some(recovery);
                            return None;
                        };
                        self.managed = list;
                    } else if recovery.draft == *draft {
                        return Some(Reconciled::Saved);
                    } else {
                        // The user changed the draft meanwhile: replace the
                        // actual committed identity, never the retired old id.
                        self.mf_editing = Some(added.clone());
                        recovery.previous = Some(added);
                    }
                }
            }
            Unanswered::Remove { id, rejected } => {
                if *rejected {
                    if self.managed.iter().any(|forward| forward.id == *id) {
                        let Some(list) = route.remove(*id) else {
                            self.mf_recovery = Some(recovery);
                            return None;
                        };
                        self.managed = list;
                    }
                    self.mf_editing = None;
                } else {
                    self.mf_editing = self
                        .managed
                        .iter()
                        .find(|forward| forward.id == *id)
                        .cloned();
                }
            }
        }
        Some(Reconciled::Continue(recovery.previous))
    }

    fn place(
        &mut self,
        route: &ForwardRoute,
        rule: SshForwardRule,
        previous: &Option<ManagedForward>,
        draft: &SshForwardRule,
    ) -> PlaceOutcome {
        let before: Vec<u64> = self.managed.iter().map(|m| m.id).collect();
        let Some(list) = route.add(rule.clone()) else {
            self.mf_recovery = Some(Recovery {
                owner: route.owner_key(),
                previous: previous.clone(),
                draft: draft.clone(),
                mutation: Unanswered::Add {
                    rule,
                    before,
                    rollback: false,
                },
            });
            return PlaceOutcome::Unreachable(t(L10nKey::ForwardRequestFailed).to_string());
        };
        let mut candidates = list
            .iter()
            .filter(|forward| !before.contains(&forward.id) && matches_rule(&rule, forward));
        let added = candidates.next();
        if added.is_none() || candidates.next().is_some() {
            self.managed = list;
            self.mf_recovery = Some(Recovery {
                owner: route.owner_key(),
                previous: previous.clone(),
                draft: draft.clone(),
                mutation: Unanswered::Add {
                    rule,
                    before,
                    rollback: false,
                },
            });
            return PlaceOutcome::Unreachable(t(L10nKey::ForwardRequestFailed).to_string());
        }
        let broken = added
            .filter(|added| added.enabled)
            .and_then(|added| match &added.status {
                ForwardStatus::Error(msg) => Some((added.id, msg.clone())),
                ForwardStatus::Listening => None,
            });
        self.managed = list;
        let Some((id, msg)) = broken else {
            return PlaceOutcome::Placed;
        };
        let Some(list) = route.remove(id) else {
            self.mf_recovery = Some(Recovery {
                owner: route.owner_key(),
                previous: previous.clone(),
                draft: draft.clone(),
                mutation: Unanswered::Remove { id, rejected: true },
            });
            return PlaceOutcome::Unreachable(t(L10nKey::ForwardRequestFailed).to_string());
        };
        self.managed = list;
        PlaceOutcome::Rejected(msg)
    }

    pub(crate) fn edit(&mut self, forward: ManagedForward, window: &mut Window, cx: &mut App) {
        if self.mf_recovery.is_some() {
            // A different row must not replace the draft whose mutation is
            // still unresolved. Finish that submission against its owner first.
            self.mf_error = Some(t(L10nKey::ForwardRequestFailed).to_string());
            return;
        }
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
            (
                &self.mf_description,
                forward.description.clone().unwrap_or_default(),
            ),
        ];
        for (input, value) in fields {
            input.update(cx, |input, cx| input.set_value(&value, window, cx));
        }
        self.mf_editing = Some(forward);
    }

    fn clear_values(&self, window: &mut Window, cx: &mut App) {
        for input in [
            &self.mf_bind_port,
            &self.mf_target_host,
            &self.mf_target_port,
            &self.mf_description,
        ] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    fn cancel_edit(&mut self, window: &mut Window, cx: &mut App) {
        self.mf_editing = None;
        self.mf_error = None;
        self.clear_values(window, cx);
        self.mf_bind_host
            .update(cx, |input, cx| input.set_value("127.0.0.1", window, cx));
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
        if self
            .mf_recovery
            .as_ref()
            .is_some_and(|recovery| recovery.owner != route.owner_key())
        {
            self.mf_error = Some(t(L10nKey::ForwardRequestFailed).to_string());
            return;
        }
        self.form_pane_id = Some(pane_id);
        if self.mf_recovery.is_none() {
            self.mf_advanced = false;
            self.mf_kind = SshForwardKind::Local;
            self.cancel_edit(window, cx);
        }
        self.refresh(route);
        let inputs = [
            self.mf_bind_host.clone(),
            self.mf_bind_port.clone(),
            self.mf_target_host.clone(),
            self.mf_target_port.clone(),
            self.mf_description.clone(),
        ];
        self.mf_subs = inputs
            .iter()
            .map(|input| {
                cx.subscribe_in(
                    input,
                    window,
                    move |owner, _, ev: &InputEvent, window, cx| {
                        if let InputEvent::PressEnter { .. } = ev {
                            on_submit(owner, pane_id, window, cx);
                        }
                    },
                )
            })
            .collect();
        inputs[0].update(cx, |input, cx| input.focus(window, cx));
    }

    /// The workbench restores content focus only when a form actually closed.
    pub(crate) fn close(&mut self, window: &mut Window, cx: &mut App) -> bool {
        let was_open = self.form_pane_id.take().is_some();
        self.mf_subs.clear();
        if self.mf_recovery.is_none() {
            self.cancel_edit(window, cx);
        }
        was_open
    }
}
