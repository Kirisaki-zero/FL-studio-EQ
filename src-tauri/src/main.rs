#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use claxon::FlacReader;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use biquad::*;
use crossbeam_channel::{unbounded, Sender, Receiver};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Oscilloscope ring buffer capacity (samples). ~93ms @ 44100 Hz.
const OSC_SIZE: usize = 4096;

/// Sentinel value meaning "no seek is pending".
const NO_SEEK: u64 = u64::MAX;

// ─────────────────────────────────────────────────────────────────────────────
// Shared Types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, serde::Deserialize, PartialEq)]
pub struct BandConfig {
    pub freq: f32,
    pub gain: f32,
    pub q: f32,
    pub shape: String,
    pub muted: bool,
}

/// Decoded PCM audio data — mapped to a temporary file via mmap.
/// Wrapped in Arc so the audio thread can read it without a lock after swap.
struct AudioData {
    mmap: Option<memmap2::Mmap>,
    samples_len: usize,
    sample_rate: u32,
    channels: u16,
}

impl Default for AudioData {
    fn default() -> Self {
        Self { mmap: None, samples_len: 0, sample_rate: 44100, channels: 2 }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Lock-Free Peak Meters (AtomicU32 storing f32 bits)
// ─────────────────────────────────────────────────────────────────────────────

struct Peaks {
    vals: [AtomicU32; 4],
}

impl Peaks {
    fn new() -> Self {
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
    fn update(&self, i: usize, v: f32) {
        let old = f32::from_bits(self.vals[i].load(Ordering::Relaxed));
        if v > old {
            self.vals[i].store(v.to_bits(), Ordering::Relaxed);
        }
    }

    /// Read and atomically reset the peak. Called from UI thread.
    fn take(&self, i: usize) -> f32 {
        f32::from_bits(self.vals[i].swap(0, Ordering::Relaxed))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EQ Filter Channel
// ─────────────────────────────────────────────────────────────────────────────

struct EqCh {
    filters: Vec<DirectForm2Transposed<f32>>,
    sr: f32,
}

impl EqCh {
    fn new(sr: f32) -> Self {
        Self { filters: vec![], sr }
    }

    fn update(&mut self, bands: &[BandConfig]) {
        let mut active_idx = 0;
        for b in bands {
            if b.muted || b.freq >= self.sr / 2.0 { continue; }
            let ft = match b.shape.as_str() {
                "Low Shelf"        => biquad::Type::LowShelf(b.gain),
                "High Shelf"       => biquad::Type::HighShelf(b.gain),
                "LP" | "Low Pass"  => biquad::Type::LowPass,
                "HP" | "High Pass" => biquad::Type::HighPass,
                "Notch"            => biquad::Type::Notch,
                "Band Pass"        => biquad::Type::BandPass,
                _                  => biquad::Type::PeakingEQ(b.gain),
            };
            
            // PERBAIKAN FATAL: Urutan yang benar adalah (Type, SampleRate, Frequency, Q)
            if let Ok(c) = biquad::Coefficients::<f32>::from_params(
                ft, self.sr.hz(), b.freq.hz(), b.q,
            ) {
                if active_idx < self.filters.len() {
                    // Update memori filter lama (Anti-Clicking / Zipper Noise)
                    self.filters[active_idx].update_coefficients(c);
                } else {
                    // Buat filter baru jika kurang
                    self.filters.push(DirectForm2Transposed::<f32>::new(c));
                }
                active_idx += 1;
            }
        }
        // Hapus sisa filter yang tidak terpakai
        self.filters.truncate(active_idx);
    }

    fn run(&mut self, s: f32) -> f32 {
        self.filters.iter_mut().fold(s, |acc, f| f.run(acc))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Application State
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    /// Shared audio data. Loader thread writes; audio thread reads briefly.
    audio_data: Arc<Mutex<Arc<AudioData>>>,

    /// Current playback position in source *frames* (written by audio thread,
    /// read by UI for display).
    play_pos: Arc<AtomicU64>,

    /// Seek target set by UI. Audio thread acknowledges by resetting to NO_SEEK.
    /// This gives < 5 ms seek latency (one audio callback period).
    seek_pos: Arc<AtomicU64>,

    /// Playback running state — toggled by UI, respected by audio thread.
    is_playing: Arc<AtomicBool>,

    /// Channel for sending updated EQ bands to the audio thread (lock-free).
    eq_tx: Sender<Vec<BandConfig>>,

    /// Lock-free peak meters (L, R, Mid, Side).
    peaks: Arc<Peaks>,

    /// Consumer side of the oscilloscope ring buffer. Only accessed by Tauri
    /// commands (not the audio callback).
    osc_consumer: Arc<Mutex<ringbuf::HeapConsumer<(f32, f32)>>>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Tauri Commands
// ─────────────────────────────────────────────────────────────────────────────

/// Load and play a FLAC file. Decoding happens in a background thread so the
/// UI never blocks. Silence is played during the decode phase.
#[tauri::command]
fn play_audio(path: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let audio_data = Arc::clone(&state.audio_data);
    let play_pos   = Arc::clone(&state.play_pos);
    let is_playing = Arc::clone(&state.is_playing);

    // Mute audio while loading to avoid stale samples
    is_playing.store(false, Ordering::Relaxed);

    std::thread::spawn(move || {
        let result = (|| -> Result<Arc<AudioData>, String> {
            use std::io::{Read, Seek, SeekFrom};
            let mut file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
            
            // Cek apakah ada header ID3 (sering ditambahkan secara paksa oleh beberapa tagger)
            let mut magic = [0u8; 4];
            if file.read_exact(&mut magic).is_ok() && &magic[0..3] == b"ID3" {
                // Lewati header ID3
                let mut header = [0u8; 6];
                if file.read_exact(&mut header).is_ok() {
                    let mut size = ((header[2] as u64) << 21)
                        | ((header[3] as u64) << 14)
                        | ((header[4] as u64) << 7)
                        | (header[5] as u64);
                    // Jika ada ID3 footer
                    if (header[1] & 0x10) != 0 { size += 10; }
                    // Seek melewati metadata ID3
                    file.seek(SeekFrom::Current(size as i64)).map_err(|e| e.to_string())?;
                }
            } else {
                // Kembalikan kursor ke awal jika bukan ID3
                file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
            }

            // Pencarian tangguh (Robust Sync): Cari marker "fLaC" 
            // untuk melewati padding kosong setelah ID3
            let mut sync_buf = [0u8; 4];
            let mut found = false;
            if file.read_exact(&mut sync_buf).is_ok() {
                if &sync_buf == b"fLaC" {
                    found = true;
                    file.seek(SeekFrom::Current(-4)).unwrap();
                } else {
                    let mut search_limit = 1024 * 1024; // maksimal cari 1MB ke depan
                    let mut ring = sync_buf;
                    let mut byte = [0u8; 1];
                    while search_limit > 0 && file.read_exact(&mut byte).is_ok() {
                        ring[0] = ring[1];
                        ring[1] = ring[2];
                        ring[2] = ring[3];
                        ring[3] = byte[0];
                        if &ring == b"fLaC" {
                            found = true;
                            file.seek(SeekFrom::Current(-4)).unwrap();
                            break;
                        }
                        search_limit -= 1;
                    }
                }
            }

            if !found {
                return Err("Bukan file FLAC yang valid (tanda 'fLaC' tidak ditemukan)".into());
            }

            let mut reader = FlacReader::new(file).map_err(|e| format!("FLAC parse error: {}", e))?;
            let info    = reader.streaminfo();
            let max_val = (1u64 << (info.bits_per_sample - 1)) as f32;

            // Buat tempfile untuk menampung data PCM mentah f32 (menghindari OOM)
            use std::io::{Write, BufWriter};
            let temp_file = tempfile::tempfile().map_err(|e| e.to_string())?;
            let mut writer = BufWriter::with_capacity(1024 * 1024, temp_file);
            let mut samples_len = 0;

            // Stream dan tulis sampel langsung ke disk virtual (menggunakan chunking & bufwriter)
            let mut chunk = Vec::with_capacity(8192);
            for sample in reader.samples().filter_map(|s| s.ok()) {
                chunk.push(sample as f32 / max_val);
                if chunk.len() >= 8192 {
                    writer.write_all(bytemuck::cast_slice(&chunk)).map_err(|e| e.to_string())?;
                    samples_len += chunk.len();
                    chunk.clear();
                }
            }
            if !chunk.is_empty() {
                writer.write_all(bytemuck::cast_slice(&chunk)).map_err(|e| e.to_string())?;
                samples_len += chunk.len();
            }

            // Flush dan kembalikan ke file asli
            writer.flush().map_err(|e| e.to_string())?;
            let temp_file = writer.into_inner().map_err(|e| e.to_string())?;

            // Mmap file temp ke memori (OS akan mengurus paging jika RAM penuh)
            let mmap = unsafe { memmap2::MmapOptions::new().map(&temp_file) }.map_err(|e| e.to_string())?;

            Ok(Arc::new(AudioData {
                mmap: Some(mmap),
                samples_len,
                sample_rate: info.sample_rate,
                channels:    info.channels as u16,
            }))
        })();

        match result {
            Ok(data) => {
                *audio_data.lock().unwrap() = data;
                play_pos.store(0, Ordering::Relaxed);
                is_playing.store(true, Ordering::Relaxed);
                println!("Loaded: {}", path);
            }
            Err(e) => eprintln!("Load error: {e}"),
        }
    });

    Ok(())
}

/// Toggle play/pause.
#[tauri::command]
fn pause_audio(state: tauri::State<'_, AppState>) {
    let prev = state.is_playing.load(Ordering::Relaxed);
    state.is_playing.store(!prev, Ordering::Relaxed);
}

/// Seek to a position (in source frames). Takes effect within one audio
/// callback period (typically < 5 ms) — ultra-low latency scrubbing.
#[tauri::command]
fn seek_audio(frame: u64, state: tauri::State<'_, AppState>) {
    state.seek_pos.store(frame, Ordering::Relaxed);
}

/// Push new EQ band configuration to the audio thread (lock-free via channel).
#[tauri::command]
fn update_eq_bands(bands: Vec<BandConfig>, state: tauri::State<'_, AppState>) {
    let _ = state.eq_tx.send(bands);
}

/// Read and reset peak meters (L, R, Mid, Side) in dBFS.
#[tauri::command]
fn get_meter_levels(state: tauri::State<'_, AppState>) -> [f32; 4] {
    [0, 1, 2, 3].map(|i| {
        let v = state.peaks.take(i);
        if v > 1e-5 { 20.0 * v.log10() } else { -100.0 }
    })
}

/// Return (is_playing, current_frame, total_frames).
#[tauri::command]
fn get_playback_info(state: tauri::State<'_, AppState>) -> (bool, u64, u64) {
    let playing = state.is_playing.load(Ordering::Relaxed);
    let pos     = state.play_pos.load(Ordering::Relaxed);
    let total   = {
        let d  = state.audio_data.lock().unwrap();
        let ch = d.channels.max(1) as u64;
        d.samples_len as u64 / ch
    };
    (playing, pos, total)
}

/// Drain the oscilloscope ring buffer and return (left_channel, right_channel).
#[tauri::command]
fn get_oscilloscope_data(state: tauri::State<'_, AppState>) -> (Vec<f32>, Vec<f32>) {
    let Ok(mut cons) = state.osc_consumer.try_lock() else {
        return (vec![], vec![]);
    };
    let n = cons.len();
    let (mut left, mut right) = (Vec::with_capacity(n), Vec::with_capacity(n));
    while let Some((l, r)) = cons.pop() {
        left.push(l);
        right.push(r);
    }
    (left, right)
}

// ─────────────────────────────────────────────────────────────────────────────
// Audio Thread (Lock-Free Render Callback)
// ─────────────────────────────────────────────────────────────────────────────

fn audio_thread(
    audio_data:   Arc<Mutex<Arc<AudioData>>>,
    play_pos:     Arc<AtomicU64>,
    seek_pos:     Arc<AtomicU64>,
    is_playing:   Arc<AtomicBool>,
    eq_rx:        Receiver<Vec<BandConfig>>,
    peaks:        Arc<Peaks>,
    mut osc_prod: ringbuf::HeapProducer<(f32, f32)>,
) {
    let host     = cpal::default_host();
    let device   = host.default_output_device().expect("no output device");
    let sc       = device.default_output_config().unwrap();
    let out_sr   = sc.sample_rate().0 as f32;
    let out_ch   = sc.channels() as usize;
    let config: cpal::StreamConfig = sc.into();

    let mut eq_l       = EqCh::new(out_sr);
    let mut eq_r       = EqCh::new(out_sr);
    let mut cur_bands  = Vec::<BandConfig>::new();

    // Local snapshot of AudioData Arc — refreshed cheaply without blocking.
    let mut local_data: Arc<AudioData> = Arc::new(AudioData::default());

    // Local high-precision playback position (f64) owned by audio thread.
    // This avoids atomic float arithmetic per sample.
    let mut local_pos: f64 = 0.0;

    let stream = device.build_output_stream(
        &config,
        move |data: &mut [f32], _| {
            // ── Step 1: Receive EQ updates (non-blocking) ─────────────────
            while let Ok(bands) = eq_rx.try_recv() {
                if bands != cur_bands {
                    cur_bands = bands;
                    eq_l.update(&cur_bands);
                    eq_r.update(&cur_bands);
                }
            }

            // ── Step 2: Snapshot audio data Arc (brief lock, just ptr swap) ─
            if let Ok(guard) = audio_data.try_lock() {
                if !Arc::ptr_eq(&*guard, &local_data) {
                    local_data = Arc::clone(&*guard);
                    local_pos  = 0.0;
                }
            }

            // ── Step 3: Handle seek (ultra-low latency) ───────────────────
            // The seek_pos atomic is written by the UI thread and read here.
            // Taking effect within one callback period ≈ 2–5 ms.
            let seek = seek_pos.load(Ordering::Relaxed);
            if seek != NO_SEEK {
                local_pos = seek as f64;
                seek_pos.store(NO_SEEK, Ordering::Relaxed); // acknowledge
            }

            // ── Step 4: Output silence if not playing ─────────────────────
            if !is_playing.load(Ordering::Relaxed) || local_data.samples_len == 0 {
                for s in data.iter_mut() { *s = 0.0; }
                return;
            }

            let src_ch    = local_data.channels.max(1) as usize;
            let src_sr    = local_data.sample_rate as f64;
            let ratio     = src_sr / out_sr as f64;
            let src_total = local_data.samples_len;
            
            // Cast MMAP bytes ke slice f32 secara aman
            let mmap_ref = local_data.mmap.as_ref();
            let samples: &[f32] = match mmap_ref {
                Some(m) => bytemuck::cast_slice(m),
                None => &[],
            };

            let get_s = |fi: usize, ch: usize| -> f32 {
                samples.get(fi * src_ch + ch).copied().unwrap_or(0.0)
            };

            // ── Step 5: Render audio frames ───────────────────────────────
            for frame in data.chunks_mut(out_ch) {
                let pi   = local_pos as usize;
                let frac = (local_pos - pi as f64) as f32;

                // End of audio
                if pi * src_ch >= src_total {
                    is_playing.store(false, Ordering::Relaxed);
                    for s in frame.iter_mut() { *s = 0.0; }
                    continue;
                }

                // Linear interpolation (L channel)
                let lin = {
                    let s0 = get_s(pi, 0);
                    let s1 = get_s(pi + 1, 0);
                    s0 + (s1 - s0) * frac
                };

                // Linear interpolation (R channel, or duplicate L if mono)
                let rin = if src_ch > 1 {
                    let s0 = get_s(pi, 1);
                    let s1 = get_s(pi + 1, 1);
                    s0 + (s1 - s0) * frac
                } else {
                    lin
                };

                // Apply EQ
                let lout = eq_l.run(lin);
                let rout = eq_r.run(rin);
                let mid  = (lout + rout) * 0.70710678;
                let side = (lout - rout) * 0.70710678;

                // Update meters
                peaks.update(0, lout.abs());
                peaks.update(1, rout.abs());
                peaks.update(2, mid.abs());
                peaks.update(3, side.abs());

                // Write to output
                if out_ch >= 1 { frame[0] = lout; }
                if out_ch >= 2 { frame[1] = rout; }

                // Push to oscilloscope ring buffer (lock-free SPSC)
                osc_prod.push((lout, rout)).ok();

                // Advance position
                local_pos += ratio;
            }

            // Publish position for UI display (once per callback, not per sample)
            play_pos.store(local_pos as u64, Ordering::Relaxed);
        },
        |err| eprintln!("Audio stream error: {err}"),
        None,
    ).expect("Failed to build audio stream");

    stream.play().expect("Failed to start audio stream");

    // Keep thread alive
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry Point
// ─────────────────────────────────────────────────────────────────────────────

fn main() {
    let initial_bands = vec![
        BandConfig { freq: 20.0,    gain: 0.0,  q: 0.7, shape: "HP".into(),         muted: false },
        BandConfig { freq: 80.0,    gain: -3.0, q: 1.2, shape: "Low Shelf".into(),  muted: false },
        BandConfig { freq: 250.0,   gain: 2.0,  q: 1.8, shape: "Peaking".into(),    muted: false },
        BandConfig { freq: 1000.0,  gain: 3.0,  q: 1.4, shape: "Peaking".into(),    muted: false },
        BandConfig { freq: 4000.0,  gain: -2.0, q: 2.0, shape: "Peaking".into(),    muted: false },
        BandConfig { freq: 10000.0, gain: 4.0,  q: 1.6, shape: "High Shelf".into(), muted: false },
        BandConfig { freq: 20000.0, gain: 0.0,  q: 0.7, shape: "LP".into(),         muted: false },
    ];

    // Lock-free communication channels
    let (eq_tx, eq_rx) = unbounded::<Vec<BandConfig>>();

    // Oscilloscope SPSC ring buffer
    let rb             = ringbuf::HeapRb::<(f32, f32)>::new(OSC_SIZE);
    let (osc_prod, osc_cons) = rb.split();

    // Shared atomic state
    let peaks      = Arc::new(Peaks::new());
    let audio_data = Arc::new(Mutex::new(Arc::new(AudioData::default())));
    let play_pos   = Arc::new(AtomicU64::new(0));
    let seek_pos   = Arc::new(AtomicU64::new(NO_SEEK));
    let is_playing = Arc::new(AtomicBool::new(false));

    let state = AppState {
        audio_data:   Arc::clone(&audio_data),
        play_pos:     Arc::clone(&play_pos),
        seek_pos:     Arc::clone(&seek_pos),
        is_playing:   Arc::clone(&is_playing),
        eq_tx:        eq_tx.clone(),
        peaks:        Arc::clone(&peaks),
        osc_consumer: Arc::new(Mutex::new(osc_cons)),
    };

    // Send initial EQ configuration to audio thread
    let _ = eq_tx.send(initial_bands);

    // Spawn lock-free audio thread
    std::thread::spawn(move || {
        audio_thread(
            audio_data,
            play_pos,
            seek_pos,
            is_playing,
            eq_rx,
            peaks,
            osc_prod,
        );
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            play_audio,
            pause_audio,
            seek_audio,
            update_eq_bands,
            get_meter_levels,
            get_playback_info,
            get_oscilloscope_data,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}