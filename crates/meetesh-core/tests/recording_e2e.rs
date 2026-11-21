//! End-to-end recording without hardware: synthetic 48 kHz stereo -> resample -> WAV segments -> DB.

use meetesh_core::audio::sinks::{LevelSink, WavSink};
use meetesh_core::audio::{wav, FrameSink, RecordingSession, TARGET_RATE};
use meetesh_core::Db;
use std::sync::{Arc, Mutex};

#[test]
fn ten_seconds_of_stereo_48k_is_recorded_and_recoverable() {
    let root = std::env::temp_dir().join(format!("meetesh-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let db = Db::open_in_memory().unwrap();
    let meeting = db.start_recording_meeting(Some("E2E"), &root).unwrap();

    let levels = Arc::new(Mutex::new(Vec::<f32>::new()));
    let l = levels.clone();
    let sinks: Vec<Box<dyn FrameSink>> = vec![
        Box::new(WavSink::new(&meeting.audio_dir).unwrap()),
        Box::new(LevelSink::new(move |v| l.lock().unwrap().push(v))),
    ];
    let session = RecordingSession::start(sinks, 256);
    let mut pusher = session.pusher(48_000, 2);

    // 10 s of a 440 Hz tone delivered like a sound card does: 10 ms callbacks, interleaved stereo.
    let cb = 480;
    for n in 0..1000 {
        let buf: Vec<f32> = (0..cb)
            .flat_map(|i| {
                let t = (n * cb + i) as f32 / 48_000.0;
                let v = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5;
                [v, v]
            })
            .collect();
        pusher.push_interleaved(&buf);
        if n % 50 == 0 {
            std::thread::sleep(std::time::Duration::from_millis(1)); // let the consumer breathe
        }
    }
    let report = session.stop();

    assert_eq!(report.dropped_frames, 0, "no audio may be dropped");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let expected = 10 * TARGET_RATE as u64;
    assert!((report.samples as i64 - expected as i64).abs() <= 2, "samples {}", report.samples);

    let ms = (report.samples * 1000 / TARGET_RATE as u64) as i64;
    let m = db.finish_recording(meeting.id, ms).unwrap();
    assert_eq!(m.status, "stopped");
    assert_eq!(wav::list_segments(std::path::Path::new(&m.audio_dir)).unwrap().len(), 2); // 5 s segments

    let levels = levels.lock().unwrap();
    assert!(levels.len() >= 99, "level updates: {}", levels.len());
    assert!(levels.iter().all(|v| (v - 0.354).abs() < 0.03), "RMS of a 0.5 sine is ~0.354");

    // Playable merged file, right length.
    let out = std::path::Path::new(&m.audio_dir).join("meeting.wav");
    let n = wav::merge_segments(std::path::Path::new(&m.audio_dir), &out).unwrap();
    assert_eq!(n, report.samples);
    std::fs::remove_dir_all(root).ok();
}
