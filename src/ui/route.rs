//! The shared pane/workspace/forward/SFTP route values live in
//! `lyrel-ui-base`; this facade keeps `crate::ui::route::` working.

pub use lyrel_ui_base::route::{
    ForwardOwnerKey, ForwardRoute, PaneRoute, PaneWorkspace, SftpRoute,
};
