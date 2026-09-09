//! Rule 6: one window in flight, results streamed out.
//!
//! A window is read, processed, its patch written to the session cache,
//! buffers dropped. Patches are **raw planar buffers with a small header**,
//! not PNG.
//!
//! Concurrency: 1 window under 16 GB, 2 above.
//!
//! The patch container is [`crate::project::buffers`] and has been since the
//! manifest landed. What this module is, is the **discipline**: a loop whose
//! shape makes "two windows resident at once" unreachable rather than
//! unintended.
//!
//! ## Why a driver rather than a comment on a `for` loop
//!
//! The rule is a bound on what is alive at a given moment, and a bound nothing
//! asserts is a bound that holds until the next edit. [`stream_windows`] takes
//! the read, the work and the sink as three separate closures precisely so that
//! the buffer's lifetime is this module's and not the caller's: the window
//! buffer is dropped **before** the result reaches the sink, so the peak is one
//! window plus one patch and never one window plus one patch plus the next
//! window.
//!
//! [`stream_windows::tests::the_window_buffer_is_gone_before_its_patch_is_written`]
//! is that sentence as an assertion, over a buffer type that counts its own
//! live instances.
//!
//! ## The results come out in order
//!
//! Region order is fixed and the running composite depends on it:
//! "whichever runs first would otherwise see the other's *text* inside its 512²
//! context". So the prefetch that a 2-window budget buys is a prefetch of the
//! **read** only. The work and the sink stay in plan order at every budget, and
//! two machines with different memory produce the same patches.

/// Rule 6's threshold. A machine with less than this runs one window at a time.
pub const CONCURRENCY_THRESHOLD_BYTES: u64 = 16 * 1024 * 1024 * 1024;

/// Rule 6's "1 window under 16 GB, 2 above".
///
/// A machine of exactly 16 GB is not *under* 16 GB, so it gets two - and it is
/// worth being explicit rather than letting a `>` decide it silently, because
/// 16 GB is by far the most common configuration this will meet.
///
/// An unknown total is one window. Under pressure rule 6's own remedy is to
/// "drop to one window in flight", so one is the answer that is never wrong for
/// a reason other than speed.
pub fn windows_in_flight(total_ram_bytes: Option<u64>) -> usize {
    match total_ram_bytes {
        Some(bytes) if bytes >= CONCURRENCY_THRESHOLD_BYTES => 2,
        _ => 1,
    }
}

/// Rule 6's loop.
///
/// For each window in `plan`, in order: `read` it, `work` on it, **drop the
/// buffer**, then hand the result to `sink`. At most `in_flight` window buffers
/// are alive at any moment; `in_flight` above one prefetches the next read and
/// changes nothing else.
///
/// `read` answering `None` skips a window without stopping the run - a window
/// whose page could not be decoded is one region lost, not a chapter lost, and
/// the caller counts it. An error from `work` or `sink` stops the walk, because
/// those two are the caller's own failure and it is the caller that decides
/// what a half-written region means.
pub fn stream_windows<W, B, R, E>(
    plan: &[W],
    in_flight: usize,
    mut read: impl FnMut(&W) -> Option<B>,
    mut work: impl FnMut(&W, &B) -> Result<R, E>,
    mut sink: impl FnMut(&W, R) -> Result<(), E>,
) -> Result<(), E> {
    let in_flight = in_flight.max(1);
    // `Option<B>` rather than skipping the entry, so a window that could not be
    // read still occupies its position and the prefetch does not silently run
    // ahead of the plan.
    let mut queue: std::collections::VecDeque<(usize, Option<B>)> = std::collections::VecDeque::new();
    let mut next = 0usize;

    loop {
        while queue.len() < in_flight && next < plan.len() {
            queue.push_back((next, read(&plan[next])));
            next += 1;
        }
        let Some((index, buffer)) = queue.pop_front() else { return Ok(()) };
        let Some(buffer) = buffer else { continue };

        let result = work(&plan[index], &buffer)?;
        // **The rule, as one line.** The window's samples are gone before its
        // patch is written, so the peak is one window plus one patch.
        drop(buffer);
        sink(&plan[index], result)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// A buffer that counts itself, so "how many are alive" is a fact rather
    /// than an argument.
    struct Counted {
        live: Rc<Cell<usize>>,
        value: u32,
    }

    impl Counted {
        fn new(live: &Rc<Cell<usize>>, peak: &Rc<Cell<usize>>, value: u32) -> Counted {
            live.set(live.get() + 1);
            peak.set(peak.get().max(live.get()));
            Counted { live: Rc::clone(live), value }
        }
    }

    impl Drop for Counted {
        fn drop(&mut self) {
            self.live.set(self.live.get() - 1);
        }
    }

    fn counters() -> (Rc<Cell<usize>>, Rc<Cell<usize>>) {
        (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)))
    }

    /// **Rule 6's sentence, asserted.** By the time a window's patch reaches the
    /// cache, the window's samples are gone.
    #[test]
    fn the_window_buffer_is_gone_before_its_patch_is_written() {
        let (live, peak) = counters();
        let plan: Vec<u32> = (0..8).collect();
        let mut written = Vec::new();

        stream_windows::<_, _, _, ()>(
            &plan,
            1,
            |w| Some(Counted::new(&live, &peak, *w)),
            |_, b| Ok(b.value * 10),
            |w, patch| {
                assert_eq!(live.get(), 0, "window {w}'s buffer was still resident");
                written.push(patch);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(peak.get(), 1, "two windows were resident at once");
        assert_eq!(live.get(), 0);
        assert_eq!(written, vec![0, 10, 20, 30, 40, 50, 60, 70]);
    }

    /// The 16 GB budget, and the order it does not change.
    #[test]
    fn a_second_window_is_a_prefetch_of_the_read_and_nothing_else() {
        let (live, peak) = counters();
        let plan: Vec<u32> = (0..8).collect();
        let mut order = Vec::new();

        stream_windows::<_, _, _, ()>(
            &plan,
            2,
            |w| Some(Counted::new(&live, &peak, *w)),
            |_, b| Ok(b.value),
            |_, patch| {
                assert!(live.get() <= 2, "the budget was exceeded");
                order.push(patch);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(peak.get(), 2, "the prefetch never happened");
        assert_eq!(order, (0..8).collect::<Vec<_>>(), "the plan order changed with the budget");
    }

    #[test]
    fn rule_sixs_concurrency_is_one_below_sixteen_gigabytes_and_two_at_or_above() {
        let gb = 1024 * 1024 * 1024u64;
        assert_eq!(windows_in_flight(Some(8 * gb)), 1);
        assert_eq!(windows_in_flight(Some(16 * gb - 1)), 1);
        assert_eq!(windows_in_flight(Some(16 * gb)), 2, "16 GB is not under 16 GB");
        assert_eq!(windows_in_flight(Some(32 * gb)), 2);
        assert_eq!(windows_in_flight(None), 1, "an unknown total takes the safe side");
        assert_eq!(CONCURRENCY_THRESHOLD_BYTES, 16 * gb);
    }

    /// A window whose page could not be decoded costs one region, not the run.
    #[test]
    fn an_unreadable_window_is_skipped_and_the_rest_still_run() {
        let (live, peak) = counters();
        let plan: Vec<u32> = (0..5).collect();
        let mut written = Vec::new();

        stream_windows::<_, _, _, ()>(
            &plan,
            2,
            |w| (*w != 2).then(|| Counted::new(&live, &peak, *w)),
            |_, b| Ok(b.value),
            |_, patch| {
                written.push(patch);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(written, vec![0, 1, 3, 4]);
        assert_eq!(live.get(), 0);
    }

    /// An error stops the walk, and nothing is left resident on the way out.
    #[test]
    fn a_failing_region_stops_the_walk_and_frees_its_buffer() {
        let (live, peak) = counters();
        let plan: Vec<u32> = (0..5).collect();
        let mut written = 0usize;

        let outcome = stream_windows(
            &plan,
            1,
            |w| Some(Counted::new(&live, &peak, *w)),
            |_, b| if b.value == 2 { Err("no disk") } else { Ok(b.value) },
            |_, _| {
                written += 1;
                Ok(())
            },
        );
        assert_eq!(outcome, Err("no disk"));
        assert_eq!(written, 2);
        assert_eq!(peak.get(), 1);
        assert_eq!(live.get(), 0, "the failing window's buffer outlived the walk");
    }
}
