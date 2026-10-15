//! Shrinking a pane's emulator state once the pane has gone quiet.
//!
//! Some backends can compact what they hold, Ghostty's by compressing
//! scrollback pages, in bounded steps (see [`TerminalBackend::compact_step`]).
//! A step must not run on the output path, where it would slow a busy pane,
//! nor right after output, when more is likely to follow and the work would
//! be undone. The keeper notes everything that changes or reads back the
//! scrollback with [`IdleCompaction::note_activity`]; a thread of its own
//! runs [`IdleCompaction::run`], which waits until [`COMPACTION_IDLE`] has
//! passed without any, then steps until the backend is done or activity
//! resumes.
//!
//! [`TerminalBackend::compact_step`]: crate::TerminalBackend::compact_step

use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// How long a pane must be quiet before its keeper compacts it.
pub(crate) const COMPACTION_IDLE: Duration = Duration::from_secs(5);

#[derive(Debug, Default)]
pub(crate) struct IdleCompaction {
    /// Counts activity; compaction is due when it has moved past the count
    /// the last compaction finished at.
    activity: Mutex<u64>,
    changed: Condvar,
}

impl IdleCompaction {
    pub(crate) fn note_activity(&self) {
        let mut activity = self
            .activity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *activity += 1;
        self.changed.notify_one();
    }

    /// Compact with `step` each time activity has been followed by `idle`
    /// without more. `step` returns whether more steps are left. Never
    /// returns: the keeper ends with its process.
    pub(crate) fn run(&self, idle: Duration, mut step: impl FnMut() -> bool) -> ! {
        let mut compacted_at = 0;
        loop {
            let mut activity = self
                .activity
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            while *activity == compacted_at {
                activity = self
                    .changed
                    .wait(activity)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            // Wait out the activity: until a whole `idle` passes without more.
            loop {
                let seen = *activity;
                activity = self
                    .changed
                    .wait_timeout_while(activity, idle, |activity| *activity == seen)
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .0;
                if *activity == seen {
                    break;
                }
            }
            compacted_at = *activity;
            drop(activity);

            // Each step takes the surface lock on its own, so output and
            // requests get in between steps; activity in between stops the
            // run, which starts over once the pane is quiet again.
            while step() {
                let activity = self
                    .activity
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if *activity != compacted_at {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Instant;

    use super::*;

    const IDLE: Duration = Duration::from_millis(50);

    /// Runs compaction on a thread of its own with a backend that has
    /// `pending` steps of work each time it is idle. Returns the scheduler
    /// and the count of steps run.
    fn start(pending: usize) -> (Arc<IdleCompaction>, Arc<AtomicUsize>) {
        let compaction = Arc::new(IdleCompaction::default());
        let steps = Arc::new(AtomicUsize::new(0));
        let (scheduler, counter) = (compaction.clone(), steps.clone());
        thread::spawn(move || {
            let mut left = 0;
            scheduler.run(IDLE, || {
                if left == 0 {
                    left = pending;
                }
                counter.fetch_add(1, Ordering::SeqCst);
                left -= 1;
                left > 0
            })
        });
        (compaction, steps)
    }

    fn wait_for(steps: &AtomicUsize, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while steps.load(Ordering::SeqCst) < count {
            assert!(
                Instant::now() < deadline,
                "compaction never ran {count} steps"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn nothing_runs_before_any_activity() {
        let (_compaction, steps) = start(3);
        thread::sleep(IDLE * 3);
        assert_eq!(steps.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn runs_every_step_once_quiet_then_waits_for_activity() {
        let (compaction, steps) = start(3);
        compaction.note_activity();
        wait_for(&steps, 3);
        thread::sleep(IDLE * 3);
        assert_eq!(
            steps.load(Ordering::SeqCst),
            3,
            "ran again without activity"
        );

        compaction.note_activity();
        wait_for(&steps, 6);
    }

    #[test]
    fn waits_while_activity_continues() {
        let (compaction, steps) = start(1);
        let busy_until = Instant::now() + IDLE * 4;
        while Instant::now() < busy_until {
            compaction.note_activity();
            thread::sleep(IDLE / 5);
            assert_eq!(steps.load(Ordering::SeqCst), 0, "ran while busy");
        }
        wait_for(&steps, 1);
    }
}
