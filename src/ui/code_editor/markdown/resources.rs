use lyrel_core::host::SharedHost;
use std::path::{Path, PathBuf};

/// The document owns this identity; presentations and pending resource jobs only
/// capture snapshots. Never resolve a document resource through the active tab.
#[derive(Clone)]
pub(crate) struct ResourceContext {
    host: SharedHost,
    path: Option<PathBuf>,
    epoch: u64,
}
impl ResourceContext {
    pub(crate) fn host(&self) -> &SharedHost {
        &self.host
    }
    pub(crate) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }
    pub(crate) fn is_current(&self, current: Option<&Self>) -> bool {
        current.is_some_and(|current| {
            current.epoch == self.epoch
                && current.host.id() == self.host.id()
                && current.path == self.path
        })
    }
}
impl super::MarkdownDocument {
    pub(crate) fn resources(&self) -> Option<&ResourceContext> {
        self.resources.as_ref()
    }
    pub(crate) fn set_resource_context(
        &mut self,
        host: SharedHost,
        path: Option<PathBuf>,
        cx: &mut gpui_kit::Context<Self>,
    ) {
        if self
            .resources
            .as_ref()
            .is_some_and(|old| old.host.id() == host.id() && old.path == path)
        {
            return;
        }
        let epoch = self
            .resources
            .as_ref()
            .map_or(1, |old| old.epoch.wrapping_add(1));
        self.resources = Some(ResourceContext { host, path, epoch });
        self.images = super::images::Images::default();
        cx.emit(super::DocumentEvent::ResourceContextChanged);
        cx.notify();
    }
}
