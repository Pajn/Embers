use std::collections::BTreeMap;

use embers_core::{BufferId, PtySize};
use embers_protocol::{BufferRecordKind, BufferRecordState};

use crate::presentation::PresentationModel;
use crate::state::ClientState;

/// Decides which buffers need a PTY resize so each visible pane's PTY matches
/// the content area this client draws for it.
///
/// Several clients can show the same buffer at different sizes, and the PTY can
/// only have one. The most recently active client wins: a client resizes a
/// buffer when its own wanted size for it changes (attach, layout change, outer
/// terminal resize) or after local user input, but never merely because another
/// client resized it. That keeps two attached clients from fighting over a
/// shared PTY on every redraw, and needs no extra server state.
///
/// Hidden and detached buffers are not in the presentation, so they keep the
/// last size they were given.
#[derive(Debug, Default)]
pub(crate) struct PtySizeSync {
    /// The size this client last wanted for each buffer it has drawn.
    wanted: BTreeMap<BufferId, PtySize>,
    /// Set by user input, so the next sync reclaims any buffer whose size
    /// another client changed.
    reclaim: bool,
}

impl PtySizeSync {
    pub(crate) fn note_user_activity(&mut self) {
        self.reclaim = true;
    }

    /// The resizes to request for the leaves of `presentation`.
    pub(crate) fn plan(
        &mut self,
        state: &ClientState,
        presentation: &PresentationModel,
    ) -> Vec<(BufferId, PtySize)> {
        self.wanted
            .retain(|buffer_id, _| state.buffers.contains_key(buffer_id));
        let targets = presentation.leaves.iter().filter_map(|leaf| {
            let buffer = state.buffers.get(&leaf.buffer_id)?;
            // Only a running PTY can be resized; helper buffers have no PTY.
            if buffer.kind != BufferRecordKind::Pty || buffer.state != BufferRecordState::Running {
                return None;
            }
            Some((leaf.buffer_id, leaf.pty_size()?, buffer.pty_size))
        });
        self.plan_targets(targets)
    }

    /// `targets` holds `(buffer, wanted size, size the server reports)` for
    /// every resizable visible buffer.
    fn plan_targets(
        &mut self,
        targets: impl IntoIterator<Item = (BufferId, PtySize, PtySize)>,
    ) -> Vec<(BufferId, PtySize)> {
        let reclaim = std::mem::take(&mut self.reclaim);
        let mut resizes = Vec::new();
        for (buffer_id, wanted, current) in targets {
            let changed = match self.wanted.insert(buffer_id, wanted) {
                Some(previous) if !reclaim => !same_cells(previous, wanted),
                _ => !same_cells(current, wanted),
            };
            if changed {
                resizes.push((buffer_id, wanted));
            }
        }
        resizes
    }
}

fn same_cells(left: PtySize, right: PtySize) -> bool {
    left.cols == right.cols && left.rows == right.rows
}

#[cfg(test)]
mod tests {
    use embers_core::{BufferId, PtySize};

    use super::PtySizeSync;

    const BUFFER: BufferId = BufferId(1);

    #[test]
    fn first_sync_resizes_only_when_the_server_size_differs() {
        let mut sync = PtySizeSync::default();
        assert_eq!(
            sync.plan_targets([(BUFFER, PtySize::new(80, 24), PtySize::new(80, 24))]),
            vec![]
        );

        let mut sync = PtySizeSync::default();
        assert_eq!(
            sync.plan_targets([(BUFFER, PtySize::new(100, 30), PtySize::new(80, 24))]),
            vec![(BUFFER, PtySize::new(100, 30))]
        );
    }

    #[test]
    fn unchanged_wanted_size_is_not_resent() {
        let mut sync = PtySizeSync::default();
        let wanted = PtySize::new(100, 30);
        assert_eq!(
            sync.plan_targets([(BUFFER, wanted, PtySize::new(80, 24))])
                .len(),
            1
        );
        // The server has not caught up yet; still no second request.
        assert_eq!(
            sync.plan_targets([(BUFFER, wanted, PtySize::new(80, 24))]),
            vec![]
        );
        assert_eq!(sync.plan_targets([(BUFFER, wanted, wanted)]), vec![]);
    }

    #[test]
    fn idle_client_does_not_override_another_clients_size() {
        let mut sync = PtySizeSync::default();
        let wanted = PtySize::new(100, 30);
        sync.plan_targets([(BUFFER, wanted, wanted)]);
        // Another client resized the buffer to 80x20.
        assert_eq!(
            sync.plan_targets([(BUFFER, wanted, PtySize::new(80, 20))]),
            vec![]
        );
        // This client's own layout changes, so it takes the buffer back.
        let resized = PtySize::new(120, 40);
        assert_eq!(
            sync.plan_targets([(BUFFER, resized, PtySize::new(80, 20))]),
            vec![(BUFFER, resized)]
        );
    }

    #[test]
    fn user_activity_reclaims_the_size_once() {
        let mut sync = PtySizeSync::default();
        let wanted = PtySize::new(100, 30);
        let other = PtySize::new(80, 20);
        sync.plan_targets([(BUFFER, wanted, wanted)]);
        sync.note_user_activity();
        assert_eq!(
            sync.plan_targets([(BUFFER, wanted, other)]),
            vec![(BUFFER, wanted)]
        );
        assert_eq!(sync.plan_targets([(BUFFER, wanted, other)]), vec![]);
        // Activity alone does not resend when the server already agrees.
        sync.note_user_activity();
        assert_eq!(sync.plan_targets([(BUFFER, wanted, wanted)]), vec![]);
    }
}
