use std::sync::Arc;
use std::sync::atomic::Ordering;
use claxon::FlacReader;

use super::state::{AppState, AudioData, BandConfig, CompressorConfig, ReverbConfig, DelayConfig, ChorusConfig, FlangerConfig, DistortConfig};

#[tauri::command]
pub fn play_audio(path: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let audio_data = Arc::clone(&state.audio_data);
    let play_pos   = Arc::clone(&state.play_pos);
    let is_playing = Arc::clone(&state.is_playing);

    is_playing.store(false, Ordering::Relaxed);

    std::thread::spawn(move || {
        let result = (|| -> Result<Arc<AudioData>, String> {
            use std::io::{Read, Seek, SeekFrom};
            let mut file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
            
            let mut magic = [0u8; 4];
            if file.read_exact(&mut magic).is_ok() && &magic[0..3] == b"ID3" {
                let mut header = [0u8; 6];
                if file.read_exact(&mut header).is_ok() {
                    let mut size = ((header[2] as u64) << 21)
                        | ((header[3] as u64) << 14)
                        | ((header[4] as u64) << 7)
                        | (header[5] as u64);
                    if (header[1] & 0x10) != 0 { size += 10; }
                    file.seek(SeekFrom::Current(size as i64)).map_err(|e| e.to_string())?;
                }
            } else {
                file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
            }

            let mut sync_buf = [0u8; 4];
            let mut found = false;
            if file.read_exact(&mut sync_buf).is_ok() {
                if &sync_buf == b"fLaC" {
                    found = true;
                    file.seek(SeekFrom::Current(-4)).unwrap();
                } else {
                    let mut search_limit = 1024 * 1024;
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

            use std::io::{Write, BufWriter};
            let temp_file = tempfile::tempfile().map_err(|e| e.to_string())?;
            let mut writer = BufWriter::with_capacity(1024 * 1024, temp_file);
            let mut samples_len = 0;

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

            writer.flush().map_err(|e| e.to_string())?;
            let temp_file = writer.into_inner().map_err(|e| e.to_string())?;

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
pub fn get_playback_info(state: tauri::State<'_, AppState>) -> (bool, u64, u64) {
    let playing = state.is_playing.load(Ordering::Relaxed);
    let pos     = state.play_pos.load(Ordering::Relaxed);
    let total   = {
        let d  = state.audio_data.lock().unwrap();
        let ch = d.channels.max(1) as u64;
        d.samples_len as u64 / ch
    };
    (playing, pos, total)
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
