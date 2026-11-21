//! Running a `Transcriber` over audio: chunk at pauses, transcribe each chunk, offset timestamps.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::asr::chunker::{Chunk, Chunker, ChunkerConfig};
use crate::asr::{is_noise_marker, TranscriptSegment, Transcriber};
use crate::audio::wav;
use crate::{Error, Result};

/// Transcribe one chunk; returns segments with timestamps relative to the recording start.
pub fn transcribe_chunk(t: &dyn Transcriber, chunk: &Chunk) -> Result<Vec<TranscriptSegment>> {
    let offset = chunk.start_ms();
    Ok(t.transcribe(&chunk.samples)?
        .into_iter()
        .filter(|s| !is_noise_marker(&s.text))
        .map(|s| TranscriptSegment {
            start_ms: s.start_ms + offset,
            end_ms: s.end_ms + offset,
            text: s.text.trim().to_string(),
        })
        .collect())
}

/// Transcribe a finished recording from its segment files.
///
/// `on_progress` gets 0.0..=1.0; `on_segments` gets each batch as it is produced. Returns
/// `Error::Cancelled` if `cancel` is set (segments already delivered stay delivered).
pub fn transcribe_recording(
    dir: &Path,
    t: &dyn Transcriber,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f32),
    mut on_segments: impl FnMut(&[TranscriptSegment]),
) -> Result<Vec<TranscriptSegment>> {
    let total = (wav::duration_ms(dir)? * crate::audio::TARGET_RATE as u64 / 1000).max(1) as f32;
    let mut chunker = Chunker::new(ChunkerConfig::default());
    let mut all = vec![];
    let mut done = 0f32;

    let mut handle = |chunk: Chunk, all: &mut Vec<TranscriptSegment>| -> Result<()> {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let segs = transcribe_chunk(t, &chunk)?;
        if !segs.is_empty() {
            on_segments(&segs);
            all.extend(segs);
        }
        Ok(())
    };

    for path in wav::list_segments(dir)? {
        let samples = wav::read_segment(&path)?;
        done += samples.len() as f32;
        for chunk in chunker.push(&samples) {
            handle(chunk, &mut all)?;
        }
        on_progress((done / total).min(1.0));
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
    }
    if let Some(chunk) = chunker.flush() {
        handle(chunk, &mut all)?;
    }
    on_progress(1.0);
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::wav::SegmentWriter;
    use std::sync::atomic::AtomicUsize;

    /// Says how many ms of audio it was given.
    struct Fake;
    impl Transcriber for Fake {
        fn model_id(&self) -> &str {
            "fake"
        }
        fn transcribe(&self, samples: &[f32]) -> Result<Vec<TranscriptSegment>> {
            let ms = (samples.len() / 16) as i64;
            Ok(vec![
                TranscriptSegment { start_ms: 0, end_ms: ms, text: format!(" heard {ms} ms ") },
                TranscriptSegment { start_ms: 0, end_ms: 1, text: "[BLANK_AUDIO]".into() },
            ])
        }
    }

    fn write_recording(name: &str, parts: &[(bool, usize)]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("meetesh-run-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut w = SegmentWriter::with_segment_seconds(&dir, 2).unwrap();
        for &(talk, ms) in parts {
            let s: Vec<f32> = (0..ms * 16).map(|i| if talk { (i as f32 * 0.3).sin() * 0.3 } else { 0.0 }).collect();
            w.push(&s).unwrap();
        }
        w.finish().unwrap();
        dir
    }

    #[test]
    fn transcribes_across_segment_files_with_offsets_and_progress() {
        let dir = write_recording("ok", &[(false, 500), (true, 1500), (false, 1000), (true, 1200), (false, 800)]);
        let mut progress = vec![];
        let mut batches = 0;
        let out = transcribe_recording(&dir, &Fake, &AtomicBool::new(false), |p| progress.push(p), |_| batches += 1).unwrap();
        assert_eq!(out.len(), 2, "noise markers dropped: {out:?}");
        assert!(out[0].text.starts_with("heard") && !out[0].text.starts_with(' '));
        assert!(out[1].start_ms > out[0].start_ms);
        assert!((out[1].start_ms - 3_000).abs() < 400, "offset {}", out[1].start_ms);
        assert_eq!(batches, 2);
        assert!(progress.windows(2).all(|w| w[0] <= w[1]) && *progress.last().unwrap() == 1.0);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn cancel_stops_early() {
        let dir = write_recording("cancel", &[(true, 1000), (false, 1000), (true, 1000), (false, 1000), (true, 1000), (false, 1000)]);
        let cancel = AtomicBool::new(false);
        let seen = AtomicUsize::new(0);
        let r = transcribe_recording(&dir, &Fake, &cancel, |_| {}, |_| {
            if seen.fetch_add(1, Ordering::Relaxed) == 0 {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert!(matches!(r, Err(Error::Cancelled)));
        assert_eq!(seen.load(Ordering::Relaxed), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn silent_recording_gives_empty_transcript() {
        let dir = write_recording("silent", &[(false, 4_000)]);
        let out = transcribe_recording(&dir, &Fake, &AtomicBool::new(false), |_| {}, |_| {}).unwrap();
        assert!(out.is_empty());
        std::fs::remove_dir_all(dir).ok();
    }
}
