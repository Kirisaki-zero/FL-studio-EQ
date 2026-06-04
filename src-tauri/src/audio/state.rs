use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use crossbeam_channel::Sender;

/// Oscilloscope ring buffer capacity (samples). ~93ms @ 44100 Hz.
pub const OSC_SIZE: usize = 4096;

/// Sentinel value meaning "no seek is pending".
pub const NO_SEEK: u64 = u64::MAX;

#[derive(Clone, serde::Deserialize, PartialEq, Debug)]
pub struct BandConfig {
    pub freq: f32,
    pub gain: f32,
    pub q: f32,
    pub shape: String,
    pub muted: bool,
}

#[derive(Clone, serde::Deserialize, PartialEq, Debug)]
pub struct CompressorConfig {
    pub thresh: f32,
    pub ratio: f32,
    pub attack: f32,
    pub release: f32,
    pub knee: f32,
    pub makeup: f32,
    pub bypassed: bool,
}

impl Default for CompressorConfig {
    fn default() -> Self {
        Self {
            thresh: -18.0,
            ratio: 4.0,
            attack: 10.0,
            release: 150.0,
            knee: 6.0,
            makeup: 3.0,
            bypassed: false,
        }
    }
}

#[derive(Clone, serde::Deserialize, PartialEq, Debug)]
pub struct ReverbConfig {
    pub size: f32,
    pub damp: f32,
    pub diff: f32,
    pub width: f32,
    pub wet: f32,
    pub pre: f32, // milliseconds
    pub bypassed: bool,
}

impl Default for ReverbConfig {
    fn default() -> Self {
        Self {
            size: 0.65,
            damp: 0.4,
            diff: 0.8,
            width: 1.0,
            wet: 0.2,
            pre: 20.0,
            bypassed: true,
        }
    }
}

#[derive(Clone, serde::Deserialize, PartialEq, Debug)]
pub struct DelayConfig {
    pub time: f32,
    pub feed: f32,
    pub sync: f32,
    pub pan: f32,
    pub cut: f32,
    pub wet: f32,
    pub bypassed: bool,
}

impl Default for DelayConfig {
    fn default() -> Self {
        Self {
            time: 375.0,
            feed: 0.45,
            sync: 0.0,
            pan: 0.0,
            cut: 2000.0,
            wet: 0.25,
            bypassed: true,
        }
    }
}

#[derive(Clone, serde::Deserialize, PartialEq, Debug)]
pub struct ChorusConfig {
    pub depth: f32,
    pub rate: f32,
    pub phase: f32,
    pub wet: f32,
    pub bypassed: bool,
}

impl Default for ChorusConfig {
    fn default() -> Self {
        Self {
            depth: 0.5,
            rate: 0.3,
            phase: 90.0,
            wet: 0.4,
            bypassed: true,
        }
    }
}

#[derive(Clone, serde::Deserialize, PartialEq, Debug)]
pub struct FlangerConfig {
    pub depth: f32,
    pub rate: f32,
    pub feed: f32,
    pub wet: f32,
    pub bypassed: bool,
}

impl Default for FlangerConfig {
    fn default() -> Self {
        Self {
            depth: 0.6,
            rate: 0.5,
            feed: 0.3,
            wet: 0.35,
            bypassed: true,
        }
    }
}

#[derive(Clone, serde::Deserialize, PartialEq, Debug)]
pub struct DistortConfig {
    pub drive: f32,
    pub tone: f32,
    pub dist_type: f32,
    pub mix: f32,
    pub bypassed: bool,
}

impl Default for DistortConfig {
    fn default() -> Self {
        Self {
            drive: 0.4,
            tone: 0.5,
            dist_type: 0.0,
            mix: 0.3,
            bypassed: true,
        }
    }
}

/// Decoded PCM audio data — mapped to a temporary file via mmap.
/// Wrapped in Arc so the audio thread can read it without a lock after swap.
pub struct AudioData {
    pub mmap: Option<memmap2::Mmap>,
    pub samples_len: usize,
    pub sample_rate: u32,
    pub channels: u16,
}

impl Default for AudioData {
    fn default() -> Self {
        Self { mmap: None, samples_len: 0, sample_rate: 44100, channels: 2 }
    }
}

pub struct Peaks {
    vals: [AtomicU32; 4],
}

#[derive(Clone, Debug)]
pub enum MidiEvent {
    NoteOn(u8),
    NoteOff(u8),
    SetBypass(bool),
}

impl Peaks {
    pub fn new() -> Self {
        Self {
            vals: [
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
            ],
        }
    }

    /// Update peak if the new value is larger. Called only from audio thread.
    pub fn update(&self, i: usize, v: f32) {
        let old = f32::from_bits(self.vals[i].load(Ordering::Relaxed));
        if v > old {
            self.vals[i].store(v.to_bits(), Ordering::Relaxed);
        }
    }

    /// Read and atomically reset the peak. Called from UI thread.
    pub fn take(&self, i: usize) -> f32 {
        f32::from_bits(self.vals[i].swap(0, Ordering::Relaxed))
    }
}

#[derive(Clone)]
pub struct AppState {
    pub audio_data: Arc<Mutex<Arc<AudioData>>>,
    pub play_pos: Arc<AtomicU64>,
    pub seek_pos: Arc<AtomicU64>,
    pub is_playing: Arc<AtomicBool>,

    // Channels to audio thread
    pub eq_tx: Sender<Vec<BandConfig>>,
    pub compressor_tx: Sender<CompressorConfig>,
    pub reverb_tx: Sender<ReverbConfig>,
    pub delay_tx: Sender<DelayConfig>,
    pub chorus_tx: Sender<ChorusConfig>,
    pub flanger_tx: Sender<FlangerConfig>,
    pub distort_tx: Sender<DistortConfig>,
    pub midi_tx: Sender<MidiEvent>,
    
    // Compressor feedback
    pub compressor_gr: Arc<AtomicU32>,

    // Data feedback
    pub peaks: Arc<Peaks>,
    pub osc_consumer: Arc<Mutex<ringbuf::HeapConsumer<(f32, f32)>>>,

    // Last decode error — set by background decode thread, read and cleared by UI poll
    pub last_decode_error: Arc<Mutex<Option<String>>>,
}
