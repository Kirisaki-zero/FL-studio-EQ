use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::state::{AppState, AudioData, BandConfig, CompressorConfig, ReverbConfig, DelayConfig, ChorusConfig, FlangerConfig, DistortConfig, MidiEvent};

#[tauri::command]
pub fn play_audio(path: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let audio_data         = Arc::clone(&state.audio_data);
    let play_pos           = Arc::clone(&state.play_pos);
    let is_playing         = Arc::clone(&state.is_playing);
    let last_decode_error  = Arc::clone(&state.last_decode_error);

    is_playing.store(false, Ordering::Relaxed);
    // Clear any previous error so UI poller sees fresh state
    *last_decode_error.lock().unwrap() = None;

    std::thread::spawn(move || {
        let result = (|| -> Result<Arc<AudioData>, String> {
            use std::io::{Write, BufWriter};

            let file = std::fs::File::open(&path).map_err(|e| format!("Gagal buka file: {}", e))?;
            let mss = symphonia::core::io::MediaSourceStream::new(Box::new(file), Default::default());

            let mut hint = symphonia::core::probe::Hint::new();
            if let Some(ext) = path.rsplit('.').next() {
                hint.with_extension(ext);
            }

            let probed = symphonia::default::get_probe()
                .format(
                    &hint, mss,
                    &symphonia::core::formats::FormatOptions::default(),
                    &symphonia::core::meta::MetadataOptions::default(),
                )
                .map_err(|e| format!("Format tidak dikenali: {}", e))?;

            let mut format = probed.format;
            let track = format.tracks().iter()
                .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
                .ok_or_else(|| "Tidak ada audio track di file ini".to_string())?;

            let mut decoder = symphonia::default::get_codecs()
                .make(&track.codec_params, &symphonia::core::codecs::DecoderOptions::default())
                .map_err(|e| format!("Codec tidak didukung: {}", e))?;

            let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
            let channels    = track.codec_params.channels.map(|c| c.count()).unwrap_or(2) as u16;

            let temp_file = tempfile::tempfile().map_err(|e| e.to_string())?;
            let mut writer = BufWriter::with_capacity(1024 * 1024, temp_file);
            let mut samples_len = 0usize;

            loop {
                let packet = match format.next_packet() {
                    Ok(p) => p,
                    // End of stream — normal termination
                    Err(symphonia::core::errors::Error::IoError(ref e))
                        if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                    Err(symphonia::core::errors::Error::ResetRequired) => {
                        decoder.reset();
                        continue;
                    },
                    Err(_) => break,
                };

                match decoder.decode(&packet) {
                    Ok(decoded) => {
                        // ✅ FIX: Buat SampleBuffer baru per-paket dengan kapasitas TEPAT
                        // dari decoded.frames() — bukan decoded.capacity().
                        // Ini mencegah assert!-panic ketika FLAC punya variable block size.
                        let spec     = *decoded.spec();
                        let n_frames = decoded.frames() as u64;
                        if n_frames == 0 { continue; }

                        let mut sample_buf =
                            symphonia::core::audio::SampleBuffer::<f32>::new(n_frames, spec);
                        sample_buf.copy_interleaved_ref(decoded);

                        let samples = sample_buf.samples();
                        writer.write_all(bytemuck::cast_slice(samples))
                            .map_err(|e| e.to_string())?;
                        samples_len += samples.len();
                    },
                    Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                    Err(_) => break,
                }
            }

            if samples_len == 0 {
                return Err("File berhasil dibuka tapi tidak ada sampel audio yang bisa dibaca".to_string());
            }

            writer.flush().map_err(|e| e.to_string())?;
            let temp_file = writer.into_inner().map_err(|e| e.to_string())?;
            let mmap = unsafe { memmap2::MmapOptions::new().map(&temp_file) }
                .map_err(|e| e.to_string())?;

            Ok(Arc::new(AudioData {
                mmap: Some(mmap),
                samples_len,
                sample_rate,
                channels,
            }))
        })();

        match result {
            Ok(data) => {
                *audio_data.lock().unwrap() = data;
                play_pos.store(0, Ordering::Relaxed);
                is_playing.store(true, Ordering::Relaxed);
                // Clear error on success
                *last_decode_error.lock().unwrap() = None;
                println!("✅ Audio loaded: {}", path);
            }
            Err(e) => {
                eprintln!("❌ Load error for {}: {}", path, e);
                // Store error so UI can poll and show toast
                *last_decode_error.lock().unwrap() = Some(e);
            }
        }
    });

    Ok(())
}

/// UI polls this after calling play_audio.
/// Returns Some(error_message) if decode failed, None if ok or still loading.
/// Calling this CONSUMES the error (clears it), so toast is shown only once.
#[tauri::command]
pub fn get_decode_status(state: tauri::State<'_, AppState>) -> Option<String> {
    state.last_decode_error.lock().unwrap().take()
}

#[derive(serde::Serialize)]
pub struct AudioFile {
    name: String,
    path: String,
    is_dir: bool,
}

#[tauri::command]
pub fn read_android_dir(path: String) -> Result<Vec<AudioFile>, String> {
    let mut files = Vec::new();
    let dir = std::fs::read_dir(&path).map_err(|e| format!("Gagal membaca folder {}: {}", path, e))?;
    
    for entry in dir {
        if let Ok(entry) = entry {
            let path_buf = entry.path();
            
            // Gunakan file_type bawaan OS (lebih handal di Android) daripada mengecek path
            let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            
            // let ext = path_buf.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
            // Tampilkan SEMUA file (hapus filter ketat) karena Symphonia bisa auto-probe formatnya
            if let Some(name) = path_buf.file_name().and_then(|n| n.to_str()) {
                files.push(AudioFile {
                    name: name.to_string(),
                    path: path_buf.to_string_lossy().to_string(),
                    is_dir,
                });
            }
        }
    }
    
    // Sort directories first, then alphabetically
    files.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            b.is_dir.cmp(&a.is_dir) // true comes first
        } else {
            a.name.cmp(&b.name)
        }
    });
    
    Ok(files)
}

#[tauri::command]
pub fn pause_audio(state: tauri::State<'_, AppState>) {
    let prev = state.is_playing.load(Ordering::Relaxed);
    state.is_playing.store(!prev, Ordering::Relaxed);
}

#[tauri::command]
pub fn seek_audio(frame: u64, state: tauri::State<'_, AppState>) {
    state.seek_pos.store(frame, Ordering::Relaxed);
}

#[tauri::command]
pub fn update_eq_bands(bands: Vec<BandConfig>, state: tauri::State<'_, AppState>) {
    let _ = state.eq_tx.send(bands);
}

#[tauri::command]
pub fn update_compressor(config: CompressorConfig, state: tauri::State<'_, AppState>) {
    let _ = state.compressor_tx.send(config);
}

#[tauri::command]
pub fn update_reverb(config: ReverbConfig, state: tauri::State<'_, AppState>) {
    let _ = state.reverb_tx.send(config);
}

#[tauri::command]
pub fn update_delay(config: DelayConfig, state: tauri::State<'_, AppState>) {
    let _ = state.delay_tx.send(config);
}

#[tauri::command]
pub fn update_chorus(config: ChorusConfig, state: tauri::State<'_, AppState>) {
    let _ = state.chorus_tx.send(config);
}

#[tauri::command]
pub fn update_flanger(config: FlangerConfig, state: tauri::State<'_, AppState>) {
    let _ = state.flanger_tx.send(config);
}

#[tauri::command]
pub fn update_distort(config: DistortConfig, state: tauri::State<'_, AppState>) {
    let _ = state.distort_tx.send(config);
}

#[tauri::command]
pub fn get_compressor_meter(state: tauri::State<'_, AppState>) -> f32 {
    f32::from_bits(state.compressor_gr.load(Ordering::Relaxed))
}

#[tauri::command]
pub fn get_meter_levels(state: tauri::State<'_, AppState>) -> [f32; 4] {
    [0, 1, 2, 3].map(|i| {
        let v = state.peaks.take(i);
        if v > 1e-5 { 20.0 * v.log10() } else { -100.0 }
    })
}

#[tauri::command]
pub fn get_playback_info(state: tauri::State<'_, AppState>) -> (bool, u64, u64, u32) {
    let playing = state.is_playing.load(Ordering::Relaxed);
    let pos     = state.play_pos.load(Ordering::Relaxed);
    let (total, sample_rate) = {
        let d  = state.audio_data.lock().unwrap();
        let ch = d.channels.max(1) as u64;
        (d.samples_len as u64 / ch, d.sample_rate)
    };
    (playing, pos, total, sample_rate)
}

#[tauri::command]
pub fn get_oscilloscope_data(state: tauri::State<'_, AppState>) -> (Vec<f32>, Vec<f32>) {
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

#[tauri::command]
pub fn play_midi_note(note: u8, on: bool, state: tauri::State<'_, AppState>) {
    let event = if on {
        MidiEvent::NoteOn(note)
    } else {
        MidiEvent::NoteOff(note)
    };
    let _ = state.midi_tx.send(event);
}

#[tauri::command]
pub fn update_midi_bypass(bypassed: bool, state: tauri::State<'_, AppState>) {
    let _ = state.midi_tx.send(MidiEvent::SetBypass(bypassed));
}
