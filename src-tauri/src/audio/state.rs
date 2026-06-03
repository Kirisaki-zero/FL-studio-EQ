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
    pub eq_tx: Sender<Vec<BandConfig>>,
    pub peaks: Arc<Peaks>,
    pub osc_consumer: Arc<Mutex<ringbuf::HeapConsumer<(f32, f32)>>>,
}
