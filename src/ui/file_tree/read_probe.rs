//! Test-only evidence that a real HostOps completion reached its UI callback.
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ReadKind {
    Directory,
    Names,
    Contents,
    Index,
    Root,
}

/// An opaque address ticket, never an owning reference to a retired adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InstanceTicket(usize);

impl InstanceTicket {
    pub(crate) fn of<T: ?Sized>(host: &Arc<T>) -> Self {
        Self(Arc::as_ptr(host) as *const () as usize)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Landing {
    sequence: u64,
    instance: InstanceTicket,
}

#[derive(Default)]
pub(crate) struct ReadProbe {
    latest: HashMap<ReadKind, Landing>,
}

impl ReadProbe {
    pub(crate) fn checkpoint(&self, kind: ReadKind) -> Option<Landing> {
        self.latest.get(&kind).copied()
    }

    pub(crate) fn landed_since(
        &self,
        kind: ReadKind,
        instance: InstanceTicket,
        before: Option<Landing>,
    ) -> bool {
        self.checkpoint(kind)
            .is_some_and(|landing| Some(landing) != before && landing.instance == instance)
    }

    pub(crate) fn record(&mut self, kind: ReadKind, instance: InstanceTicket) {
        let sequence = self.latest.get(&kind).map_or(1, |last| last.sequence + 1);
        self.latest.insert(kind, Landing { sequence, instance });
    }
}
