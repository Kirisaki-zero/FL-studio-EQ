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

// Macro ini menghasilkan isi callback audio tanpa duplikasi kode.
// Parameter `$write_frame` menangani perbedaan antara f32 dan i16.
macro_rules! make_audio_cb {
    (
        $out_sr:expr, $out_ch:expr,
        $audio_data:expr, $play_pos:expr, $seek_pos:expr, $is_playing:expr,
        $eq_rx:expr, $comp_rx:expr, $reverb_rx:expr, $delay_rx:expr,
        $chorus_rx:expr, $flanger_rx:expr, $distort_rx:expr, $midi_rx:expr,
        $comp_gr:expr, $peaks:expr, $osc_prod:expr,
        $eq_l:expr, $eq_r:expr, $cur_bands:expr,
        $compressor:expr, $cur_comp:expr,
        $reverb:expr, $delay:expr, $chorus:expr, $flanger:expr, $distort:expr,
        $synth:expr,
        $local_data:expr, $local_pos:expr,
        $write_frame:expr
    ) => {
        move |data: &mut [_], _: &cpal::OutputCallbackInfo| {
            // ── Step 1: Receive config updates (non-blocking) ──────────────
            while let Ok(bands) = $eq_rx.try_recv() {
                if bands != $cur_bands {
                    $cur_bands = bands;
                    $eq_l.update(&$cur_bands);
                    $eq_r.update(&$cur_bands);
                }
            }
            while let Ok(comp) = $comp_rx.try_recv() { $cur_comp = comp; }
            while let Ok(rev) = $reverb_rx.try_recv() { $reverb.set_config(rev); }
            while let Ok(del) = $delay_rx.try_recv() { $delay.set_config(del); }
            while let Ok(cho) = $chorus_rx.try_recv() { $chorus.set_config(cho); }
            while let Ok(fla) = $flanger_rx.try_recv() { $flanger.set_config(fla); }
            while let Ok(dis) = $distort_rx.try_recv() { $distort.set_config(dis); }
            while let Ok(evt) = $midi_rx.try_recv() {
                match evt {
                    MidiEvent::NoteOn(note)       => $synth.note_on(note),
                    MidiEvent::NoteOff(note)      => $synth.note_off(note),
                    MidiEvent::SetBypass(bypass)  => $synth.set_bypass(bypass),
                }
            }

            // ── Step 2: Snapshot audio data Arc (brief lock, just ptr swap) ─
            if let Ok(guard) = $audio_data.try_lock() {
                if !Arc::ptr_eq(&*guard, &$local_data) {
                    $local_data = Arc::clone(&*guard);
                    $local_pos  = 0.0;
                    println!(
                        "New audio loaded: sample_rate={}, channels={}, out_sr={}",
                        $local_data.sample_rate, $local_data.channels, $out_sr
                    );
                }
            }

            // ── Step 3: Handle seek (ultra-low latency) ───────────────────
            let seek = $seek_pos.load(Ordering::Relaxed);
            if seek != NO_SEEK {
                $local_pos = seek as f64;
                $seek_pos.store(NO_SEEK, Ordering::Relaxed);
            }

            let mut file_playing = $is_playing.load(Ordering::Relaxed)
                && $local_data.samples_len > 0;
            let synth_active = $synth.is_active();

            if !file_playing && !synth_active {
                for s in data.iter_mut() { *s = $write_frame(0.0); }
                $play_pos.store($local_pos as u64, Ordering::Relaxed);
                $comp_gr.store($compressor.gr_db.to_bits(), Ordering::Relaxed);
                return;
            }

            // ── Step 4: Compute playback parameters from actual track info ─
            let src_ch    = ($local_data.channels as usize).max(1);
            let src_sr    = $local_data.sample_rate as f64;
            let ratio     = src_sr / $out_sr as f64;
            let src_total = $local_data.samples_len;

            // Read f32 samples from mmap safely
            let get_s = |fi: usize, ch: usize| -> f32 {
                let flat = fi * src_ch + ch;
                if let Some(m) = &$local_data.mmap {
                    let byte = flat * 4;
                    if byte + 4 <= m.len() {
                        return f32::from_le_bytes([m[byte], m[byte+1], m[byte+2], m[byte+3]]);
                    }
                }
                0.0
            };

            // ── Step 5: Render audio frames ───────────────────────────────
            for frame in data.chunks_mut($out_ch) {
                let mut lin = 0.0_f32;
                let mut rin = 0.0_f32;

                if file_playing {
                    let pi   = $local_pos as usize;
                    let frac = ($local_pos - pi as f64) as f32;

                    if pi * src_ch >= src_total {
                        $is_playing.store(false, Ordering::Relaxed);
                        file_playing = false;
                    } else {
                        let s0l = get_s(pi, 0);
                        let s1l = get_s(pi + 1, 0);
                        lin += s0l + (s1l - s0l) * frac;

                        if src_ch > 1 {
                            let s0r = get_s(pi, 1);
                            let s1r = get_s(pi + 1, 1);
                            rin += s0r + (s1r - s0r) * frac;
                        } else {
                            rin += lin;
                        }

                        $local_pos += ratio;
                    }
                }

                if synth_active {
                    let (sl, sr) = $synth.process();
                    lin += sl;
                    rin += sr;
                }

                // Apply FX chain
                let lout = $eq_l.run(lin);
                let rout = $eq_r.run(rin);
                let (lout, rout) = $compressor.process(lout, rout, &$cur_comp);
                let (lout, rout) = $distort.process(lout, rout);
                let (lout, rout) = $flanger.process(lout, rout);
                let (lout, rout) = $chorus.process(lout, rout);
                let (lout, rout) = $delay.process(lout, rout);
                let (lout, rout) = $reverb.process(lout, rout);

                let mid  = (lout + rout) * 0.70710678;
                let side = (lout - rout) * 0.70710678;

                $peaks.update(0, lout.abs());
                $peaks.update(1, rout.abs());
                $peaks.update(2, mid.abs());
                $peaks.update(3, side.abs());

                if $out_ch >= 1 { frame[0] = $write_frame(lout); }
                if $out_ch >= 2 { frame[1] = $write_frame(rout); }

                $osc_prod.push((lout, rout)).ok();
            }

            $play_pos.store($local_pos as u64, Ordering::Relaxed);
            $comp_gr.store($compressor.gr_db.to_bits(), Ordering::Relaxed);
        }
    };
}

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
    // Android Audio hardware lock protection:
    // Retry finding the audio device until the OS is ready and app is fully in foreground.
    let (device, sc) = loop {
        let host = cpal::default_host();
        if let Some(device) = host.default_output_device() {
            if let Ok(sc) = device.default_output_config() {
                break (device, sc);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    };

    let out_sr   = sc.sample_rate().0 as f32;
    let out_ch   = sc.channels() as usize;
    let format   = sc.sample_format();
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
    let mut local_data: Arc<AudioData> = Arc::new(AudioData::default());
    let mut local_pos: f64 = 0.0;

    let err_fn = |err| eprintln!("Audio stream error: {}", err);
    println!(">>> MENGINISIALISASI AUDIO ENGINE DENGAN FORMAT: {:?}", format);

    let stream = match format {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &config,
            make_audio_cb!(
                out_sr, out_ch,
                audio_data, play_pos, seek_pos, is_playing,
                eq_rx, comp_rx, reverb_rx, delay_rx,
                chorus_rx, flanger_rx, distort_rx, midi_rx,
                comp_gr, peaks, osc_prod,
                eq_l, eq_r, cur_bands,
                compressor, cur_comp,
                reverb, delay, chorus, flanger, distort,
                synth,
                local_data, local_pos,
                |s: f32| s
            ),
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_output_stream(
            &config,
            make_audio_cb!(
                out_sr, out_ch,
                audio_data, play_pos, seek_pos, is_playing,
                eq_rx, comp_rx, reverb_rx, delay_rx,
                chorus_rx, flanger_rx, distort_rx, midi_rx,
                comp_gr, peaks, osc_prod,
                eq_l, eq_r, cur_bands,
                compressor, cur_comp,
                reverb, delay, chorus, flanger, distort,
                synth,
                local_data, local_pos,
                |s: f32| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
            ),
            err_fn,
            None,
        ),
        cpal::SampleFormat::I32 => device.build_output_stream(
            &config,
            make_audio_cb!(
                out_sr, out_ch,
                audio_data, play_pos, seek_pos, is_playing,
                eq_rx, comp_rx, reverb_rx, delay_rx,
                chorus_rx, flanger_rx, distort_rx, midi_rx,
                comp_gr, peaks, osc_prod,
                eq_l, eq_r, cur_bands,
                compressor, cur_comp,
                reverb, delay, chorus, flanger, distort,
                synth,
                local_data, local_pos,
                |s: f32| (s.clamp(-1.0, 1.0) * i32::MAX as f32) as i32
            ),
            err_fn,
            None,
        ),
        cpal::SampleFormat::F64 => device.build_output_stream(
            &config,
            make_audio_cb!(
                out_sr, out_ch,
                audio_data, play_pos, seek_pos, is_playing,
                eq_rx, comp_rx, reverb_rx, delay_rx,
                chorus_rx, flanger_rx, distort_rx, midi_rx,
                comp_gr, peaks, osc_prod,
                eq_l, eq_r, cur_bands,
                compressor, cur_comp,
                reverb, delay, chorus, flanger, distort,
                synth,
                local_data, local_pos,
                |s: f32| s as f64
            ),
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_output_stream(
            &config,
            make_audio_cb!(
                out_sr, out_ch,
                audio_data, play_pos, seek_pos, is_playing,
                eq_rx, comp_rx, reverb_rx, delay_rx,
                chorus_rx, flanger_rx, distort_rx, midi_rx,
                comp_gr, peaks, osc_prod,
                eq_l, eq_r, cur_bands,
                compressor, cur_comp,
                reverb, delay, chorus, flanger, distort,
                synth,
                local_data, local_pos,
                |s: f32| ((s.clamp(-1.0, 1.0) + 1.0) * 0.5 * u16::MAX as f32) as u16
            ),
            err_fn,
            None,
        ),
        _ => {
            eprintln!(">>> FORMAT AUDIO {:?} TIDAK DIDUKUNG SECARA NATIVE, FALLBACK KE F32", format);
            device.build_output_stream(
                &config,
                make_audio_cb!(
                    out_sr, out_ch,
                    audio_data, play_pos, seek_pos, is_playing,
                    eq_rx, comp_rx, reverb_rx, delay_rx,
                    chorus_rx, flanger_rx, distort_rx, midi_rx,
                    comp_gr, peaks, osc_prod,
                    eq_l, eq_r, cur_bands,
                    compressor, cur_comp,
                    reverb, delay, chorus, flanger, distort,
                    synth,
                    local_data, local_pos,
                    |s: f32| s
                ),
                err_fn,
                None,
            )
        }
    }.expect("!!! GAGAL MEMBANGUN AUDIO STREAM. KEMUNGKINAN FORMAT ATAU BUFFER TIDAK COCOK !!!");

    stream.play().expect("!!! GAGAL MEMULAI AUDIO STREAM !!!");

    // Keep thread alive
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}
