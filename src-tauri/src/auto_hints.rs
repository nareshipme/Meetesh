//! Offering hints without being asked (opt-in): watches the live transcript for a question from
//! the other participants, or a pause after a stretch of talk, and starts a hint. The rules live in
//! the core (`assist::trigger`); this is the clock and the wiring.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use meetesh_core::assist::trigger::{AutoHintMode, TriggerConfig, TriggerDetector};
use meetesh_core::TranscriptSegment;
use tauri::AppHandle;

use crate::commands::assist::start_hint;

const TICK: Duration = Duration::from_millis(500);

pub struct AutoHints {
    mode: Mutex<AutoHintMode>,
    clock: Instant,
    /// The detector and the stop flag of the ticker for the recording in progress, if any.
    current: Mutex<Option<(Arc<Mutex<TriggerDetector>>, Arc<AtomicBool>)>>,
}

impl AutoHints {
    pub fn new(mode: AutoHintMode) -> Self {
        AutoHints { mode: Mutex::new(mode), clock: Instant::now(), current: Mutex::new(None) }
    }

    pub fn mode(&self) -> AutoHintMode {
        *self.mode.lock().unwrap()
    }

    /// Change the mode, also for the recording in progress.
    pub fn set_mode(&self, mode: AutoHintMode) {
        *self.mode.lock().unwrap() = mode;
        if let Some((det, _)) = self.current.lock().unwrap().as_ref() {
            det.lock().unwrap().set_mode(mode);
        }
    }

    fn now_ms(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }

    /// A recording started. `has_others` is true when the other participants are on their own
    /// track (the only way to tell their questions from the user's own).
    pub fn begin(self: &Arc<Self>, app: &AppHandle, meeting_id: i64, has_others: bool) {
        self.end();
        let det = Arc::new(Mutex::new(TriggerDetector::new(TriggerConfig::default(), self.mode(), has_others)));
        let stop = Arc::new(AtomicBool::new(false));
        *self.current.lock().unwrap() = Some((det.clone(), stop.clone()));

        let (this, app) = (self.clone(), app.clone());
        std::thread::Builder::new()
            .name("meetesh-auto-hints".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(TICK);
                    let trigger = det.lock().unwrap().poll(this.now_ms());
                    if let Some(t) = trigger {
                        match start_hint(&app, meeting_id, Some(t)) {
                            Ok(_) => log::info!("automatic hint for meeting {meeting_id}: {}", t.as_str()),
                            // Busy or the model is not ready: try again at the next opportunity.
                            Err(_) => det.lock().unwrap().not_fired(),
                        }
                    }
                }
            })
            .expect("spawn auto hint thread");
    }

    /// New live transcript segments for the recording in progress.
    pub fn on_segments(&self, segments: &[TranscriptSegment]) {
        if let Some((det, _)) = self.current.lock().unwrap().as_ref() {
            det.lock().unwrap().on_segments(segments, self.now_ms());
        }
    }

    pub fn end(&self) {
        if let Some((_, stop)) = self.current.lock().unwrap().take() {
            stop.store(true, Ordering::Relaxed);
        }
    }
}
