use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use embers_core::{BufferId, CellSize, PtySize};
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
/// The size includes pixels when the client knows its terminal's cell size, so
/// a pane's PTY reports the host terminal's cell size to programs that size
/// images by it.
///
/// Hidden and detached buffers are not in the presentation, so they keep the
/// last size they were given.
///
/// A failed request is retried on a later sync, at most once per
/// `RETRY_INTERVAL`, and only while the server still reports the size it had
/// when the request was planned. If another client resized the buffer in the
/// meantime, its size stands, as it would after a successful request.
#[derive(Debug, Default)]
pub(crate) struct PtySizeSync {
    /// The size this client last wanted for each buffer it has drawn.
    wanted: BTreeMap<BufferId, PtySize>,
    /// Set by user input, so the next sync reclaims any buffer whose size
    /// another client changed.
    reclaim: bool,
    /// The requests planned by the last sync.
    planned: BTreeMap<BufferId, Attempt>,
    /// Failed requests waiting to be retried.
    failed: BTreeMap<BufferId, Attempt>,
}

/// Keeps a buffer whose resize keeps failing from sending a request and
/// logging a warning on every frame.
const RETRY_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug)]
struct Attempt {
    /// The size the server reported when the request was planned.
    server_size: PtySize,
    planned_at: Instant,
}

impl PtySizeSync {
    pub(crate) fn note_user_activity(&mut self) {
        self.reclaim = true;
    }

    /// Records that the request planned for `buffer_id` by the last sync
    /// failed, so a later sync can retry it.
    pub(crate) fn resize_failed(&mut self, buffer_id: BufferId) {
        if let Some(attempt) = self.planned.remove(&buffer_id) {
            self.failed.insert(buffer_id, attempt);
        }
    }

    /// The resizes to request for the leaves of `presentation`, drawn on a
    /// terminal whose cells are `cell` pixels.
    pub(crate) fn plan(
        &mut self,
        state: &ClientState,
        presentation: &PresentationModel,
        cell: Option<CellSize>,
    ) -> Vec<(BufferId, PtySize)> {
        self.wanted
            .retain(|buffer_id, _| state.buffers.contains_key(buffer_id));
        self.failed
            .retain(|buffer_id, _| state.buffers.contains_key(buffer_id));
        let targets = presentation.leaves.iter().filter_map(|leaf| {
            let buffer = state.buffers.get(&leaf.buffer_id)?;
            // Only a running PTY can be resized; helper buffers have no PTY.
            if buffer.kind != BufferRecordKind::Pty || buffer.state != BufferRecordState::Running {
                return None;
            }
            Some((
                leaf.buffer_id,
                leaf.pty_size()?.with_cell_size(cell),
                buffer.pty_size,
            ))
        });
        self.plan_targets(targets, Instant::now())
    }

    /// `targets` holds `(buffer, wanted size, size the server reports)` for
    /// every resizable visible buffer.
    fn plan_targets(
        &mut self,
        targets: impl IntoIterator<Item = (BufferId, PtySize, PtySize)>,
        now: Instant,
    ) -> Vec<(BufferId, PtySize)> {
        let reclaim = std::mem::take(&mut self.reclaim);
        self.planned.clear();
        let mut resizes = Vec::new();
        for (buffer_id, wanted, current) in targets {
            let mut retry = false;
            if let Some(failure) = self.failed.get(&buffer_id) {
                if failure.server_size != current {
                    // Another client resized the buffer since; its size stands.
                    self.failed.remove(&buffer_id);
                } else if now.duration_since(failure.planned_at) < RETRY_INTERVAL {
                    // Leave `wanted` as it was, so a change made meanwhile is
                    // still noticed once the retry is due.
                    continue;
                } else {
                    self.failed.remove(&buffer_id);
                    retry = true;
                }
            }
            let changed = match self.wanted.insert(buffer_id, wanted) {
                Some(previous) if !reclaim && !retry => previous != wanted,
                Some(_) => !matches_server(current, wanted),
                None => current != wanted,
            };
            if changed {
                self.planned.insert(
                    buffer_id,
                    Attempt {
                        server_size: current,
                        planned_at: now,
                    },
                );
                resizes.push((buffer_id, wanted));
            }
        }
        resizes
    }
}

/// Whether the server's size for a buffer already is `wanted`. A size without
/// pixels only has to match in cells: a server that predates pixel sizes
/// always reports none, and must not be resized on every keystroke. A buffer
/// first drawn without them still gets this client's, as the first sync
/// compares whole sizes.
fn matches_server(current: PtySize, wanted: PtySize) -> bool {
    if current.cell_size().is_none() {
        return current.cols == wanted.cols && current.rows == wanted.rows;
    }
    current == wanted
}

#[cfg(test)]
mod tests {
    use embers_core::{BufferId, PtySize};

    use std::time::{Duration, Instant};

    use super::{PtySizeSync, RETRY_INTERVAL};

    const BUFFER: BufferId = BufferId(1);

    fn plan(
        sync: &mut PtySizeSync,
        targets: [(BufferId, PtySize, PtySize); 1],
    ) -> Vec<(BufferId, PtySize)> {
        sync.plan_targets(targets, Instant::now())
    }

    #[test]
    fn first_sync_resizes_only_when_the_server_size_differs() {
        let mut sync = PtySizeSync::default();
        assert_eq!(
            plan(
                &mut sync,
                [(BUFFER, PtySize::new(80, 24), PtySize::new(80, 24))]
            ),
            vec![]
        );

        let mut sync = PtySizeSync::default();
        assert_eq!(
            plan(
                &mut sync,
                [(BUFFER, PtySize::new(100, 30), PtySize::new(80, 24))]
            ),
            vec![(BUFFER, PtySize::new(100, 30))]
        );
    }

    fn with_cells(size: PtySize, width: u16, height: u16) -> PtySize {
        size.with_cell_size(Some(embers_core::CellSize { width, height }))
    }

    #[test]
    fn a_pixel_size_is_sent_with_the_cells() {
        let mut sync = PtySizeSync::default();
        let cells = PtySize::new(80, 24);
        let wanted = with_cells(cells, 9, 18);
        assert_eq!(
            wanted,
            PtySize {
                cols: 80,
                rows: 24,
                pixel_width: 720,
                pixel_height: 432,
            }
        );
        // The server has the cells but no pixel size, as for a pane a
        // command line created.
        assert_eq!(
            plan(&mut sync, [(BUFFER, wanted, cells)]),
            vec![(BUFFER, wanted)]
        );
        assert_eq!(plan(&mut sync, [(BUFFER, wanted, wanted)]), vec![]);

        // The host terminal's font changes: same cells, new pixels.
        let bigger = with_cells(cells, 10, 20);
        assert_eq!(
            plan(&mut sync, [(BUFFER, bigger, wanted)]),
            vec![(BUFFER, bigger)]
        );
    }

    #[test]
    fn a_server_without_pixel_sizes_is_not_resized_on_every_keystroke() {
        let mut sync = PtySizeSync::default();
        let cells = PtySize::new(80, 24);
        let wanted = with_cells(cells, 9, 18);
        // A server that predates pixel sizes reports none, even after this
        // client sent them.
        assert_eq!(plan(&mut sync, [(BUFFER, wanted, cells)]).len(), 1);
        sync.note_user_activity();
        assert_eq!(plan(&mut sync, [(BUFFER, wanted, cells)]), vec![]);
        // Activity still reclaims a size another client set with pixels.
        sync.note_user_activity();
        let other = with_cells(cells, 10, 20);
        assert_eq!(
            plan(&mut sync, [(BUFFER, wanted, other)]),
            vec![(BUFFER, wanted)]
        );
    }

    #[test]
    fn unchanged_wanted_size_is_not_resent() {
        let mut sync = PtySizeSync::default();
        let wanted = PtySize::new(100, 30);
        assert_eq!(
            plan(&mut sync, [(BUFFER, wanted, PtySize::new(80, 24))]).len(),
            1
        );
        // The server has not caught up yet; still no second request.
        assert_eq!(
            plan(&mut sync, [(BUFFER, wanted, PtySize::new(80, 24))]),
            vec![]
        );
        assert_eq!(plan(&mut sync, [(BUFFER, wanted, wanted)]), vec![]);
    }

    #[test]
    fn idle_client_does_not_override_another_clients_size() {
        let mut sync = PtySizeSync::default();
        let wanted = PtySize::new(100, 30);
        plan(&mut sync, [(BUFFER, wanted, wanted)]);
        // Another client resized the buffer to 80x20.
        assert_eq!(
            plan(&mut sync, [(BUFFER, wanted, PtySize::new(80, 20))]),
            vec![]
        );
        // This client's own layout changes, so it takes the buffer back.
        let resized = PtySize::new(120, 40);
        assert_eq!(
            plan(&mut sync, [(BUFFER, resized, PtySize::new(80, 20))]),
            vec![(BUFFER, resized)]
        );
    }

    #[test]
    fn user_activity_reclaims_the_size_once() {
        let mut sync = PtySizeSync::default();
        let wanted = PtySize::new(100, 30);
        let other = PtySize::new(80, 20);
        plan(&mut sync, [(BUFFER, wanted, wanted)]);
        sync.note_user_activity();
        assert_eq!(
            plan(&mut sync, [(BUFFER, wanted, other)]),
            vec![(BUFFER, wanted)]
        );
        assert_eq!(plan(&mut sync, [(BUFFER, wanted, other)]), vec![]);
        // Activity alone does not resend when the server already agrees.
        sync.note_user_activity();
        assert_eq!(plan(&mut sync, [(BUFFER, wanted, wanted)]), vec![]);
    }

    #[test]
    fn failed_resize_is_retried_once_due() {
        let mut sync = PtySizeSync::default();
        let wanted = PtySize::new(100, 30);
        let server = PtySize::new(80, 24);
        let start = Instant::now();
        assert_eq!(
            sync.plan_targets([(BUFFER, wanted, server)], start),
            vec![(BUFFER, wanted)]
        );
        sync.resize_failed(BUFFER);
        let due = start + RETRY_INTERVAL;
        assert_eq!(
            sync.plan_targets([(BUFFER, wanted, server)], due),
            vec![(BUFFER, wanted)]
        );
        // The retry succeeded, so nothing more is sent.
        assert_eq!(
            sync.plan_targets([(BUFFER, wanted, server)], due + RETRY_INTERVAL),
            vec![]
        );
    }

    #[test]
    fn failing_resize_is_retried_at_most_once_per_interval() {
        let mut sync = PtySizeSync::default();
        let wanted = PtySize::new(100, 30);
        let server = PtySize::new(80, 24);
        let start = Instant::now();
        sync.plan_targets([(BUFFER, wanted, server)], start);
        sync.resize_failed(BUFFER);
        // Frames before the interval has passed send nothing, even after a
        // layout change and user input.
        let early = start + RETRY_INTERVAL - Duration::from_millis(1);
        assert_eq!(sync.plan_targets([(BUFFER, wanted, server)], early), vec![]);
        let resized = PtySize::new(120, 40);
        sync.note_user_activity();
        assert_eq!(
            sync.plan_targets([(BUFFER, resized, server)], early),
            vec![]
        );
        let due = start + RETRY_INTERVAL;
        assert_eq!(
            sync.plan_targets([(BUFFER, resized, server)], due),
            vec![(BUFFER, resized)]
        );
        sync.resize_failed(BUFFER);
        assert_eq!(
            sync.plan_targets([(BUFFER, resized, server)], due + Duration::from_millis(1)),
            vec![]
        );
    }

    #[test]
    fn failed_resize_does_not_override_another_clients_size() {
        let mut sync = PtySizeSync::default();
        let wanted = PtySize::new(100, 30);
        let start = Instant::now();
        sync.plan_targets([(BUFFER, wanted, PtySize::new(80, 24))], start);
        sync.resize_failed(BUFFER);
        // Another client resized the buffer before the retry was due.
        let other = PtySize::new(80, 20);
        let due = start + RETRY_INTERVAL;
        assert_eq!(sync.plan_targets([(BUFFER, wanted, other)], due), vec![]);
        // This client's own layout change still takes the buffer back.
        let resized = PtySize::new(120, 40);
        assert_eq!(
            sync.plan_targets([(BUFFER, resized, other)], due),
            vec![(BUFFER, resized)]
        );
    }

    #[test]
    fn layout_change_during_backoff_survives_another_clients_resize() {
        let mut sync = PtySizeSync::default();
        let wanted = PtySize::new(100, 30);
        let server = PtySize::new(80, 24);
        let start = Instant::now();
        sync.plan_targets([(BUFFER, wanted, server)], start);
        sync.resize_failed(BUFFER);
        let resized = PtySize::new(120, 40);
        assert_eq!(
            sync.plan_targets([(BUFFER, resized, server)], start),
            vec![]
        );
        // Another client resized the buffer, but this client's layout changed
        // too, which a successful request would also have sent.
        let other = PtySize::new(80, 20);
        assert_eq!(
            sync.plan_targets([(BUFFER, resized, other)], start + RETRY_INTERVAL),
            vec![(BUFFER, resized)]
        );
    }
}
