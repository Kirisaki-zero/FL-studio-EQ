#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let initial_bands = vec![
        crate::audio::state::BandConfig { freq: 20.0,    gain: 0.0,  q: 0.7, shape: "HP".into(),         muted: false },
        crate::audio::state::BandConfig { freq: 80.0,    gain: -3.0, q: 1.2, shape: "Low Shelf".into(),  muted: false },
        crate::audio::state::BandConfig { freq: 250.0,   gain: 2.0,  q: 1.8, shape: "Peaking".into(),    muted: false },
        crate::audio::state::BandConfig { freq: 1000.0,  gain: 3.0,  q: 1.4, shape: "Peaking".into(),    muted: false },
        crate::audio::state::BandConfig { freq: 4000.0,  gain: -2.0, q: 2.0, shape: "Peaking".into(),    muted: false },
        crate::audio::state::BandConfig { freq: 10000.0, gain: 4.0,  q: 1.6, shape: "High Shelf".into(), muted: false },
        crate::audio::state::BandConfig { freq: 20000.0, gain: 0.0,  q: 0.7, shape: "LP".into(),         muted: false },
    ];

    // Lock-free communication channels
    let (eq_tx, eq_rx) = crossbeam_channel::unbounded::<Vec<crate::audio::state::BandConfig>>();
    let (comp_tx, comp_rx) = crossbeam_channel::unbounded::<crate::audio::state::CompressorConfig>();
    let (reverb_tx, reverb_rx) = crossbeam_channel::unbounded::<crate::audio::state::ReverbConfig>();
    let (delay_tx, delay_rx) = crossbeam_channel::unbounded::<crate::audio::state::DelayConfig>();
    let (chorus_tx, chorus_rx) = crossbeam_channel::unbounded::<crate::audio::state::ChorusConfig>();
    let (flanger_tx, flanger_rx) = crossbeam_channel::unbounded::<crate::audio::state::FlangerConfig>();
    let (distort_tx, distort_rx) = crossbeam_channel::unbounded::<crate::audio::state::DistortConfig>();
    let (midi_tx, midi_rx) = crossbeam_channel::unbounded::<crate::audio::state::MidiEvent>();

    // Oscilloscope SPSC ring buffer
    let rb             = ringbuf::HeapRb::<(f32, f32)>::new(crate::audio::state::OSC_SIZE);
    let (osc_prod, osc_cons) = rb.split();

    // Shared atomic state
    let peaks      = std::sync::Arc::new(crate::audio::state::Peaks::new());
    let audio_data = std::sync::Arc::new(std::sync::Mutex::new(std::sync::Arc::new(crate::audio::state::AudioData::default())));
    let play_pos   = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let seek_pos   = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(crate::audio::state::NO_SEEK));
    let is_playing = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let comp_gr    = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));

    let last_decode_error = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));

    let state = crate::audio::state::AppState {
        audio_data:   std::sync::Arc::clone(&audio_data),
        play_pos:     std::sync::Arc::clone(&play_pos),
        seek_pos:     std::sync::Arc::clone(&seek_pos),
        is_playing:   std::sync::Arc::clone(&is_playing),
        eq_tx:        eq_tx.clone(),
        compressor_tx: comp_tx.clone(),
        reverb_tx:    reverb_tx.clone(),
        delay_tx:     delay_tx.clone(),
        chorus_tx:    chorus_tx.clone(),
        flanger_tx:   flanger_tx.clone(),
        distort_tx:   distort_tx.clone(),
        midi_tx:      midi_tx.clone(),
        compressor_gr: std::sync::Arc::clone(&comp_gr),
        peaks:        std::sync::Arc::clone(&peaks),
        osc_consumer: std::sync::Arc::new(std::sync::Mutex::new(osc_cons)),
        last_decode_error: std::sync::Arc::clone(&last_decode_error),
    };

    // Send initial EQ configuration to audio thread
    let _ = eq_tx.send(initial_bands);

    // Spawn lock-free audio thread
    std::thread::spawn(move || {
        crate::audio::engine::audio_thread(
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
            crate::audio::commands::play_audio,
            crate::audio::commands::pause_audio,
            crate::audio::commands::seek_audio,
            crate::audio::commands::update_eq_bands,
            crate::audio::commands::update_compressor,
            crate::audio::commands::update_reverb,
            crate::audio::commands::update_delay,
            crate::audio::commands::update_chorus,
            crate::audio::commands::update_flanger,
            crate::audio::commands::update_distort,
            crate::audio::commands::get_compressor_meter,
            crate::audio::commands::get_meter_levels,
            crate::audio::commands::get_playback_info,
            crate::audio::commands::get_oscilloscope_data,
            crate::audio::commands::get_decode_status,
            crate::audio::commands::read_android_dir,
            crate::audio::commands::play_midi_note,
            crate::audio::commands::update_midi_bypass,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

pub mod audio;
