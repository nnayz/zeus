use std::collections::VecDeque;

use diri_proto::SessionId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidencyUpdate {
    pub resident: Vec<SessionId>,
    /// The caller must detach this session's live `SessionAttachment`.
    pub evicted: Option<SessionId>,
}

/// Bounded most-recently-used set of sessions allowed to retain live attachments.
#[derive(Clone, Debug)]
pub struct TerminalResidency {
    capacity: usize,
    order: VecDeque<SessionId>,
}

impl Default for TerminalResidency {
    fn default() -> Self {
        // Only the selected terminal is visible. Keeping background data
        // channels and grid buffers alive made one-window memory scale with
        // navigation history instead of what is on screen.
        Self::new(1)
    }
}

impl TerminalResidency {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::new(),
        }
    }

    pub fn touch(&mut self, id: SessionId) -> ResidencyUpdate {
        self.order.retain(|candidate| candidate != &id);
        self.order.push_front(id);
        let evicted = (self.order.len() > self.capacity)
            .then(|| self.order.pop_back())
            .flatten();
        ResidencyUpdate {
            resident: self.order.iter().cloned().collect(),
            evicted,
        }
    }

    pub fn remove(&mut self, id: &SessionId) -> bool {
        let old_len = self.order.len();
        self.order.retain(|candidate| candidate != id);
        self.order.len() != old_len
    }

    pub fn contains(&self, id: &SessionId) -> bool {
        self.order.contains(id)
    }

    pub fn resident(&self) -> impl Iterator<Item = &SessionId> {
        self.order.iter()
    }
}
