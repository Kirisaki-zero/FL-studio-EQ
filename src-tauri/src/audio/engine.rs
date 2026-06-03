use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU32, Ordering};
use crossbeam_channel::Receiver;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::state::{AudioData, BandConfig, CompressorConfig, Peaks, NO_SEEK};
use super::eq::EqCh;
use super::fx::compressor::Compressor;

pub fn audio_thread(
    audio_data:   Arc<Mutex<Arc<AudioData>>>,
    play_pos:     Arc<AtomicU64>,
    seek_pos:     Arc<AtomicU64>,
    is_playing:   Arc<AtomicBool>,
    eq_rx:        Receiver<Vec<BandConfig>>,
    comp_rx:      Receiver<CompressorConfig>,
    comp_gr:      Arc<AtomicU32>,
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

    let mut compressor = Compressor::new(out_sr);
    let mut cur_comp   = CompressorConfig::default();

    // Local snapshot of AudioData Arc — refreshed cheaply without blocking.
    let mut local_data: Arc<AudioData> = Arc::new(AudioData::default());

    // Local high-precision playback position (f64) owned by audio thread.
    // This avoids atomic float arithmetic per sample.
    let mut local_pos: f64 = 0.0;

    let stream = device.build_output_stream(
        &config,
        move |data: &mut [f32], _| {
            // ── Step 1: Receive Config updates (non-blocking) ─────────────
            while let Ok(bands) = eq_rx.try_recv() {
                if bands != cur_bands {
                    cur_bands = bands;
                    eq_l.update(&cur_bands);
                    eq_r.update(&cur_bands);
                }
            }
            while let Ok(comp) = comp_rx.try_recv() {
                cur_comp = comp;
            }

            // ── Step 2: Snapshot audio data Arc (brief lock, just ptr swap) ─
            if let Ok(guard) = audio_data.try_lock() {
                if !Arc::ptr_eq(&*guard, &local_data) {
                    local_data = Arc::clone(&*guard);
                    local_pos  = 0.0;
                }
            }

            // ── Step 3: Handle seek (ultra-low latency) ───────────────────
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

                // Apply Compressor
                let (lout, rout) = compressor.process(lout, rout, &cur_comp);

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

            // Publish position for UI display
            play_pos.store(local_pos as u64, Ordering::Relaxed);
            
            // Publish Gain Reduction for Compressor UI Meter
            comp_gr.store(compressor.gr_db.to_bits(), Ordering::Relaxed);
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
