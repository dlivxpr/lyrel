use lyrel_core::core::machine::{Axis, Machine, PaneNode, PaneRecord, Tab, TabId, Workspace};
use lyrel_core::core::session::WorkspaceId;

pub fn two_workspace_machine() -> Machine {
    let api = Workspace {
        id: WorkspaceId::new(),
        name: Some("api".into()),
        last_active: 0,
        tabs: vec![
            Tab {
                id: TabId::new(),
                name: Some("build".into()),
                group: None,
                last_auto: None,
                root: PaneNode::Leaf { pane: 1 },
                presentation: Default::default(),
                hibernated: false,
            },
            Tab {
                id: TabId::new(),
                name: None,
                group: None,
                last_auto: None,
                root: PaneNode::Split {
                    axis: Axis::Horizontal,
                    ratio: 0.5,
                    a: Box::new(PaneNode::Leaf { pane: 2 }),
                    b: Box::new(PaneNode::Leaf { pane: 3 }),
                },
                presentation: Default::default(),
                hibernated: false,
            },
        ],
        active_tab: None,
        attachment: None,
        groups: Default::default(),
        closed: Vec::new(),
        foreign_view: None,
    };
    let web = Workspace {
        id: WorkspaceId::new(),
        name: Some("web".into()),
        last_active: 0,
        tabs: vec![Tab {
            id: TabId::new(),
            name: None,
            group: None,
            last_auto: None,
            root: PaneNode::Leaf { pane: 5 },
            presentation: Default::default(),
            hibernated: false,
        }],
        active_tab: None,
        attachment: None,
        groups: Default::default(),
        closed: Vec::new(),
        foreign_view: None,
    };
    let record = |id: u64, cwd: &str| PaneRecord {
        id,
        cwd: Some(cwd.to_string()),
        title: String::new(),
        osc_title: None,
        ssh_spec: None,
        agent: None,
        shell: None,
        live: true,
    };
    Machine {
        workspaces: vec![api, web],
        panes: vec![
            record(1, "C:\\proj"),
            record(2, "C:\\proj"),
            record(3, "C:\\proj\\sub"),
            record(5, "C:\\web"),
        ],
    }
}

/// The owning daemon keeps only the local native SSH leaf in its ordinary tree.
pub fn mixed_sidecar_projection() -> (Workspace, PaneRecord) {
    use lyrel_core::core::pane_sidecar::{ForeignWorkspace, ScopedPane, SidecarUpdate};
    use lyrel_core::core::session::{RemoteRef, RemoteTarget};

    let remote = RemoteRef::new(
        RemoteTarget::Alias {
            alias: "remote-workbench".into(),
        },
        WorkspaceId::new(),
    );
    let mut mixed = Tab::leaf(ScopedPane::workspace(1));
    mixed.root = PaneNode::Split {
        axis: Axis::Horizontal,
        ratio: 0.5,
        a: Box::new(PaneNode::Leaf {
            pane: ScopedPane::workspace(1),
        }),
        b: Box::new(PaneNode::Leaf {
            pane: ScopedPane::client_local(77),
        }),
    };
    let projection = mixed
        .map_panes(|pane| pane.owner().is_client_local().then_some(pane.pane()))
        .unwrap();
    let mut foreign = ForeignWorkspace::new(remote);
    foreign.apply(&[SidecarUpdate::new(None, mixed)]).unwrap();
    (
        Workspace {
            name: Some("internal-projection".into()),
            tabs: vec![projection],
            foreign_view: Some(foreign),
            ..Workspace::default()
        },
        PaneRecord {
            live: true,
            ssh_spec: Some(
                serde_json::from_value(serde_json::json!({
                    "host": "native-box",
                    "port": 22,
                    "user": "developer",
                    "auth_mode": "agent"
                }))
                .unwrap(),
            ),
            ..PaneRecord::new(77)
        },
    )
}
