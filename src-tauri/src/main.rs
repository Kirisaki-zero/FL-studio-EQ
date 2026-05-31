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

struct AudioState {
    is_playing: bool,
    samples: Vec<f32>,
    position: usize,
    sample_rate: u32,
    channels: u16,
    gains: [f32; 7], // gains in dB for each band
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

    fn update_filters(&mut self, gains: &[f32; 7]) {
        self.filters.clear();
        let q = 1.414; // Q-factor

        for (i, &freq) in EQ_FREQS.iter().enumerate() {
            let gain = gains[i];
            let f = freq.hz();
            let fs = self.sample_rate.hz();
            
            let filter_type = if i == 0 {
                // Low shelf
                biquad::Type::LowShelf(gain)
            } else if i == EQ_FREQS.len() - 1 {
                // High shelf
                biquad::Type::HighShelf(gain)
            } else {
                // Peaking EQ
                biquad::Type::PeakingEQ(gain)
            };

            if let Ok(coeffs) = biquad::Coefficients::<f32>::from_params(filter_type, fs, f, q) {
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
    st.position = 0;
    st.is_playing = true;
    
    println!("Loaded audio: {} ({} Hz, {} ch)", path, info.sample_rate, info.channels);
    Ok(())
}

#[tauri::command]
fn set_eq_gain(band: usize, gain: f32, state: tauri::State<'_, AppState>) {
    if band < 7 {
        let mut st = state.audio_state.lock().unwrap();
        st.gains[band] = gain;
    }
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
    let mut current_gains = [0.0; 7];

    let stream = device.build_output_stream(
        &config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            let mut st = audio_state.lock().unwrap();
            
            // Update EQ if gains changed
            if st.gains != current_gains {
                current_gains = st.gains;
                eq_left.update_filters(&current_gains);
                eq_right.update_filters(&current_gains);
            }

            if !st.is_playing || st.samples.is_empty() {
                for sample in data.iter_mut() {
                    *sample = 0.0;
                }
                return;
            }

            let channels = config.channels as usize;
            let mut pos = st.position;
            let src_channels = st.channels as usize;

            for frame in data.chunks_mut(channels) {
                if pos >= st.samples.len() {
                    st.is_playing = false;
                    for sample in frame.iter_mut() {
                        *sample = 0.0;
                    }
                    continue;
                }

                // Simple channel mapping (assuming stereo or mono source)
                let left_in = st.samples[pos];
                let right_in = if src_channels > 1 && pos + 1 < st.samples.len() {
                    st.samples[pos + 1]
                } else {
                    left_in
                };

                let left_out = eq_left.process(left_in);
                let right_out = eq_right.process(right_in);

                if channels >= 1 {
                    frame[0] = left_out;
                }
                if channels >= 2 {
                    frame[1] = right_out;
                }

                pos += src_channels;
            }
            st.position = pos;
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
    let state = AppState {
        audio_state: Arc::new(Mutex::new(AudioState {
            is_playing: false,
            samples: vec![],
            position: 0,
            sample_rate: 44100,
            channels: 2,
            gains: [0.0; 7],
        })),
    };

    let audio_state_clone = state.audio_state.clone();
    std::thread::spawn(move || {
        audio_thread(audio_state_clone);
    });

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![play_audio, set_eq_gain])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}