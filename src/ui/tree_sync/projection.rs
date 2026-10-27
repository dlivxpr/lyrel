//! Projects borrowed GUI tabs into machine-owned desired state; no queue or mirror access.
use gpui_kit::App;
use lyrel_core::core::machine::{AgentFacts, Axis as TreeAxis, PaneNode, PaneSeed, TabId};

use crate::core::session::{SessionPane, WorkspaceId, WorkspaceStore};
use crate::ui::app::Tab;
use crate::ui::pane::{Pane, PaneSlot};

use super::desired::{DesiredNode, DesiredTab};
pub(super) fn slot_pane_id(slot: &PaneSlot, cx: &App) -> Option<u64> {
    match slot {
        PaneSlot::Ready(view) => Some(view.read(cx).pane_id),
        PaneSlot::Connecting(pending) => pending.read(cx).spawn.restore_pane,
    }
}

pub(super) fn logical_node(pane: &Pane, remote_window: bool, cx: &App) -> Option<PaneNode> {
    match pane {
        Pane::Leaf(slot) => {
            if remote_window
                && matches!(slot, PaneSlot::Ready(view) if view.read(cx).ssh_spec().is_some())
            {
                return None;
            }
            slot_pane_id(slot, cx).map(|pane| PaneNode::Leaf { pane })
        }
        Pane::Split {
            axis, ratio, a, b, ..
        } => {
            match (
                logical_node(a, remote_window, cx),
                logical_node(b, remote_window, cx),
            ) {
                (Some(a), Some(b)) => Some(PaneNode::Split {
                    axis: match axis {
                        gpui_kit::Axis::Horizontal => TreeAxis::Horizontal,
                        gpui_kit::Axis::Vertical => TreeAxis::Vertical,
                    },
                    ratio: ratio.get(),
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (one, other) => one.or(other),
            }
        }
        Pane::Empty => None,
    }
}
pub(crate) fn desired_tabs(
    tabs: &[Tab],
    active_index: usize,
    workspace: WorkspaceId,
    cx: &App,
) -> (Vec<DesiredTab>, Option<TabId>, Vec<TabId>) {
    let remote = WorkspaceStore::all(cx)
        .get(workspace)
        .is_some_and(|w| w.is_remote());
    let mut out = Vec::new();
    let mut active = None;
    let mut held = Vec::new();
    for (index, tab) in tabs.iter().enumerate() {
        // A sleeping tab has nothing on screen to read its layout off, so it
        // is read off what it will wake into: the same panes, by the same ids,
        // which is what keeps the diff below from taking it for a tab that
        // lost them all.
        let root = match tab.asleep_layout() {
            Some(layout) => desired_node_from_session(layout, remote),
            None => desired_node(&tab.pane, remote, cx),
        };
        let Some(root) = root else {
            if !(remote && every_leaf_is_native_ssh(&tab.pane, cx)) {
                held.push(tab.tree_id.get());
            }
            continue;
        };
        let id = tab.tree_id.get();
        if index == active_index {
            active = Some(id);
        }
        out.push(DesiredTab {
            id,
            name: tab.name.clone(),
            // As the tab has it, even when this window does not know the
            // group: a window whose copy of the groups has not landed yet
            // would otherwise send every tab it holds back to auto grouping.
            // A group that really is gone was already cleared by the machine.
            group: tab.group.get(),
            last_auto: tab.auto_group.borrow().clone(),
            presentation: {
                let mut presentation = tab.presentation.clone();
                presentation.remap_panes(|pane| root.seed_of(pane).map(|_| pane));
                presentation
            },
            presentation_fallback: tab.last_focused.and_then(|focused| {
                tab.pane
                    .leaves()
                    .into_iter()
                    .find(|slot| slot.entity_id() == focused)
                    .and_then(|slot| slot_pane_id(&slot, cx))
            }),
            root,
            hibernated: tab.asleep_layout().is_some(),
        });
    }
    (out, active, held)
}

/// [`desired_node`] for a layout that is only a record — a sleeping tab's.
/// A leaf with no id is one the machine never had, and a native SSH leaf in a
/// remote window lives on this client rather than on the window's machine;
/// neither is the machine's to hold, exactly as for a live tab.
fn desired_node_from_session(layout: &SessionPane, remote_window: bool) -> Option<DesiredNode> {
    match layout {
        SessionPane::Leaf {
            cwd,
            pane_id,
            shell,
            ssh_spec,
            agent,
            agent_session_id,
            agent_launch_argv,
            agent_session_scope,
        } => {
            let pane = (*pane_id)?;
            if remote_window && ssh_spec.is_some() {
                return None;
            }
            Some(DesiredNode::Leaf {
                pane,
                seed: PaneSeed {
                    pane,
                    cwd: cwd.as_ref().map(|p| p.to_string_lossy().into_owned()),
                    ssh_spec: ssh_spec.clone(),
                    agent: agent.map(|agent| AgentFacts {
                        agent,
                        session_id: agent_session_id.clone(),
                        launch_argv: agent_launch_argv.clone(),
                        session_scope: agent_session_scope
                            .as_ref()
                            .map(lyrel_core::host::session_operations::durable_scope),
                        status: None,
                    }),
                    shell: shell.clone(),
                },
            })
        }
        SessionPane::Split { axis, ratio, a, b } => {
            let left = desired_node_from_session(a, remote_window);
            let right = desired_node_from_session(b, remote_window);
            match (left, right) {
                (Some(a), Some(b)) => Some(DesiredNode::Split {
                    axis: match axis {
                        crate::core::session::SessionAxis::Horizontal => TreeAxis::Horizontal,
                        crate::core::session::SessionAxis::Vertical => TreeAxis::Vertical,
                    },
                    ratio: *ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (one, other) => one.or(other),
            }
        }
    }
}

fn every_leaf_is_native_ssh(pane: &Pane, cx: &App) -> bool {
    match pane {
        Pane::Leaf(PaneSlot::Ready(view)) => view.read(cx).ssh_spec().is_some(),
        Pane::Leaf(PaneSlot::Connecting(_)) | Pane::Empty => false,
        Pane::Split { a, b, .. } => {
            every_leaf_is_native_ssh(a, cx) && every_leaf_is_native_ssh(b, cx)
        }
    }
}

pub(crate) fn seed_for_slot(slot: &PaneSlot, cx: &App) -> Option<PaneSeed> {
    let node = desired_node(&Pane::Leaf(slot.clone()), false, cx)?;
    Some(node.first_leaf().1.clone())
}

fn desired_node(pane: &Pane, remote_window: bool, cx: &App) -> Option<DesiredNode> {
    match pane {
        Pane::Leaf(PaneSlot::Ready(view)) => {
            let view = view.read(cx);
            let ssh_spec = view.ssh_spec();
            if remote_window && ssh_spec.is_some() {
                return None;
            }
            let agent = view.agent().map(|agent| {
                let session = view.agent_session();
                AgentFacts {
                    agent,
                    session_id: session.as_ref().and_then(|s| s.session_id.clone()),
                    launch_argv: session.as_ref().and_then(|s| s.launch_argv.clone()),
                    session_scope: session
                        .as_ref()
                        .and_then(|s| s.readout.session_scope.as_ref())
                        .map(lyrel_core::host::session_operations::durable_scope),
                    status: None,
                }
            });
            Some(DesiredNode::Leaf {
                pane: view.pane_id,
                seed: PaneSeed {
                    pane: view.pane_id,
                    cwd: view
                        .spawnable_cwd()
                        .map(|p| p.to_string_lossy().into_owned()),
                    ssh_spec,
                    agent,
                    // Only a pane this window spawned knows this; one it
                    // attached to never saw the command line. The daemon fills
                    // that gap from its own side, so leaving it empty here
                    // withholds nothing the tree does not already get.
                    shell: view.shell_spec(),
                },
            })
        }
        Pane::Leaf(PaneSlot::Connecting(pending)) => {
            let spawn = &pending.read(cx).spawn;
            let pane = spawn.restore_pane?;
            let agent = spawn.agent.map(|agent| AgentFacts {
                agent,
                session_id: spawn.agent_session_id.clone(),
                launch_argv: spawn.agent_launch_argv.clone(),
                session_scope: spawn
                    .agent_session_scope
                    .as_ref()
                    .map(lyrel_core::host::session_operations::durable_scope),
                status: None,
            });
            Some(DesiredNode::Leaf {
                pane,
                seed: PaneSeed {
                    pane,
                    cwd: spawn
                        .working_directory
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    ssh_spec: None,
                    agent,
                    shell: spawn.shell.clone(),
                },
            })
        }
        Pane::Split {
            axis, a, b, ratio, ..
        } => {
            let left = desired_node(a, remote_window, cx);
            let right = desired_node(b, remote_window, cx);
            match (left, right) {
                (Some(a), Some(b)) => Some(DesiredNode::Split {
                    axis: match axis {
                        gpui_kit::Axis::Horizontal => TreeAxis::Horizontal,
                        gpui_kit::Axis::Vertical => TreeAxis::Vertical,
                    },
                    ratio: ratio.get(),
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (one, other) => one.or(other),
            }
        }
        Pane::Empty => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyrel_core::core::machine::PaneNode;
    /// The window reads a sleeping tab's layout off the record it kept, and
    /// has to hand the machine the very tab the machine already holds: same
    /// ids, same seeds, same shape.
    #[test]
    fn a_sleeping_layout_reads_back_as_the_tree_it_came_from() {
        use crate::core::session::SessionAxis;
        let leaf_of = |pane: u64, cwd: &str| SessionPane::Leaf {
            cwd: Some(std::path::PathBuf::from(cwd)),
            pane_id: Some(pane),
            shell: None,
            ssh_spec: None,
            agent: Some(crate::core::cli_agent::CLIAgent::OhMyPi),
            agent_session_id: Some(format!("sess-{pane}")),
            agent_launch_argv: None,
            agent_session_scope: None,
        };
        let layout = SessionPane::Split {
            axis: SessionAxis::Horizontal,
            ratio: 0.3,
            a: Box::new(leaf_of(3, "/a")),
            b: Box::new(leaf_of(4, "/b")),
        };
        let node = desired_node_from_session(&layout, false).expect("both leaves have ids");
        assert_eq!(
            node.to_pane_node(),
            PaneNode::Split {
                axis: TreeAxis::Horizontal,
                ratio: 0.3,
                a: Box::new(PaneNode::Leaf { pane: 3 }),
                b: Box::new(PaneNode::Leaf { pane: 4 }),
            }
        );
        let seed = node.seed_of(4).expect("pane 4 is a leaf");
        assert_eq!(seed.cwd.as_deref(), Some("/b"));
        assert_eq!(
            seed.agent.as_ref().and_then(|a| a.session_id.as_deref()),
            Some("sess-4"),
            "the session a wake resumes stays on the record"
        );

        // A leaf the machine never had is left out, as for a live tab.
        let unknown = SessionPane::Split {
            axis: SessionAxis::Vertical,
            ratio: 0.5,
            a: Box::new(leaf_of(3, "/a")),
            b: Box::new(SessionPane::Leaf {
                cwd: None,
                pane_id: None,
                shell: None,
                ssh_spec: None,
                agent: None,
                agent_session_id: None,
                agent_launch_argv: None,
                agent_session_scope: None,
            }),
        };
        assert_eq!(
            desired_node_from_session(&unknown, false).map(|n| n.to_pane_node()),
            Some(PaneNode::Leaf { pane: 3 })
        );
    }

}
