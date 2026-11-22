//! Who gets the processor: jobs that wait for the user come first.
//!
//! The on-device model handles one request at a time, and the processor is shared with live
//! transcription. So work is split in two kinds:
//! - **interactive**: a hint the user asked for (or the app offered), which someone is waiting for;
//! - **background**: meeting notes and titles, which nobody is waiting for.
//!
//! A background job asks the gate before each step and is stopped (not cancelled) while an
//! interactive job runs, and, on a processor-bound machine, while a recording is in progress so live
//! transcription never competes with it. When the way is clear it carries on by itself.
//! Pure logic with no model, so every rule is unit-tested.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Why background work is waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseReason {
    /// A hint is being written.
    Hint,
    /// A recording is in progress and this job would compete with live transcription.
    Recording,
}

#[derive(Default)]
pub struct ModelGate {
    interactive: AtomicUsize,
    recording: AtomicBool,
}

/// Held while an interactive job runs; dropping it lets background work continue.
pub struct InteractiveGuard {
    gate: Arc<ModelGate>,
}

impl Drop for InteractiveGuard {
    fn drop(&mut self) {
        self.gate.interactive.fetch_sub(1, Ordering::SeqCst);
    }
}

impl ModelGate {
    pub fn new() -> Arc<Self> {
        Arc::new(ModelGate::default())
    }

    /// An interactive job starts. Background work yields until the guard is dropped.
    pub fn begin_interactive(self: &Arc<Self>) -> InteractiveGuard {
        self.interactive.fetch_add(1, Ordering::SeqCst);
        InteractiveGuard { gate: self.clone() }
    }

    /// A recording started or stopped.
    pub fn set_recording(&self, on: bool) {
        self.recording.store(on, Ordering::SeqCst);
    }

    /// Should a background job wait right now? `cpu_bound`: the job runs on the processor (no GPU),
    /// so it would slow down live transcription; a GPU job may run during a recording.
    pub fn pause_reason(&self, cpu_bound: bool) -> Option<PauseReason> {
        if self.interactive.load(Ordering::SeqCst) > 0 {
            Some(PauseReason::Hint)
        } else if cpu_bound && self.recording.load(Ordering::SeqCst) {
            Some(PauseReason::Recording)
        } else {
            None
        }
    }

    /// The check to hand to a running request (`GenOptions::preempt`): true means "stop, someone more
    /// important needs the processor".
    pub fn preempt_check(self: &Arc<Self>, cpu_bound: bool) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let gate = self.clone();
        Arc::new(move || gate.pause_reason(cpu_bound).is_some())
    }

    /// Block until a background job may run. `on_change` is told when the job starts waiting
    /// (`Some(reason)`) and when it may go on (`None`). Returns false if `cancel` was set meanwhile.
    pub fn wait_for_turn(&self, cpu_bound: bool, cancel: &AtomicBool, mut on_change: impl FnMut(Option<PauseReason>)) -> bool {
        let mut announced: Option<PauseReason> = None;
        loop {
            if cancel.load(Ordering::Relaxed) {
                if announced.is_some() {
                    on_change(None);
                }
                return false;
            }
            let reason = self.pause_reason(cpu_bound);
            if reason != announced {
                on_change(reason);
                announced = reason;
            }
            if reason.is_none() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn an_interactive_job_pauses_background_work_until_it_ends() {
        let g = ModelGate::new();
        assert_eq!(g.pause_reason(false), None);
        let a = g.begin_interactive();
        let b = g.begin_interactive();
        assert_eq!(g.pause_reason(false), Some(PauseReason::Hint));
        assert_eq!(g.pause_reason(true), Some(PauseReason::Hint), "also for a processor-bound job");
        drop(a);
        assert_eq!(g.pause_reason(false), Some(PauseReason::Hint), "two hints: still one running");
        drop(b);
        assert_eq!(g.pause_reason(false), None);
    }

    #[test]
    fn a_recording_pauses_only_processor_bound_background_work() {
        let g = ModelGate::new();
        g.set_recording(true);
        assert_eq!(g.pause_reason(true), Some(PauseReason::Recording), "no GPU: it would slow live transcription");
        assert_eq!(g.pause_reason(false), None, "a GPU job does not compete for the processor");
        g.set_recording(false);
        assert_eq!(g.pause_reason(true), None);
        // A hint outranks the "GPU may run during a recording" allowance.
        g.set_recording(true);
        let _h = g.begin_interactive();
        assert_eq!(g.pause_reason(false), Some(PauseReason::Hint));
    }

    #[test]
    fn the_preempt_check_follows_the_gate() {
        let g = ModelGate::new();
        let check = g.preempt_check(true);
        assert!(!check());
        let h = g.begin_interactive();
        assert!(check());
        drop(h);
        assert!(!check());
        g.set_recording(true);
        assert!(check());
        assert!(!g.preempt_check(false)());
    }

    #[test]
    fn waiting_announces_the_pause_and_the_resume_and_then_goes_on() {
        let g = ModelGate::new();
        let cancel = AtomicBool::new(false);
        let guard = g.begin_interactive();
        let events = Arc::new(Mutex::new(Vec::new()));
        let (g2, e2) = (g.clone(), events.clone());
        let t = std::thread::spawn(move || g2.wait_for_turn(false, &AtomicBool::new(false), |r| e2.lock().unwrap().push(r)));
        std::thread::sleep(Duration::from_millis(120));
        assert!(!t.is_finished(), "still waiting while the hint runs");
        drop(guard);
        assert!(t.join().unwrap(), "continues once the hint is done");
        assert_eq!(*events.lock().unwrap(), vec![Some(PauseReason::Hint), None]);
        assert!(g.wait_for_turn(false, &cancel, |_| panic!("nothing to announce when the way is clear")));
    }

    #[test]
    fn cancelling_ends_the_wait() {
        let g = ModelGate::new();
        let _h = g.begin_interactive();
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            c2.store(true, Ordering::Relaxed);
        });
        let start = std::time::Instant::now();
        assert!(!g.wait_for_turn(false, &cancel, |_| {}));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
