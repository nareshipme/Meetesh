//! Crash-safe audio storage: the recording is split into fixed-length WAV segments
//! (`seg-000001.wav`, ...). The open segment's header is refreshed about once per second,
//! so after a crash or power loss at most about a second of audio is unreadable (NFR-09).

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavSpec, WavWriter};

use crate::audio::TARGET_RATE;
use crate::error::{Error, Result};

pub const SEGMENT_SECONDS: u64 = 5;
const FLUSH_EVERY_SAMPLES: u64 = TARGET_RATE as u64; // 1 s
const WAV_HEADER_BYTES: u64 = 44;

fn spec() -> WavSpec {
    WavSpec { channels: 1, sample_rate: TARGET_RATE, bits_per_sample: 16, sample_format: SampleFormat::Int }
}

fn hound_err(e: hound::Error) -> Error {
    Error::Other(format!("wav: {e}"))
}

pub fn segment_path(dir: &Path, index: u32) -> PathBuf {
    dir.join(format!("seg-{index:06}.wav"))
}

pub struct SegmentWriter {
    dir: PathBuf,
    seg_samples: u64,
    index: u32,
    current: Option<WavWriter<BufWriter<File>>>,
    in_segment: u64,
    since_flush: u64,
    total: u64,
}

impl SegmentWriter {
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self> {
        Self::with_segment_seconds(dir, SEGMENT_SECONDS)
    }

    pub fn with_segment_seconds(dir: impl Into<PathBuf>, secs: u64) -> Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        Ok(SegmentWriter {
            dir,
            seg_samples: secs * TARGET_RATE as u64,
            index: 0,
            current: None,
            in_segment: 0,
            since_flush: 0,
            total: 0,
        })
    }

    pub fn total_samples(&self) -> u64 {
        self.total
    }

    pub fn push(&mut self, samples: &[f32]) -> Result<()> {
        for &s in samples {
            if self.current.is_none() {
                self.index += 1;
                let w = WavWriter::create(segment_path(&self.dir, self.index), spec()).map_err(hound_err)?;
                self.current = Some(w);
                self.in_segment = 0;
            }
            let w = self.current.as_mut().unwrap();
            let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            w.write_sample(v).map_err(hound_err)?;
            self.in_segment += 1;
            self.since_flush += 1;
            self.total += 1;
            if self.in_segment >= self.seg_samples {
                self.current.take().unwrap().finalize().map_err(hound_err)?;
            } else if self.since_flush >= FLUSH_EVERY_SAMPLES {
                self.current.as_mut().unwrap().flush().map_err(hound_err)?;
                self.since_flush = 0;
            }
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Result<u64> {
        if let Some(w) = self.current.take() {
            w.finalize().map_err(hound_err)?;
        }
        Ok(self.total)
    }
}

/// Segment files in a recording directory, oldest first.
pub fn list_segments(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut v: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map_or(false, |n| n.starts_with("seg-") && n.ends_with(".wav"))
        })
        .collect();
    v.sort();
    Ok(v)
}

/// Duration from file sizes. Works even for a segment whose header was never finalized.
pub fn duration_ms(dir: &Path) -> Result<u64> {
    let mut samples = 0u64;
    for p in list_segments(dir)? {
        samples += fs::metadata(&p)?.len().saturating_sub(WAV_HEADER_BYTES) / 2;
    }
    Ok(samples * 1000 / TARGET_RATE as u64)
}

/// Read one segment as f32 samples. Tolerates a segment cut off by a crash.
pub fn read_segment(path: &Path) -> Result<Vec<f32>> {
    let bytes = fs::read(path)?;
    if (bytes.len() as u64) < WAV_HEADER_BYTES {
        return Ok(vec![]);
    }
    // Decode PCM straight from the data section; ignores a stale length field after a crash.
    Ok(bytes[WAV_HEADER_BYTES as usize..]
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / i16::MAX as f32)
        .collect())
}

/// Merge all segments into one playable WAV file.
pub fn merge_segments(dir: &Path, out: &Path) -> Result<u64> {
    let mut w = WavWriter::create(out, spec()).map_err(hound_err)?;
    let mut n = 0u64;
    for p in list_segments(dir)? {
        for s in read_segment(&p)? {
            w.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).map_err(hound_err)?;
            n += 1;
        }
    }
    w.finalize().map_err(hound_err)?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("meetesh-wav-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn tone(n: usize) -> Vec<f32> {
        (0..n).map(|i| (i as f32 * 0.05).sin() * 0.4).collect()
    }

    #[test]
    fn splits_into_segments_and_reads_back() {
        let dir = tmp("split");
        let mut w = SegmentWriter::with_segment_seconds(&dir, 1).unwrap();
        let input = tone(16_000 * 2 + 4_000); // 2.25 s
        w.push(&input).unwrap();
        w.finish().unwrap();
        let segs = list_segments(&dir).unwrap();
        assert_eq!(segs.len(), 3);
        let back: Vec<f32> = segs.iter().flat_map(|p| read_segment(p).unwrap()).collect();
        assert_eq!(back.len(), input.len());
        assert!(back.iter().zip(&input).all(|(a, b)| (a - b).abs() < 1e-3));
        assert_eq!(duration_ms(&dir).unwrap(), 2250);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn crash_loses_at_most_the_unflushed_tail() {
        let dir = tmp("crash");
        let mut w = SegmentWriter::with_segment_seconds(&dir, 5).unwrap();
        w.push(&tone(16_000 * 3 + 5_000)).unwrap(); // 3.3 s, then "crash": never finish()
        let flushed_floor = 3 * 16_000; // a flush happened at each full second
        let got: usize = list_segments(&dir).unwrap().iter().map(|p| read_segment(p).unwrap().len()).sum();
        assert!(got >= flushed_floor, "recovered {got} samples");
        std::mem::forget(w); // simulate no destructor running
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn merge_produces_valid_wav() {
        let dir = tmp("merge");
        let mut w = SegmentWriter::with_segment_seconds(&dir, 1).unwrap();
        w.push(&tone(16_000 + 800)).unwrap();
        w.finish().unwrap();
        let out = dir.join("all.wav");
        let n = merge_segments(&dir, &out).unwrap();
        assert_eq!(n, 16_800);
        let r = hound::WavReader::open(&out).unwrap();
        assert_eq!(r.spec().sample_rate, TARGET_RATE);
        assert_eq!(r.len(), 16_800);
        fs::remove_dir_all(dir).ok();
    }
}
