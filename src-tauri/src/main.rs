#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64};
use crossbeam_channel::unbounded;

use audio::state::*;
use audio::engine::audio_thread;
use audio::commands::*;

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
    let (comp_tx, comp_rx) = unbounded::<CompressorConfig>();
    let (reverb_tx, reverb_rx) = unbounded::<ReverbConfig>();
    let (delay_tx, delay_rx) = unbounded::<DelayConfig>();
    let (chorus_tx, chorus_rx) = unbounded::<ChorusConfig>();
    let (flanger_tx, flanger_rx) = unbounded::<FlangerConfig>();
    let (distort_tx, distort_rx) = unbounded::<DistortConfig>();
    let (midi_tx, midi_rx) = unbounded::<MidiEvent>();

    // Oscilloscope SPSC ring buffer
    let rb             = ringbuf::HeapRb::<(f32, f32)>::new(OSC_SIZE);
    let (osc_prod, osc_cons) = rb.split();

    // Shared atomic state
    let peaks      = Arc::new(Peaks::new());
    let audio_data = Arc::new(Mutex::new(Arc::new(AudioData::default())));
    let play_pos   = Arc::new(AtomicU64::new(0));
    let seek_pos   = Arc::new(AtomicU64::new(NO_SEEK));
    let is_playing = Arc::new(AtomicBool::new(false));
    let comp_gr    = Arc::new(std::sync::atomic::AtomicU32::new(0));

    let last_decode_error = Arc::new(Mutex::new(None::<String>));

    let state = AppState {
        audio_data:   Arc::clone(&audio_data),
        play_pos:     Arc::clone(&play_pos),
        seek_pos:     Arc::clone(&seek_pos),
        is_playing:   Arc::clone(&is_playing),
        eq_tx:        eq_tx.clone(),
        compressor_tx: comp_tx.clone(),
        reverb_tx:    reverb_tx.clone(),
        delay_tx:     delay_tx.clone(),
        chorus_tx:    chorus_tx.clone(),
        flanger_tx:   flanger_tx.clone(),
        distort_tx:   distort_tx.clone(),
        midi_tx:      midi_tx.clone(),
        compressor_gr: Arc::clone(&comp_gr),
        peaks:        Arc::clone(&peaks),
        osc_consumer: Arc::new(Mutex::new(osc_cons)),
        last_decode_error: Arc::clone(&last_decode_error),
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
            comp_rx,
            reverb_rx,
            delay_rx,
            chorus_rx,
            flanger_rx,
            distort_rx,
            midi_rx,
            comp_gr,
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
            update_compressor,
            update_reverb,
            update_delay,
            update_chorus,
            update_flanger,
            update_distort,
            get_compressor_meter,
            get_meter_levels,
            get_playback_info,
            get_oscilloscope_data,
            get_decode_status,
            play_midi_note,
            update_midi_bypass,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}