//! Observed pane facts use the store's existing deferred-write policy.

use std::io;
use std::sync::{Arc, Mutex};

use crate::core::machine::{LayoutDelta, PaneRecord};
use crate::core::session::WorkspaceId;

use super::{MachineStore, Persist};

impl MachineStore {
    pub fn note_pane_facts(self: &Arc<Self>, pane: u64, update: impl FnOnce(&mut PaneRecord)) {
        self.ensure_flusher();
        let result: io::Result<()> = self.mutate_with(None, Persist::Soon, |m| {
            let Some(record) = m.panes.iter_mut().find(|p| p.id == pane) else {
                return Ok(((), Vec::new()));
            };
            let before = record.clone();
            update(record);
            record.id = before.id;
            if *record == before {
                return Ok(((), Vec::new()));
            }
            let record = record.clone();
            let workspaces: Vec<WorkspaceId> = m
                .workspaces
                .iter()
                .filter(|w| w.tabs.iter().any(|t| t.root.contains(pane)))
                .map(|w| w.id)
                .collect();
            Ok((
                (),
                workspaces
                    .into_iter()
                    .map(|w| {
                        (
                            w,
                            LayoutDelta::PaneFacts {
                                pane: record.clone(),
                            },
                        )
                    })
                    .collect(),
            ))
        });
        if let Err(e) = result {
            log::warn!("could not record facts about pane {pane}: {e}");
        }
    }
}

static OBSERVED: Mutex<Option<Arc<MachineStore>>> = Mutex::new(None);

pub fn publish_observations(store: &Arc<MachineStore>) {
    *OBSERVED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(store));
}

pub fn observe_pane(pane: u64, f: impl FnOnce(&mut PaneRecord)) {
    let store = OBSERVED.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(store) = store {
        store.note_pane_facts(pane, f);
    }
}

pub fn observed_store() -> Option<Arc<MachineStore>> {
    OBSERVED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
pub(crate) fn withdraw_observations() {
    *OBSERVED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg(test)]
pub(crate) static OBSERVE_SLOT: Mutex<()> = Mutex::new(());
