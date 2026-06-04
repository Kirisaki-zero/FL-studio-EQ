use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU32, Ordering};
use crossbeam_channel::Receiver;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::state::{AudioData, BandConfig, CompressorConfig, ReverbConfig, DelayConfig, ChorusConfig, FlangerConfig, DistortConfig, Peaks, NO_SEEK, MidiEvent};
use super::eq::EqCh;
use super::fx::compressor::Compressor;
use super::fx::reverb::Reverb;
use super::fx::delay::Delay;
use super::fx::chorus::Chorus;
use super::fx::flanger::Flanger;
use super::fx::distort::Distort;
use super::synth::MidiSynth;

pub fn audio_thread(
    audio_data:   Arc<Mutex<Arc<AudioData>>>,
    play_pos:     Arc<AtomicU64>,
    seek_pos:     Arc<AtomicU64>,
    is_playing:   Arc<AtomicBool>,
    eq_rx:        Receiver<Vec<BandConfig>>,
    comp_rx:      Receiver<CompressorConfig>,
    reverb_rx:    Receiver<ReverbConfig>,
    delay_rx:     Receiver<DelayConfig>,
    chorus_rx:    Receiver<ChorusConfig>,
    flanger_rx:   Receiver<FlangerConfig>,
    distort_rx:   Receiver<DistortConfig>,
    midi_rx:      Receiver<MidiEvent>,
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

    let mut reverb     = Reverb::new(out_sr);
    let mut delay      = Delay::new(out_sr);
    let mut chorus     = Chorus::new(out_sr);
    let mut flanger    = Flanger::new(out_sr);
    let mut distort    = Distort::new(out_sr);
    let mut synth      = MidiSynth::new(out_sr);

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
            while let Ok(rev) = reverb_rx.try_recv() {
                reverb.set_config(rev);
            }
            while let Ok(del) = delay_rx.try_recv() {
                delay.set_config(del);
            }
            while let Ok(cho) = chorus_rx.try_recv() {
                chorus.set_config(cho);
            }
            while let Ok(fla) = flanger_rx.try_recv() {
                flanger.set_config(fla);
            }
            while let Ok(dis) = distort_rx.try_recv() {
                distort.set_config(dis);
            }
            while let Ok(evt) = midi_rx.try_recv() {
                match evt {
                    MidiEvent::NoteOn(note) => synth.note_on(note),
                    MidiEvent::NoteOff(note) => synth.note_off(note),
                    MidiEvent::SetBypass(bypassed) => synth.set_bypass(bypassed),
                }
            }

            // ── Step 2: Snapshot audio data Arc (brief lock, just ptr swap) ─
            if let Ok(guard) = audio_data.try_lock() {
                if !Arc::ptr_eq(&*guard, &local_data) {
                    local_data = Arc::clone(&*guard);
                    local_pos  = 0.0;
                    println!(
                        "New audio loaded: sample_rate={}, channels={}, out_sr={}",
                        local_data.sample_rate,
                        local_data.channels,
                        out_sr
                    );
                }
            }

            // ── Step 3: Handle seek (ultra-low latency) ───────────────────
            let seek = seek_pos.load(Ordering::Relaxed);
            if seek != NO_SEEK {
                local_pos = seek as f64;
                seek_pos.store(NO_SEEK, Ordering::Relaxed); // acknowledge
            }

            let file_playing = is_playing.load(Ordering::Relaxed) && local_data.samples_len > 0;
            let synth_active = synth.is_active();

            if !file_playing && !synth_active {
                for s in data.iter_mut() { *s = 0.0; }
                // Publish position for UI display
                play_pos.store(local_pos as u64, Ordering::Relaxed);
                comp_gr.store(compressor.gr_db.to_bits(), Ordering::Relaxed);
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
                let mut lin = 0.0;
                let mut rin = 0.0;

                // 1. Process Audio File if playing
                if file_playing {
                    let pi   = local_pos as usize;
                    let frac = (local_pos - pi as f64) as f32;

                    // End of audio file
                    if pi * src_ch >= src_total {
                        is_playing.store(false, Ordering::Relaxed);
                    } else {
                        // Linear interpolation (L channel)
                        let file_l = {
                            let s0 = get_s(pi, 0);
                            let s1 = get_s(pi + 1, 0);
                            s0 + (s1 - s0) * frac
                        };

                        // Linear interpolation (R channel, or duplicate L if mono)
                        let file_r = if src_ch > 1 {
                            let s0 = get_s(pi, 1);
                            let s1 = get_s(pi + 1, 1);
                            s0 + (s1 - s0) * frac
                        } else {
                            file_l
                        };

                        lin += file_l;
                        rin += file_r;
                        local_pos += ratio;
                    }
                }

                // 2. Process MIDI Synth if active
                if synth_active {
                    let (synth_l, synth_r) = synth.process();
                    lin += synth_l;
                    rin += synth_r;
                }

                // Apply EQ
                let lout = eq_l.run(lin);
                let rout = eq_r.run(rin);

                // Apply Compressor
                let (lout, rout) = compressor.process(lout, rout, &cur_comp);

                // Apply Distort
                let (lout, rout) = distort.process(lout, rout);

                // Apply Flanger
                let (lout, rout) = flanger.process(lout, rout);

                // Apply Chorus
                let (lout, rout) = chorus.process(lout, rout);

                // Apply Delay
                let (lout, rout) = delay.process(lout, rout);

                // Apply Reverb
                let (lout, rout) = reverb.process(lout, rout);

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
