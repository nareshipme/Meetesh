//! macOS system-audio capture with ScreenCaptureKit (macOS 13 or newer).
//!
//! ScreenCaptureKit is the supported way to hear what the computer plays, without installing a
//! virtual audio driver. It needs the "Screen & System Audio Recording" permission; it records
//! audio only (the one-pixel video it requires is thrown away). This module is self-contained:
//! it hands interleaved f32 audio and its format to a callback, and returns a stop function.

use std::ptr::{null_mut, NonNull};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::Mutex;
use std::time::Duration;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_audio_types::{
    kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved, AudioBufferList,
};
use objc2_core_foundation::CFRetained;
use objc2_core_media::{
    CMAudioFormatDescriptionGetStreamBasicDescription, CMBlockBuffer, CMSampleBuffer, CMTime,
};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutput,
    SCStreamOutputType, SCWindow,
};

pub type StopFn = Box<dyn FnOnce() + Send>;

/// Called for every buffer with interleaved samples, the sample rate and the channel count.
pub type OnAudio = Box<dyn FnMut(&[f32], u32, u16) + Send>;

const WAIT: Duration = Duration::from_secs(10);
/// What we ask ScreenCaptureKit for. The real format is read from every buffer, so a different
/// answer is handled; this only keeps the common case simple.
const WANT_RATE: isize = 48_000;
const WANT_CHANNELS: isize = 2;

/// ScreenCaptureKit objects are documented as thread-safe; this lets them cross threads.
struct Shared<T>(T);
unsafe impl<T> Send for Shared<T> {}

impl<T> Shared<T> {
    /// A method (not `.0`) so closures capture the whole wrapper, which is `Send`.
    fn into_inner(self) -> T {
        self.0
    }
}

struct Ivars {
    on_audio: Mutex<OnAudio>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "MeeteshSystemAudioOutput"]
    #[ivars = Ivars]
    struct AudioOutput;

    unsafe impl NSObjectProtocol for AudioOutput {}

    unsafe impl SCStreamOutput for AudioOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_did_output_sample_buffer_of_type(
            &self,
            _stream: &SCStream,
            buffer: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind != SCStreamOutputType::Audio {
                return; // the (tiny) video frames are not wanted
            }
            let mut pcm = Vec::new();
            if let Some((rate, channels)) = unsafe { read_pcm(buffer, &mut pcm) } {
                if let Ok(mut f) = self.ivars().on_audio.lock() {
                    (f)(&pcm, rate, channels);
                }
            }
        }
    }
);

impl AudioOutput {
    fn new(on_audio: OnAudio) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { on_audio: Mutex::new(on_audio) });
        unsafe { msg_send![super(this), init] }
    }
}

/// Copy the audio in `buffer` into `out` as interleaved f32 (one or two channels).
/// Returns the sample rate and channel count, or `None` if the buffer cannot be read.
unsafe fn read_pcm(buffer: &CMSampleBuffer, out: &mut Vec<f32>) -> Option<(u32, u16)> {
    let fmt = buffer.format_description()?;
    let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(&fmt);
    if asbd.is_null() {
        return None;
    }
    let asbd = &*asbd;
    let is_float = asbd.mFormatFlags & kAudioFormatFlagIsFloat != 0;
    let planar = asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0;
    let channels = asbd.mChannelsPerFrame as usize;
    if channels == 0 || asbd.mBitsPerChannel != if is_float { 32 } else { 16 } {
        return None;
    }

    // First ask how big the buffer list is, then fetch it (the audio stays valid while `block` lives).
    let mut needed = 0usize;
    let _ = buffer.audio_buffer_list_with_retained_block_buffer(&mut needed, null_mut(), 0, None, None, 0, null_mut());
    if needed == 0 {
        return None;
    }
    let mut mem = vec![0u64; needed.div_ceil(8)]; // 8-byte aligned storage for the list
    let list = mem.as_mut_ptr() as *mut AudioBufferList;
    let mut block: *mut CMBlockBuffer = null_mut();
    let status = buffer.audio_buffer_list_with_retained_block_buffer(&mut needed, list, mem.len() * 8, None, None, 1, &mut block);
    let _block = NonNull::new(block).map(|b| CFRetained::from_raw(b)); // released when this returns
    if status != 0 {
        return None;
    }

    let list = &*list;
    let bufs = std::slice::from_raw_parts(list.mBuffers.as_ptr(), list.mNumberBuffers as usize);
    let sample = |b: &objc2_core_audio_types::AudioBuffer, i: usize| -> f32 {
        if is_float {
            *(b.mData as *const f32).add(i)
        } else {
            *(b.mData as *const i16).add(i) as f32 / i16::MAX as f32
        }
    };
    let bytes_per_sample = if is_float { 4 } else { 2 };
    let out_ch = channels.min(2);
    out.clear();
    if planar {
        // One buffer per channel.
        let frames = bufs.first().map_or(0, |b| b.mDataByteSize as usize / bytes_per_sample);
        if bufs.len() < channels || bufs.iter().any(|b| b.mData.is_null()) {
            return None;
        }
        out.reserve(frames * out_ch);
        for i in 0..frames {
            for b in bufs.iter().take(out_ch) {
                out.push(sample(b, i));
            }
        }
    } else {
        let b = bufs.first()?;
        if b.mData.is_null() {
            return None;
        }
        let frames = b.mDataByteSize as usize / bytes_per_sample / channels;
        out.reserve(frames * out_ch);
        for f in 0..frames {
            for c in 0..out_ch {
                out.push(sample(b, f * channels + c));
            }
        }
    }
    Some((asbd.mSampleRate as u32, out_ch as u16))
}

fn permission_hint(e: &NSError) -> String {
    format!(
        "Could not capture system audio: {}. Allow Meetesh under System Settings > Privacy & Security > Screen & System Audio Recording, then restart the app.",
        e.localizedDescription()
    )
}

/// Start capturing everything the computer plays (except Meetesh itself).
pub fn start(on_audio: OnAudio) -> Result<StopFn, String> {
    unsafe {
        // 1. What can be captured? This is also where the permission prompt appears.
        let (tx, rx) = channel::<Result<Shared<Retained<SCShareableContent>>, String>>();
        let handler = RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
            let r = match Retained::retain(content) {
                Some(c) => Ok(Shared(c)),
                None => Err(match err.as_ref() {
                    Some(e) => permission_hint(e),
                    None => "Could not capture system audio: no answer from macOS".to_string(),
                }),
            };
            let _ = tx.send(r);
        });
        SCShareableContent::getShareableContentWithCompletionHandler(&handler);
        let content = match rx.recv_timeout(WAIT) {
            Ok(r) => r?.into_inner(),
            Err(RecvTimeoutError::Timeout) => {
                return Err("Could not capture system audio: macOS did not answer in time (is the permission prompt waiting?)".into())
            }
            Err(_) => return Err("Could not capture system audio".into()),
        };
        let display: Retained<SCDisplay> = content
            .displays()
            .firstObject()
            .ok_or_else(|| "Could not capture system audio: no display found".to_string())?;

        // 2. An audio-only stream (a 2x2 video at 1 fps is the smallest ScreenCaptureKit allows).
        let no_windows: Retained<NSArray<SCWindow>> = NSArray::new();
        let filter = SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &display, &no_windows);
        let config = SCStreamConfiguration::new();
        config.setCapturesAudio(true);
        config.setExcludesCurrentProcessAudio(true);
        config.setSampleRate(WANT_RATE);
        config.setChannelCount(WANT_CHANNELS);
        config.setWidth(2);
        config.setHeight(2);
        config.setMinimumFrameInterval(CMTime::new(1, 1));
        let stream = SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, None);

        let output = AudioOutput::new(on_audio);
        let queue = DispatchQueue::new("app.meetesh.system-audio", None);
        stream
            .addStreamOutput_type_sampleHandlerQueue_error(ProtocolObject::from_ref(&*output), SCStreamOutputType::Audio, Some(&queue))
            .map_err(|e| permission_hint(&e))?;

        // 3. Go.
        let (tx, rx) = channel::<Option<String>>();
        let started = RcBlock::new(move |err: *mut NSError| {
            let _ = tx.send(err.as_ref().map(permission_hint));
        });
        stream.startCaptureWithCompletionHandler(Some(&started));
        match rx.recv_timeout(WAIT) {
            Ok(None) => {}
            Ok(Some(e)) => return Err(e),
            Err(_) => return Err("Could not capture system audio: macOS did not start the stream in time".into()),
        }

        let keep = Shared((stream, output, queue));
        Ok(Box::new(move || {
            let (stream, _output, _queue) = keep.into_inner();
            let (tx, rx) = channel::<()>();
            let stopped = RcBlock::new(move |_err: *mut NSError| {
                let _ = tx.send(());
            });
            stream.stopCaptureWithCompletionHandler(Some(&stopped));
            let _ = rx.recv_timeout(WAIT);
        }))
    }
}
