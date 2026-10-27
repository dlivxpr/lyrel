//! Keyboard shortcuts: every action in the palette's groups, each with its
//! keys as keycaps. Click the keys to record new ones; a chord another action
//! already has asks before it is taken over.

use super::kit::{BtnKind, Tk, fs};
use super::*;

impl SettingsWindow {
    pub(crate) fn render_settings_keybindings(&self, cx: &mut Context<Self>) -> AnyElement {
        let tk = Tk::of(cx);
        let Some(s) = self.active_settings() else {
            return div().into_any_element();
        };
        let search = s.shortcut_search.clone();
        let query = search.read(cx).value().trim().to_lowercase();
        let recording = s
            .recording
            .as_ref()
            .map(|r| (r.action.clone(), r.chords.clone()));
        let conflict = s
            .kb_conflict
            .as_ref()
            .map(|c| (c.action.clone(), c.spec.clone(), c.other.clone()));
        let note = s.rebinding_note.clone();

        let (preset, prefix, overridden) = {
            let cfg = cx.global::<Config>();
            let overridden: std::collections::HashSet<String> =
                cfg.keybindings.keys().cloned().collect();
            (
                cfg.keybinding_preset.clone(),
                cfg.prefix.clone(),
                overridden,
            )
        };
        let tmux = preset == "tmux";
        let changed = overridden.len();
        let effective = crate::ui::keymap::effective_chords(cx);

        // The toolbar: the filter, a way back from every change at once, and
        // the preset the whole table starts from.
        let presets = self.settings_choice(
            "kb-preset",
            &[t(L10nKey::SettingsDefault), "tmux"],
            usize::from(tmux),
            cx,
            |this, ix, _w, cx| {
                this.set_keybinding_preset(if ix == 0 { "default" } else { "tmux" }, cx)
            },
        );
        let toolbar =
            h_flex()
                .gap(px(12.))
                .items_center()
                .child(
                    kit::search_field(&search, &tk)
                        .h(px(28.))
                        .flex_1()
                        .max_w(px(300.)),
                )
                .child(div().flex_1())
                .when(changed > 0, |r| {
                    r.child(
                        kit::button(
                            "kb-restore-all",
                            t_plural(L10nKey::SettingsRestoreChanged, changed, &[]),
                            BtnKind::Link,
                        )
                        .on_click(cx.listener(|this, _, _w, cx| {
                            this.restore_default_keybindings_confirmed(cx)
                        })),
                    )
                })
                .child(presets);
        let hint = div()
            .text_size(fs(12.))
            .line_height(px(17.))
            .text_color(tk.k45)
            .child(if tmux {
                t(L10nKey::SettingsShortcutsHintTmux)
            } else {
                t(L10nKey::SettingsShortcutsHint)
            });
        let prefix_row = tmux.then(|| {
            h_flex()
                .gap(px(10.))
                .items_center()
                .child(
                    div()
                        .text_size(fs(12.))
                        .text_color(tk.k5)
                        .child(t(L10nKey::SettingsPrefix)),
                )
                .child(self.settings_choice(
                    "kb-prefix",
                    &["⌃B", "⌃A"],
                    usize::from(prefix == "ctrl-a"),
                    cx,
                    |this, ix, _w, cx| {
                        this.set_keybinding_prefix(if ix == 0 { "ctrl-b" } else { "ctrl-a" }, cx)
                    },
                ))
        });
        let intro = v_flex()
            .mt(px(-12.))
            .gap(px(10.))
            .child(toolbar)
            .child(hint)
            .children(prefix_row)
            .when_some(note, |v, note| {
                v.child(div().text_size(fs(12.)).text_color(tk.fg).child(note))
            })
            .into_any_element();

        // The rows, in the palette's groups.
        let mut grouped: Vec<(
            crate::ui::search::CommandGroup,
            Vec<(String, Vec<String>, String)>,
        )> = Vec::new();
        for (action, keys) in effective {
            let (group, label) = crate::ui::keymap::action_entry(&action);
            if !query.is_empty()
                && !keybinding_matches_query(&action, &query)
                && !label.to_lowercase().contains(&query)
            {
                continue;
            }
            match grouped.iter_mut().find(|(g, _)| *g == group) {
                Some((_, rows)) => rows.push((action, keys, label)),
                None => grouped.push((group, vec![(action, keys, label)])),
            }
        }
        grouped.sort_by_key(|(g, _)| {
            crate::ui::search::CommandGroup::ORDER
                .iter()
                .position(|o| o == g)
                .unwrap_or(usize::MAX)
        });

        let mut groups = vec![intro];
        let none = grouped.is_empty();
        for (group, rows) in grouped {
            let mut section = v_flex().child(
                div()
                    .pb(px(6.))
                    .text_size(fs(11.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(tk.heading)
                    .child(group.title()),
            );
            for (action, keys, label) in rows {
                let is_recording = recording.as_ref().is_some_and(|(a, _)| *a == action);
                let conflict_here = conflict.as_ref().filter(|(a, _, _)| *a == action);
                let shown: Vec<String> = match (&recording, conflict_here) {
                    (Some((a, chords)), _) if *a == action => vec![chords.join(" ")]
                        .into_iter()
                        .filter(|c| !c.is_empty())
                        .collect(),
                    (_, Some((_, spec, _))) => vec![spec.clone()],
                    _ => keys,
                };
                // "New Tab, ⌘T": the action and what it is bound to, the way
                // the row reads — a screen reader says ⌘ as "command".
                let a11y_keys = match shown.is_empty() {
                    true => "—".to_string(),
                    false => shown
                        .iter()
                        .map(|spec| crate::ui::keymap::key_tokens(spec).join(""))
                        .collect::<Vec<_>>()
                        .join(" "),
                };
                let a11y_name = format!("{label}, {a11y_keys}");
                let modified = overridden.contains(&action) && !is_recording;
                let ring = if is_recording {
                    vec![kit::ring(tk.fg, 1., true)]
                } else if conflict_here.is_some() {
                    vec![kit::ring(tk.warn, 1., true)]
                } else {
                    Vec::new()
                };
                let hover_ring = if is_recording || conflict_here.is_some() {
                    vec![kit::ring(tk.fg, 1., true)]
                } else {
                    vec![kit::ring(tk.k15, 0.5, true)]
                };
                let record_action = action.clone();
                let keys_button = h_flex()
                    .id(SharedString::from(format!("kb-{action}")))
                    .role(gpui_kit::Role::Button)
                    .aria_label(a11y_name)
                    .h(px(26.))
                    .min_w(px(72.))
                    .pl(px(8.))
                    .pr(px(4.))
                    .gap(px(5.))
                    .items_center()
                    .justify_end()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(is_recording, |b| b.bg(tk.page))
                    .shadow(ring)
                    .hover(move |b| b.shadow(hover_ring.clone()))
                    .when(is_recording && shown.is_empty(), |b| {
                        b.child(
                            div()
                                .pr(px(2.))
                                .text_size(fs(12.))
                                .text_color(tk.k45)
                                .child(t(L10nKey::SettingsPressKeysShort)),
                        )
                    })
                    .children(shown.iter().enumerate().map(|(n, spec)| {
                        h_flex()
                            .gap(px(5.))
                            .items_center()
                            .when(n > 0, |d| {
                                d.child(div().text_size(fs(11.)).text_color(tk.k4).child("/"))
                            })
                            .child(Self::keycap_row(spec, &tk))
                    }))
                    .when(!is_recording && shown.is_empty(), |b| {
                        b.child(
                            div()
                                .pr(px(4.))
                                .text_size(fs(12.))
                                .text_color(tk.k3)
                                .child("—"),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let already = this
                            .active_settings()
                            .and_then(|s| s.recording.as_ref())
                            .is_some_and(|r| r.action == record_action);
                        if already {
                            if let Some(s) = this.active_settings_mut() {
                                s.recording = None;
                            }
                            cx.notify();
                        } else {
                            this.start_recording_key(record_action.clone(), window, cx)
                        }
                    }));
                let reset_action = action.clone();
                let row = h_flex()
                    .h(px(34.))
                    .px(px(8.))
                    .gap(px(16.))
                    .items_center()
                    .justify_between()
                    .rounded(px(7.))
                    .when(conflict_here.is_none(), |r| r.hover(move |s| s.bg(tk.k04)))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(8.))
                            .items_center()
                            .child(div().min_w_0().truncate().child(label))
                            .when(modified, |r| {
                                r.child(
                                    div()
                                        .id(SharedString::from(format!("reset-{action}")))
                                        .flex_shrink_0()
                                        .text_size(fs(11.5))
                                        .text_color(tk.k4)
                                        .cursor_pointer()
                                        .hover(move |s| s.text_color(tk.fg))
                                        .child(t(L10nKey::Reset))
                                        .on_click(cx.listener(move |this, _, _w, cx| {
                                            this.reset_keybinding(reset_action.clone(), cx)
                                        })),
                                )
                            }),
                    )
                    .child(keys_button);
                let conflict_line = conflict_here.map(|(_, spec, other)| {
                    let keys = crate::ui::keymap::key_chords(spec)
                        .into_iter()
                        .map(|chord| chord.concat())
                        .collect::<Vec<_>>()
                        .join(&format!(" {} ", t(L10nKey::SettingsKeyThen)));
                    let other = crate::ui::keymap::action_entry(other)
                        .1
                        .trim_end_matches('…')
                        .to_string();
                    h_flex()
                        .px(px(8.))
                        .pt(px(2.))
                        .pb(px(10.))
                        .gap(px(8.))
                        .items_center()
                        .text_size(fs(12.))
                        .text_color(tk.k6)
                        .child(
                            div()
                                .size(px(6.))
                                .flex_shrink_0()
                                .rounded_full()
                                .bg(tk.warn),
                        )
                        .child(div().flex_1().min_w_0().child(t_fmt(
                            L10nKey::SettingsShortcutConflict,
                            &[("keys", &keys), ("action", &other)],
                        )))
                        .child(
                            kit::button(
                                "kb-conflict-replace",
                                t(L10nKey::SettingsReplace),
                                BtnKind::Primary,
                            )
                            .text_size(fs(12.))
                            .on_click(cx.listener(
                                |this, _, _w, cx| this.resolve_keybinding_conflict(true, cx),
                            )),
                        )
                        .child(
                            kit::button(
                                "kb-conflict-cancel",
                                t(L10nKey::Cancel),
                                BtnKind::Secondary,
                            )
                            .text_size(fs(12.))
                            .on_click(cx.listener(
                                |this, _, _w, cx| this.resolve_keybinding_conflict(false, cx),
                            )),
                        )
                });
                section = section.child(
                    v_flex()
                        .mx(px(-8.))
                        .rounded(px(7.))
                        .when(conflict_line.is_some(), |d| d.bg(tk.k04))
                        .child(row)
                        .children(conflict_line),
                );
            }
            groups.push(section.into_any_element());
        }
        if none {
            groups.push(
                div()
                    .text_color(tk.k35)
                    .child(t_fmt(L10nKey::SettingsNoActionsMatch, &[("query", &query)]))
                    .into_any_element(),
            );
        }
        Self::settings_page(groups)
    }
}

use super::kit;
pub(crate) const RECORD_COMMIT_DELAY_MS: u64 = 650;

impl SettingsWindow {
    pub(crate) fn start_recording_key(
        &mut self,
        action: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.weak_entity();
        let intercept = cx.intercept_keystrokes(move |ev, _window, cx| {
            let keystroke = ev.keystroke.clone();
            let _ = this.update(cx, |this, cx| this.on_record_key(&keystroke, cx));
            cx.stop_propagation();
        });
        self.record_gen = self.record_gen.wrapping_add(1);
        if let Some(s) = self.active_settings_mut() {
            s.rebinding_note = None;
            s.kb_conflict = None;
            s.recording = Some(Recording {
                action,
                chords: Vec::new(),
                _intercept: intercept,
            });
        }
        cx.notify();
    }

    fn on_record_key(&mut self, keystroke: &gpui_kit::Keystroke, cx: &mut Context<Self>) {
        let Some((action, has_chords)) = self
            .active_settings()
            .and_then(|s| s.recording.as_ref())
            .map(|r| (r.action.clone(), !r.chords.is_empty()))
        else {
            return;
        };
        match keystroke.key.as_str() {
            "escape" => {
                self.stop_recording(cx);
                return;
            }
            "backspace" | "delete" => {
                if has_chords {
                    if let Some(r) = self
                        .active_settings_mut()
                        .and_then(|s| s.recording.as_mut())
                    {
                        r.chords.pop();
                    }
                    let still_has = self
                        .active_settings()
                        .and_then(|s| s.recording.as_ref())
                        .is_some_and(|r| !r.chords.is_empty());
                    if still_has {
                        self.schedule_recording_commit(cx);
                    } else {
                        self.record_gen = self.record_gen.wrapping_add(1);
                    }
                    cx.notify();
                } else {
                    self.stop_recording(cx);
                    self.unbind_keybinding(action, cx);
                }
                return;
            }
            _ => {}
        }
        let Some(spec) = crate::ui::keymap::spec_from_keystroke(keystroke) else {
            return;
        };
        if let Some(r) = self
            .active_settings_mut()
            .and_then(|s| s.recording.as_mut())
        {
            r.chords.push(spec);
        }
        self.schedule_recording_commit(cx);
        cx.notify();
    }

    fn schedule_recording_commit(&mut self, cx: &mut Context<Self>) {
        self.record_gen = self.record_gen.wrapping_add(1);
        let generation = self.record_gen;
        cx.spawn(async move |this, cx| {
            smol::Timer::after(std::time::Duration::from_millis(RECORD_COMMIT_DELAY_MS)).await;
            let _ = this.update(cx, |this, cx| {
                if this.record_gen == generation {
                    this.commit_recording(cx);
                }
            });
        })
        .detach();
    }

    fn commit_recording(&mut self, cx: &mut Context<Self>) {
        let Some((action, chords)) = self
            .active_settings()
            .and_then(|s| s.recording.as_ref())
            .filter(|r| !r.chords.is_empty())
            .map(|r| (r.action.clone(), r.chords.clone()))
        else {
            return;
        };
        self.stop_recording(cx);
        let spec = chords.join(" ");
        // A chord another action answers to is not taken silently: the row
        // asks whether to take it over, and only Replace moves it.
        if let Some(other) = Self::keybinding_owner(&action, &spec, cx) {
            if let Some(s) = self.active_settings_mut() {
                s.kb_conflict = Some(crate::ui::settings::KeyConflict {
                    action,
                    spec,
                    other,
                });
            }
            cx.notify();
            return;
        }
        self.assign_keybinding(action, spec, cx);
    }

    /// The other action that already answers to `spec`, if any.
    fn keybinding_owner(action: &str, spec: &str, cx: &App) -> Option<String> {
        use crate::ui::keymap::same_chord;
        crate::ui::keymap::effective_chords(cx)
            .into_iter()
            .find(|(a, chords)| a != action && chords.iter().any(|k| same_chord(k, spec)))
            .map(|(a, _)| a)
            .or_else(|| {
                crate::ui::keymap::extra_bindings(cx)
                    .into_iter()
                    .find(|(a, k)| a != action && same_chord(k, spec))
                    .map(|(a, _)| a)
            })
    }

    /// Takes the conflicting chord over, as the row's Replace button says.
    pub(crate) fn resolve_keybinding_conflict(&mut self, replace: bool, cx: &mut Context<Self>) {
        let Some(conflict) = self
            .active_settings_mut()
            .and_then(|s| s.kb_conflict.take())
        else {
            return;
        };
        if replace {
            self.assign_keybinding(conflict.action, conflict.spec, cx);
            if let Some(s) = self.active_settings_mut() {
                // The row already said what Replace would do.
                s.rebinding_note = None;
            }
        }
        cx.notify();
    }

    fn stop_recording(&mut self, cx: &mut Context<Self>) {
        self.record_gen = self.record_gen.wrapping_add(1);
        if let Some(s) = self.active_settings_mut() {
            s.recording = None;
        }
        cx.notify();
    }

    fn assign_keybinding(&mut self, action: String, spec: String, cx: &mut Context<Self>) {
        // Compared as chords, not as spellings: a recorded `secondary-}` and a
        // config's `secondary-shift-]` are one keystroke written two ways, and
        // only `same_chord` sees it. Compared as text, the displacement never
        // fires and both bindings survive onto that keystroke, where which one
        // wins is arbitrary (#750).
        //
        // Only that chord moves: the action that had it keeps any others it
        // has, since an action can carry several (#868) and emptying it would
        // take away keys that were never on this keystroke. An extra default —
        // Alt+Enter beside Shift+Enter — follows its action's first chord rather
        // than being one of its own, so its owner is unbound outright, as it
        // always was.
        use crate::ui::keymap::same_chord;
        let displaced: Option<(String, Vec<String>)> = crate::ui::keymap::effective_chords(cx)
            .into_iter()
            .find(|(a, chords)| *a != action && chords.iter().any(|k| same_chord(k, &spec)))
            .map(|(a, chords)| {
                let rest = chords.into_iter().filter(|k| !same_chord(k, &spec));
                (a, rest.collect())
            })
            .or_else(|| {
                crate::ui::keymap::extra_bindings(cx)
                    .into_iter()
                    .find(|(a, k)| *a != action && same_chord(k, &spec))
                    .map(|(a, _)| (a, Vec::new()))
            });
        // A trailing "…" on an action name marks a command that opens
        // something; it is not punctuation, and inside a sentence it reads as
        // the sentence trailing off — "Rename Tab… took the shortcut from".
        let in_prose = |name: &str| name.trim_end_matches('…').to_string();
        let note = displaced.as_ref().map(|(other, _)| {
            t_fmt(
                L10nKey::AppKeybindingDisplacedNote,
                &[
                    (
                        "action",
                        &in_prose(&crate::ui::keymap::action_entry(&action).1),
                    ),
                    (
                        "previous",
                        &in_prose(&crate::ui::keymap::action_entry(other).1),
                    ),
                ],
            )
        });
        // Both written as lists. Recording a shortcut sets it — the row showed
        // one chord and now shows another — and a bare string in config adds a
        // chord beside the default instead (#868).
        self.update_config(cx, |cfg| {
            use crate::core::config::KeybindingOverride;
            if let Some((other, rest)) = &displaced {
                cfg.keybindings
                    .insert(other.clone(), KeybindingOverride::Exact(rest.clone()));
            }
            cfg.keybindings
                .insert(action, KeybindingOverride::Exact(vec![spec]));
        });
        crate::ui::keymap::rebind(cx);
        if let Some(s) = self.active_settings_mut() {
            s.rebinding_note = note;
        }
        cx.notify();
    }

    /// Takes every chord off an action, and keeps it off.
    ///
    /// ⌫ on a row that has recorded nothing used to *reset* it — drop the
    /// override so the action gets its shipped chord back. On a row nobody has
    /// overridden, which is every row the first time it is looked at, that is a
    /// no-op: someone pressing Backspace over Alt+1 to be rid of it watched
    /// Alt+1 sit exactly where it was and read it as the default restoring
    /// itself (#901). Nothing anywhere in the app said "this action should have
    /// no key", though `config.json` has spelled it `[]` since #868.
    ///
    /// So ⌫ writes that empty list, and the **Reset** button beside the row —
    /// which appears the moment an action is overridden, this way included — is
    /// the way back to the default.
    pub(crate) fn unbind_keybinding(&mut self, action: String, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| {
            cfg.keybindings.insert(
                action,
                crate::core::config::KeybindingOverride::Exact(Vec::new()),
            );
        });
        crate::ui::keymap::rebind(cx);
        if let Some(s) = self.active_settings_mut() {
            s.recording = None;
            s.rebinding_note = None;
        }
        cx.notify();
    }

    pub(crate) fn reset_keybinding(&mut self, action: String, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| {
            cfg.keybindings.remove(&action);
        });
        crate::ui::keymap::rebind(cx);
        if let Some(s) = self.active_settings_mut() {
            s.recording = None;
            s.rebinding_note = None;
        }
        cx.notify();
    }

    pub(crate) fn restore_default_keybindings_confirmed(&mut self, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.keybindings.clear());
        crate::ui::keymap::rebind(cx);
        if let Some(s) = self.active_settings_mut() {
            s.recording = None;
            s.rebinding_note = None;
        }
        cx.notify();
    }

    pub(crate) fn set_keybinding_preset(&mut self, preset: &str, cx: &mut Context<Self>) {
        let preset = preset.to_string();
        self.update_config(cx, |cfg| cfg.keybinding_preset = preset);
        crate::ui::keymap::rebind(cx);
        if let Some(s) = self.active_settings_mut() {
            s.recording = None;
            s.rebinding_note = None;
        }
        cx.notify();
    }

    pub(crate) fn set_keybinding_prefix(&mut self, prefix: &str, cx: &mut Context<Self>) {
        let prefix = prefix.to_string();
        self.update_config(cx, |cfg| cfg.prefix = prefix);
        crate::ui::keymap::rebind(cx);
        cx.notify();
    }
}
