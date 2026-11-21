//! Ready-made sinks: disk writer and level meter.

use std::path::PathBuf;

use crate::audio::pipeline::FrameSink;
use crate::audio::wav::SegmentWriter;
use crate::audio::TARGET_RATE;
use crate::Result;

/// Writes the recording to segmented WAV files.
pub struct WavSink {
    writer: SegmentWriter,
}

impl WavSink {
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self> {
        Ok(WavSink { writer: SegmentWriter::new(dir)? })
    }
}

impl FrameSink for WavSink {
    fn on_samples(&mut self, samples: &[f32], _start: u64) -> Result<()> {
        self.writer.push(samples)
    }
    fn finish(&mut self) -> Result<()> {
        self.writer.finish().map(|_| ())
    }
}

/// Calls `on_level` about 10 times per second with an RMS level in 0.0 ..= 1.0.
pub struct LevelSink {
    on_level: Box<dyn Fn(f32) + Send>,
    window: usize,
    sum_sq: f64,
    n: usize,
}

impl LevelSink {
    pub fn new(on_level: impl Fn(f32) + Send + 'static) -> Self {
        LevelSink { on_level: Box::new(on_level), window: (TARGET_RATE / 10) as usize, sum_sq: 0.0, n: 0 }
    }
}

impl FrameSink for LevelSink {
    fn on_samples(&mut self, samples: &[f32], _start: u64) -> Result<()> {
        for &s in samples {
            self.sum_sq += (s as f64) * (s as f64);
            self.n += 1;
            if self.n >= self.window {
                let rms = (self.sum_sq / self.n as f64).sqrt() as f32;
                (self.on_level)(rms.clamp(0.0, 1.0));
                self.sum_sq = 0.0;
                self.n = 0;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn level_reports_ten_times_a_second() {
        let levels = Arc::new(Mutex::new(vec![]));
        let l = levels.clone();
        let mut sink = LevelSink::new(move |v| l.lock().unwrap().push(v));
        sink.on_samples(&vec![0.5; 16_000], 0).unwrap();
        let v = levels.lock().unwrap();
        assert_eq!(v.len(), 10);
        assert!(v.iter().all(|x| (x - 0.5).abs() < 1e-4));
    }

    #[test]
    fn silence_is_zero() {
        let levels = Arc::new(Mutex::new(vec![]));
        let l = levels.clone();
        let mut sink = LevelSink::new(move |v| l.lock().unwrap().push(v));
        sink.on_samples(&vec![0.0; 3200], 0).unwrap();
        assert!(levels.lock().unwrap().iter().all(|x| *x == 0.0));
    }
}
