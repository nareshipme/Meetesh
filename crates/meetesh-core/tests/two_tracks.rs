//! A call recorded without hardware: the microphone and the other participants' audio are two
//! tracks on one clock, transcribed separately and merged into one transcript.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use meetesh_core::asr::chunker::ChunkerConfig;
use meetesh_core::asr::run::transcribe_track;
use meetesh_core::audio::sinks::WavSink;
use meetesh_core::audio::{system_dir, wav, FrameSink, Pusher, RecordingSession, TARGET_RATE};
use meetesh_core::{Db, Source, TranscriptSegment, Transcriber};

/// "Understands" any speech as the word "speech" with the chunk's length.
struct Fake;
impl Transcriber for Fake {
    fn model_id(&self) -> &str {
        "fake"
    }
    fn transcribe(&self, s: &[f32]) -> meetesh_core::Result<Vec<TranscriptSegment>> {
        Ok(vec![TranscriptSegment::new(0, (s.len() / 16) as i64, "speech")])
    }
}

fn tone(ms: usize) -> Vec<f32> {
    (0..ms * 16).map(|i| (i as f32 * 0.3).sin() * 0.3).collect()
}

fn feed(audio: &[f32], mut p: Pusher) {
    for c in audio.chunks(160) {
        p.push_interleaved(c);
    }
}

#[test]
fn two_tracks_share_one_clock_and_merge_into_one_transcript() {
    let root = std::env::temp_dir().join(format!("meetesh-2tracks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let db = Db::open_in_memory().unwrap();
    let m = db.start_recording_meeting(Some("Call"), &root).unwrap();
    let mic_dir = Path::new(&m.audio_dir).to_path_buf();
    let sys_dir = system_dir(&mic_dir);

    // You speak at 0.5 s to 1.7 s, then stay quiet until 6 s.
    let mut mic_audio = vec![0.0; 8_000];
    mic_audio.extend(tone(1_200));
    mic_audio.extend(vec![0.0; 16_000 * 6 - mic_audio.len()]);
    let mic = RecordingSession::start(vec![Box::new(WavSink::new(&mic_dir).unwrap()) as Box<dyn FrameSink>], 4096);
    feed(&mic_audio, mic.pusher(16_000, 1));

    // The other side is silent for the first 3 s (a loopback API delivers nothing), then speaks
    // for 1.2 s; its first block arrives at 3 s. The pusher keeps real time, so the track still
    // lines up with the microphone.
    let sys = RecordingSession::start(vec![Box::new(WavSink::new(&sys_dir).unwrap()) as Box<dyn FrameSink>], 4096);
    let late = Instant::now() - Duration::from_millis(3_010);
    feed(&tone(1_200), sys.pusher(16_000, 1).fill_gaps_from(late));

    let mic_report = mic.stop();
    let sys_report = sys.stop();
    assert_eq!((mic_report.dropped_frames, sys_report.dropped_frames), (0, 0));

    // The system track is on the meeting clock: silence for 3 s, then the speech.
    let sys_ms = (sys_report.samples * 1000 / TARGET_RATE as u64) as i64;
    assert!((4_100..=4_400).contains(&sys_ms), "system track is {sys_ms} ms");

    // The microphone is the recording; the system folder neither changes its length nor its file list.
    assert_eq!(wav::duration_ms(&mic_dir).unwrap(), 6_000);
    assert!(!wav::list_segments(&mic_dir).unwrap().iter().any(|p| p.starts_with(&sys_dir)));
    db.finish_recording(m.id, 6_000).unwrap();
    assert!(db.set_system_audio(m.id).unwrap().has_system_audio);

    let cancel = AtomicBool::new(false);
    let cfg = ChunkerConfig::default;
    let mut all = transcribe_track(&mic_dir, Source::Mic, &Fake, cfg(), &cancel, |_| {}, |_| {}).unwrap();
    all.extend(transcribe_track(&sys_dir, Source::System, &Fake, cfg(), &cancel, |_| {}, |_| {}).unwrap());
    all.sort_by_key(|s| s.start_ms);
    db.replace_transcript(m.id, "fake", &all).unwrap();

    let t = db.get_transcript(m.id).unwrap();
    assert_eq!(t.iter().map(|s| s.source).collect::<Vec<_>>(), [Source::Mic, Source::System]);
    assert!(t[0].start_ms < 1_500, "you spoke first: {}", t[0].start_ms);
    assert!(t[1].start_ms >= 2_800, "the other side spoke after 3 s: {}", t[1].start_ms);

    // Deleting the meeting removes both tracks.
    db.delete_meeting(m.id).unwrap();
    assert!(!sys_dir.exists() && !mic_dir.exists());
    std::fs::remove_dir_all(root).ok();
}
