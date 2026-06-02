#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use claxon::FlacReader;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};
use std::path::PathBuf;
use biquad::*;
use tauri::Manager;

// 7-Band EQ Frequencies
const EQ_FREQS: [f32; 7] = [60.0, 230.0, 600.0, 1500.0, 3000.0, 8000.0, 14000.0];

#[derive(Clone)]
struct AppState {
    audio_state: Arc<Mutex<AudioState>>,
}

#[derive(Clone, serde::Deserialize, PartialEq)]
pub struct BandConfig {
    pub freq: f32,
    pub gain: f32,
    pub q: f32,
    pub shape: String,
    pub muted: bool,
}

struct AudioState {
    is_playing: bool,
    samples: Vec<f32>,
    position: f64, // float position for sub-sample interpolation
    sample_rate: u32,
    channels: u16,
    bands: Vec<BandConfig>, // 7 bands
    meter_peaks: [f32; 4], // L, R, M, S maximum amplitude
    osc_buffer: Vec<(f32, f32)>, // Circular buffer for Oscilloscope (L, R)
    osc_index: usize,
}

// Biquad filter chain for one channel
struct EqChannel {
    filters: Vec<DirectForm2Transposed<f32>>,
    sample_rate: f32,
}

impl EqChannel {
    fn new(sample_rate: f32) -> Self {
        Self {
            filters: vec![],
            sample_rate,
        }
    }

    fn update_filters(&mut self, bands: &[BandConfig]) {
        self.filters.clear();

        for band in bands.iter() {
            if band.muted { continue; } // Skip muted bands
            
            let f = band.freq.hz();
            let fs = self.sample_rate.hz();
            
            // Freq should be < Nyquist
            if band.freq >= self.sample_rate / 2.0 { continue; }

            let filter_type = match band.shape.as_str() {
                "Low Shelf" => biquad::Type::LowShelf(band.gain),
                "High Shelf" => biquad::Type::HighShelf(band.gain),
                "Peaking" => biquad::Type::PeakingEQ(band.gain),
                "LP" | "Low Pass" => biquad::Type::LowPass,
                "HP" | "High Pass" => biquad::Type::HighPass,
                "Notch" => biquad::Type::Notch,
                "Band Pass" => biquad::Type::BandPass,
                _ => biquad::Type::PeakingEQ(band.gain), // Fallback
            };

            if let Ok(coeffs) = biquad::Coefficients::<f32>::from_params(filter_type, fs, f, band.q) {
                self.filters.push(DirectForm2Transposed::<f32>::new(coeffs));
            }
        }
    }

    fn process(&mut self, sample: f32) -> f32 {
        let mut s = sample;
        for filter in &mut self.filters {
            s = filter.run(s);
        }
        s
    }
}

#[tauri::command]
fn play_audio(path: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut reader = FlacReader::open(&path).map_err(|e| e.to_string())?;
    let info = reader.streaminfo();
    
    let mut samples = Vec::new();
    let max_val = (1u64 << (info.bits_per_sample - 1)) as f32;
    
    // Decode FLAC to f32 PCM
    for sample in reader.samples() {
        let s = sample.unwrap_or(0);
        samples.push(s as f32 / max_val);
    }
    
    let mut st = state.audio_state.lock().unwrap();
    st.samples = samples;
    st.sample_rate = info.sample_rate;
    st.channels = info.channels as u16;
    st.position = 0.0;
    st.is_playing = true;
    
    println!("Loaded audio: {} ({} Hz, {} ch)", path, info.sample_rate, info.channels);
    Ok(())
}

#[tauri::command]
fn update_eq_bands(bands: Vec<BandConfig>, state: tauri::State<'_, AppState>) {
    let mut st = state.audio_state.lock().unwrap();
    st.bands = bands;
}

#[tauri::command]
fn get_meter_levels(state: tauri::State<'_, AppState>) -> [f32; 4] {
    let mut st = state.audio_state.lock().unwrap();
    let peaks = st.meter_peaks;
    // Reset peaks for next poll window
    st.meter_peaks = [0.0; 4];
    
    let to_db = |val: f32| if val > 1e-5 { 20.0 * val.log10() } else { -100.0 };
    [to_db(peaks[0]), to_db(peaks[1]), to_db(peaks[2]), to_db(peaks[3])]
}

#[tauri::command]
fn get_oscilloscope_data(state: tauri::State<'_, AppState>) -> (Vec<f32>, Vec<f32>) {
    let st = state.audio_state.lock().unwrap();
    let capacity = st.osc_buffer.len();
    if capacity == 0 {
        return (vec![], vec![]);
    }
    
    let mut left = Vec::with_capacity(capacity);
    let mut right = Vec::with_capacity(capacity);
    
    // Read from oldest to newest
    for i in 0..capacity {
        let idx = (st.osc_index + i) % capacity;
        let (l, r) = st.osc_buffer[idx];
        left.push(l);
        right.push(r);
    }
    
    (left, right)
}

fn audio_thread(audio_state: Arc<Mutex<AudioState>>) {
    let host = cpal::default_host();
    let device = host.default_output_device().expect("no output device available");
    let mut config = device.default_output_config().unwrap().config();
    
    // Wait until audio is loaded to match sample rate if possible, 
    // but for simplicity we will just use the device default and resample later if needed.
    // For now, let's assume device default is fine.
    
    let mut eq_left = EqChannel::new(config.sample_rate.0 as f32);
    let mut eq_right = EqChannel::new(config.sample_rate.0 as f32);
    let mut current_bands: Vec<BandConfig> = vec![];

    let stream = device.build_output_stream(
        &config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            let mut st = audio_state.lock().unwrap();
            
            // Update EQ if bands changed
            if st.bands != current_bands {
                current_bands = st.bands.clone();
                eq_left.update_filters(&current_bands);
                eq_right.update_filters(&current_bands);
            }

            if !st.is_playing || st.samples.is_empty() {
                for sample in data.iter_mut() {
                    *sample = 0.0;
                }
                return;
            }

            let channels = config.channels as usize;
            let src_channels = st.channels as usize;
            let output_sr = config.sample_rate.0 as f64;
            let input_sr = st.sample_rate as f64;
            let speed_ratio = input_sr / output_sr;
            
            let mut local_peaks = [0.0f32; 4];

            for frame in data.chunks_mut(channels) {
                let pos_int = st.position.floor() as usize;
                let pos_frac = (st.position - pos_int as f64) as f32;

                // Check if we've reached the end
                if pos_int * src_channels >= st.samples.len() {
                    st.is_playing = false;
                    for sample in frame.iter_mut() {
                        *sample = 0.0;
                    }
                    continue;
                }

                // Helper to safely get a sample
                let get_sample = |frame_idx: usize, ch: usize| -> f32 {
                    let idx = frame_idx * src_channels + ch;
                    if idx < st.samples.len() {
                        st.samples[idx]
                    } else {
                        0.0
                    }
                };

                // Linear interpolation for Left channel
                let left_0 = get_sample(pos_int, 0);
                let left_1 = get_sample(pos_int + 1, 0);
                let left_in = left_0 + (left_1 - left_0) * pos_frac;

                // Linear interpolation for Right channel (or duplicate Left if mono)
                let right_in = if src_channels > 1 {
                    let right_0 = get_sample(pos_int, 1);
                    let right_1 = get_sample(pos_int + 1, 1);
                    right_0 + (right_1 - right_0) * pos_frac
                } else {
                    left_in
                };

                let left_out = eq_left.process(left_in);
                let right_out = eq_right.process(right_in);
                
                // Calculate Mid and Side
                let mid_out = (left_out + right_out) * 0.70710678;
                let side_out = (left_out - right_out) * 0.70710678;

                // Update local peaks
                local_peaks[0] = local_peaks[0].max(left_out.abs());
                local_peaks[1] = local_peaks[1].max(right_out.abs());
                local_peaks[2] = local_peaks[2].max(mid_out.abs());
                local_peaks[3] = local_peaks[3].max(side_out.abs());

                if channels >= 1 {
                    frame[0] = left_out;
                }
                if channels >= 2 {
                    frame[1] = right_out;
                }

                // Push to oscilloscope buffer
                let osc_cap = st.osc_buffer.len();
                if osc_cap > 0 {
                    let osc_idx = st.osc_index;
                    st.osc_buffer[osc_idx] = (left_out, right_out);
                    st.osc_index = (osc_idx + 1) % osc_cap;
                }

                st.position += speed_ratio;
            }
            
            // Accumulate peaks in state
            for i in 0..4 {
                st.meter_peaks[i] = st.meter_peaks[i].max(local_peaks[i]);
            }
        },
        |err| eprintln!("Audio error: {}", err),
        None
    ).unwrap();

    stream.play().unwrap();

    // keep thread alive
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn main() {
    let initial_bands = vec![
        BandConfig { freq: 20.0, gain: 0.0, q: 0.7, shape: "HP".into(), muted: false },
        BandConfig { freq: 80.0, gain: -3.0, q: 1.2, shape: "Low Shelf".into(), muted: false },
        BandConfig { freq: 250.0, gain: 2.0, q: 1.8, shape: "Peaking".into(), muted: false },
        BandConfig { freq: 1000.0, gain: 3.0, q: 1.4, shape: "Peaking".into(), muted: false },
        BandConfig { freq: 4000.0, gain: -2.0, q: 2.0, shape: "Peaking".into(), muted: false },
        BandConfig { freq: 10000.0, gain: 4.0, q: 1.6, shape: "High Shelf".into(), muted: false },
        BandConfig { freq: 20000.0, gain: 0.0, q: 0.7, shape: "LP".into(), muted: false },
    ];

    let state = AppState {
        audio_state: Arc::new(Mutex::new(AudioState {
            is_playing: false,
            samples: vec![],
            position: 0.0,
            sample_rate: 44100,
            channels: 2,
            bands: initial_bands,
            meter_peaks: [0.0; 4],
            osc_buffer: vec![(0.0, 0.0); 4096],
            osc_index: 0,
        })),
    };

    // Note: If you have UI related stuff inside `main`, keep it below
    // Start Audio thread
    let audio_state_clone = Arc::clone(&state.audio_state);
    std::thread::spawn(move || {
        audio_thread(audio_state_clone);
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![play_audio, update_eq_bands, get_meter_levels, get_oscilloscope_data])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}