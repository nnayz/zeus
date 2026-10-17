//! Keyboard-first state for the needs-you inbox overlay.
//!
//! The queue itself — classification, dismiss, snooze — lives in
//! `taskmaster_proto::inbox` and is a pure fold over session records. This module is
//! only the overlay's navigation state, kept free of GPUI so the triage keys
//! are unit-tested without a window.

use taskmaster_proto::SessionId;

/// A key event already interpreted for the inbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboxKey {
    Down,
    Up,
    /// Return: go to the session.
    Jump,
    /// Answer a captured permission the same way its notification would.
    Approve,
    Deny,
    /// Hide the item for a while; it returns when the snooze expires.
    Snooze,
    /// Hide the item until the session produces something new.
    Dismiss,
    Escape,
    Other,
}

/// What a triage key asks the store to do, once it has an item to act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboxAction {
    Jump,
    Approve,
    Deny,
    Snooze,
    Dismiss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboxOutcome {
    /// The inbox is closed and does not own this key.
    Ignored,
    /// The key belongs to the open inbox but asked for nothing.
    Consumed,
    /// The highlight moved (or the overlay closed): repaint.
    Changed,
    /// Act on the highlighted item.
    Act(InboxAction),
}

/// The overlay's position, nothing more. Items are rebuilt from the queue on
/// every render; this only remembers where the user is and whether the inbox
/// is up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InboxSurfaceState {
    visible: bool,
    highlight: usize,
}

impl InboxSurfaceState {
    pub const fn is_visible(&self) -> bool {
        self.visible
    }

    pub const fn highlight(&self) -> usize {
        self.highlight
    }

    /// Opens at the top; returns whether it became visible.
    pub fn open(&mut self) -> bool {
        let opened = !self.visible;
        self.visible = true;
        self.highlight = 0;
        opened
    }

    pub fn close(&mut self) -> bool {
        std::mem::take(&mut self.visible)
    }

    /// Clamp the highlight to a queue that shrank while the inbox was open.
    pub fn reconcile(&mut self, count: usize) {
        self.highlight = self.highlight.min(count.saturating_sub(1));
    }

    /// Routes one key. Non-navigation actions never close the overlay: triage
    /// is a run of decisions, and only a jump changes context.
    pub fn handle(&mut self, key: InboxKey, count: usize) -> InboxOutcome {
        if !self.visible {
            return InboxOutcome::Ignored;
        }
        self.reconcile(count);
        match key {
            InboxKey::Escape => {
                self.close();
                InboxOutcome::Changed
            }
            InboxKey::Down => {
                if count == 0 {
                    InboxOutcome::Consumed
                } else {
                    self.highlight = (self.highlight + 1) % count;
                    InboxOutcome::Changed
                }
            }
            InboxKey::Up => {
                if count == 0 {
                    InboxOutcome::Consumed
                } else {
                    self.highlight = (self.highlight + count - 1) % count;
                    InboxOutcome::Changed
                }
            }
            InboxKey::Jump => self.act(count, InboxAction::Jump),
            InboxKey::Approve => self.act(count, InboxAction::Approve),
            InboxKey::Deny => self.act(count, InboxAction::Deny),
            InboxKey::Snooze => self.act(count, InboxAction::Snooze),
            InboxKey::Dismiss => self.act(count, InboxAction::Dismiss),
            InboxKey::Other => InboxOutcome::Consumed,
        }
    }

    fn act(&self, count: usize, action: InboxAction) -> InboxOutcome {
        if count == 0 {
            InboxOutcome::Consumed
        } else {
            InboxOutcome::Act(action)
        }
    }
}

/// One item of visible triage state, as the shell needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct InboxSnapshot {
    pub items: Vec<taskmaster_proto::InboxItem>,
    pub highlight: usize,
}

impl InboxSnapshot {
    pub fn highlighted(&self) -> Option<&taskmaster_proto::InboxItem> {
        self.items.get(self.highlight)
    }

    pub fn highlighted_session(&self) -> Option<&SessionId> {
        self.highlighted().map(|item| &item.session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_ignored_until_the_inbox_opens() {
        let mut state = InboxSurfaceState::default();
        for key in [
            InboxKey::Down,
            InboxKey::Up,
            InboxKey::Jump,
            InboxKey::Approve,
            InboxKey::Deny,
            InboxKey::Snooze,
            InboxKey::Dismiss,
            InboxKey::Escape,
        ] {
            assert_eq!(state.handle(key, 3), InboxOutcome::Ignored);
        }
        assert!(state.open());
        assert!(!state.open(), "opening twice is not a new open");
        assert!(state.is_visible());
    }

    #[test]
    fn navigation_wraps_and_stays_consumed_on_an_empty_queue() {
        let mut state = InboxSurfaceState::default();
        state.open();
        assert_eq!(state.handle(InboxKey::Down, 0), InboxOutcome::Consumed);
        assert_eq!(state.handle(InboxKey::Up, 0), InboxOutcome::Consumed);

        assert_eq!(state.handle(InboxKey::Down, 3), InboxOutcome::Changed);
        assert_eq!(state.highlight(), 1);
        assert_eq!(state.handle(InboxKey::Down, 3), InboxOutcome::Changed);
        assert_eq!(state.highlight(), 2);
        assert_eq!(state.handle(InboxKey::Down, 3), InboxOutcome::Changed);
        assert_eq!(state.highlight(), 0, "wraps at the end");
        assert_eq!(state.handle(InboxKey::Up, 3), InboxOutcome::Changed);
        assert_eq!(state.highlight(), 2, "wraps at the start");
    }

    #[test]
    fn act_keys_require_an_item() {
        let mut state = InboxSurfaceState::default();
        state.open();
        assert_eq!(state.handle(InboxKey::Jump, 0), InboxOutcome::Consumed);
        assert_eq!(state.handle(InboxKey::Approve, 0), InboxOutcome::Consumed);
        assert_eq!(state.handle(InboxKey::Deny, 0), InboxOutcome::Consumed);
        assert_eq!(state.handle(InboxKey::Snooze, 0), InboxOutcome::Consumed);
        assert_eq!(state.handle(InboxKey::Dismiss, 0), InboxOutcome::Consumed);

        assert_eq!(
            state.handle(InboxKey::Approve, 2),
            InboxOutcome::Act(InboxAction::Approve)
        );
        assert_eq!(
            state.handle(InboxKey::Deny, 2),
            InboxOutcome::Act(InboxAction::Deny)
        );
        assert_eq!(
            state.handle(InboxKey::Jump, 2),
            InboxOutcome::Act(InboxAction::Jump)
        );
        assert_eq!(
            state.handle(InboxKey::Snooze, 2),
            InboxOutcome::Act(InboxAction::Snooze)
        );
        assert_eq!(
            state.handle(InboxKey::Dismiss, 2),
            InboxOutcome::Act(InboxAction::Dismiss)
        );
        assert!(state.is_visible(), "triage actions keep the inbox open");
        assert_eq!(state.handle(InboxKey::Other, 2), InboxOutcome::Consumed);
    }

    #[test]
    fn escape_closes_only_while_visible() {
        let mut state = InboxSurfaceState::default();
        assert_eq!(state.handle(InboxKey::Escape, 2), InboxOutcome::Ignored);
        state.open();
        assert_eq!(state.handle(InboxKey::Escape, 2), InboxOutcome::Changed);
        assert!(!state.is_visible());
    }

    #[test]
    fn highlight_clamps_when_items_leave_underneath() {
        let mut state = InboxSurfaceState::default();
        state.open();
        state.handle(InboxKey::Down, 4);
        state.handle(InboxKey::Down, 4);
        assert_eq!(state.highlight(), 2);

        state.reconcile(1);
        assert_eq!(state.highlight(), 0);

        state.reconcile(0);
        assert_eq!(state.highlight(), 0);
        assert_eq!(state.handle(InboxKey::Down, 0), InboxOutcome::Consumed);

        // Reopening starts at the top again.
        state.close();
        state.open();
        assert_eq!(state.highlight(), 0);
    }

    #[test]
    fn snapshot_resolves_the_highlighted_item() {
        assert_eq!(
            InboxSnapshot {
                items: vec![],
                highlight: 0
            }
            .highlighted_session(),
            None
        );
    }
}
