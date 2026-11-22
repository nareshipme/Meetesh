//! Live points while recording: every minute or so the lines said since the last update go to the
//! model, which updates the short list of what is being discussed. The rules (what to ask, when to
//! ask) are in the core (`assist::points`); this is the clock and the wiring.
//!
//! It never competes with a hint (the update steps aside and is redone), and on a slow machine it
//! rests longer between updates, so the processor stays free for live transcription.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use meetesh_core::assist::points::{update_points, Points, Scheduler};
use meetesh_core::assist::summary::SummaryRun;
use meetesh_core::{Error, TranscriptSegment};
use tauri::{AppHandle, Emitter, Manager};

use crate::events::{LivePointsEvent, LIVE_POINTS};
use crate::state::{get_assistant, AppState};

pub const SETTING_KEY: &str = "live_points";
const TICK: Duration = Duration::from_millis(1_000);

struct Active {
    pending: Arc<Mutex<Vec<TranscriptSegment>>>,
    stop: Arc<AtomicBool>,
}

pub struct LivePoints {
    enabled: AtomicBool,
    clock: Instant,
    active: Mutex<Option<Active>>,
    /// The list as it stands, so the UI can show it after a reload.
    view: Mutex<LivePointsEvent>,
}

impl LivePoints {
    pub fn new(enabled: bool) -> Self {
        LivePoints {
            enabled: AtomicBool::new(enabled),
            clock: Instant::now(),
            active: Mutex::new(None),
            view: Mutex::new(LivePointsEvent { enabled, ..Default::default() }),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn view(&self) -> LivePointsEvent {
        let mut v = self.view.lock().unwrap().clone();
        v.enabled = self.enabled();
        v
    }

    /// Switch on or off, telling the UI (also while nothing is recording).
    pub fn set_enabled(&self, app: &AppHandle, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
        let ev = {
            let mut v = self.view.lock().unwrap();
            v.enabled = on;
            v.updating = false;
            v.clone()
        };
        let _ = app.emit(LIVE_POINTS, ev);
    }

    fn now_ms(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }

    /// A recording started: begin watching the transcript.
    pub fn begin(self: &Arc<Self>, app: &AppHandle, meeting_id: i64) {
        self.end();
        let pending: Arc<Mutex<Vec<TranscriptSegment>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        *self.active.lock().unwrap() = Some(Active { pending: pending.clone(), stop: stop.clone() });
        *self.view.lock().unwrap() = LivePointsEvent { meeting_id, enabled: self.enabled(), ..Default::default() };

        let (this, app) = (self.clone(), app.clone());
        std::thread::Builder::new()
            .name("meetesh-live-points".into())
            .spawn(move || this.run(&app, meeting_id, pending, stop))
            .expect("spawn live points thread");
    }

    /// New live transcript segments for the recording in progress.
    pub fn on_segments(&self, segments: &[TranscriptSegment]) {
        if let Some(a) = self.active.lock().unwrap().as_ref() {
            a.pending.lock().unwrap().extend_from_slice(segments);
        }
    }

    pub fn end(&self) {
        if let Some(a) = self.active.lock().unwrap().take() {
            a.stop.store(true, Ordering::Relaxed);
        }
    }

    fn run(&self, app: &AppHandle, meeting_id: i64, pending: Arc<Mutex<Vec<TranscriptSegment>>>, stop: Arc<AtomicBool>) {
        let mut sched = Scheduler::default();
        let mut current = Points::default();
        let mut warned = false;
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(TICK);
            let st = app.state::<AppState>();
            let installed = st.assist_model().map(|m| st.models.is_installed(m)).unwrap_or(false);
            let words: usize = pending.lock().unwrap().iter().map(|s| s.text.split_whitespace().count()).sum();
            if !self.enabled() || !installed || !sched.should_start(self.now_ms(), words) {
                continue;
            }
            sched.started();
            let batch: Vec<TranscriptSegment> = std::mem::take(&mut *pending.lock().unwrap());
            self.publish(app, meeting_id, &current, true);
            let began = Instant::now();
            let outcome = (|| {
                let assistant = get_assistant(app)?;
                let run = SummaryRun {
                    cancel: stop.clone(),
                    yield_to: Some(st.asr_pending.clone()),
                    gate: Some(st.gate.clone()),
                    cpu_bound: false, // for the recording, so a recording never holds it back; a hint does
                    on_pause: None,
                };
                update_points(assistant.as_ref(), &current, &batch, &run)
            })();
            match outcome {
                Ok(p) => {
                    log::info!("live points for meeting {meeting_id}: {} points in {} ms", p.points.len(), began.elapsed().as_millis());
                    current = p;
                }
                Err(Error::Cancelled) => {}
                Err(e) => {
                    // Keep the lines for the next try; say so once in the log, never the text.
                    if !warned {
                        log::warn!("live points update failed: {e}");
                        warned = true;
                    }
                    let mut p = pending.lock().unwrap();
                    let mut again = batch;
                    again.append(&mut p);
                    *p = again;
                }
            }
            sched.finished(self.now_ms(), began.elapsed().as_millis() as u64);
            self.publish(app, meeting_id, &current, false);
        }
    }

    fn publish(&self, app: &AppHandle, meeting_id: i64, p: &Points, updating: bool) {
        let ev = LivePointsEvent { meeting_id, enabled: self.enabled(), now: p.now.clone(), points: p.points.clone(), updating };
        *self.view.lock().unwrap() = ev.clone();
        let _ = app.emit(LIVE_POINTS, ev);
    }
}

/// The saved setting; on unless it was switched off.
pub fn load_enabled(db: &meetesh_core::Db) -> bool {
    db.get_setting(SETTING_KEY).ok().flatten().as_deref() != Some("off")
}
