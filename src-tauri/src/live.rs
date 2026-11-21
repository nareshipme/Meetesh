//! Glue between the core's live transcriber and the app: saves segments, notifies the UI, and
//! decides what happens when the recording and the live worker have both finished.

use std::sync::{Arc, Mutex};

use meetesh_core::asr::live::{live_chunker_config, LiveReport, LiveSettle, LiveTranscriber};
use meetesh_core::Transcriber;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::transcription::start_transcription;
use crate::events::{LiveDoneEvent, LiveSegmentsEvent, LIVE_DONE, LIVE_SEGMENTS};
use crate::state::{get_transcriber, AppState};

/// Build the live transcription sink for one recording. Segments are saved as they arrive and
/// pushed to the UI. Never logs transcript text.
pub fn live_sink(app: &AppHandle, meeting_id: i64, model_id: String, outcome: Arc<Mutex<LiveSettle>>) -> LiveTranscriber {
    let (load_app, seg_app, done_app) = (app.clone(), app.clone(), app.clone());
    LiveTranscriber::start(
        live_chunker_config(),
        8,
        Box::new(move || Ok(get_transcriber(&load_app)? as Arc<dyn Transcriber>)),
        move |segments| {
            let st = seg_app.state::<AppState>();
            {
                let db = st.db.lock().unwrap();
                for s in &segments {
                    if let Err(e) = db.append_transcript_segment(meeting_id, &model_id, s) {
                        log::warn!("could not save live segment: {e}");
                    }
                }
            }
            let _ = seg_app.emit(LIVE_SEGMENTS, LiveSegmentsEvent { meeting_id, segments });
        },
        move |report| {
            log::info!(
                "live transcription for meeting {meeting_id}: {} chunks, {} dropped, {} errors",
                report.chunks,
                report.dropped_chunks,
                report.errors.len()
            );
            for e in &report.errors {
                log::warn!("live transcription error: {e}");
            }
            let settled = outcome.lock().unwrap().worker_finished(report);
            if let Some(report) = settled {
                finish_live(&done_app, meeting_id, report);
            }
        },
    )
}

/// Runs once both the recording and the live worker have finished, in either order.
pub fn finish_live(app: &AppHandle, meeting_id: i64, report: LiveReport) {
    // The live transcript has gaps (transcriber fell behind, or the model failed): fill them from the audio.
    let full_pass_started = report.needs_full_pass() && start_transcription(app, meeting_id).is_ok();
    let _ = app.emit(LIVE_DONE, LiveDoneEvent { meeting_id, full_pass_started });
}
