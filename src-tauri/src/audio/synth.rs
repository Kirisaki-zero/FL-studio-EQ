use std::f32::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AdsrStage {
    Attack,
    Decay,
    Sustain,
    Release,
    Idle,
}

pub struct Voice {
    note: u8,
    freq: f32,
    phase: f32,
    phase_inc: f32,
    stage: AdsrStage,
    adsr_val: f32,
}

impl Voice {
    pub fn new() -> Self {
        Self {
            note: 0,
            freq: 0.0,
            phase: 0.0,
            phase_inc: 0.0,
            stage: AdsrStage::Idle,
            adsr_val: 0.0,
        }
    }

    pub fn trigger_on(&mut self, note: u8, freq: f32, sample_rate: f32) {
        self.note = note;
        self.freq = freq;
        self.phase = 0.0;
        self.phase_inc = (2.0 * PI * freq) / sample_rate;
        self.stage = AdsrStage::Attack;
    }

    pub fn trigger_off(&mut self) {
        if !matches!(self.stage, AdsrStage::Idle) {
            self.stage = AdsrStage::Release;
        }
    }

    pub fn process(&mut self, sample_rate: f32) -> f32 {
        if matches!(self.stage, AdsrStage::Idle) {
            return 0.0;
        }

        // ADSR parameters
        let attack_time = 0.010; // 10ms (smooth start)
        let decay_time = 0.100;  // 100ms
        let sustain_level = 0.700; // 70% volume level
        let release_time = 0.200; // 200ms (smooth end)

        let dt = 1.0 / sample_rate;

        match self.stage {
            AdsrStage::Attack => {
                self.adsr_val += dt / attack_time;
                if self.adsr_val >= 1.0 {
                    self.adsr_val = 1.0;
                    self.stage = AdsrStage::Decay;
                }
            }
            AdsrStage::Decay => {
                self.adsr_val -= (dt / decay_time) * (1.0 - sustain_level);
                if self.adsr_val <= sustain_level {
                    self.adsr_val = sustain_level;
                    self.stage = AdsrStage::Sustain;
                }
            }
            AdsrStage::Sustain => {
                self.adsr_val = sustain_level;
            }
            AdsrStage::Release => {
                self.adsr_val -= (dt / release_time) * sustain_level;
                if self.adsr_val <= 0.0 {
                    self.adsr_val = 0.0;
                    self.stage = AdsrStage::Idle;
                }
            }
            AdsrStage::Idle => {}
        }

        // Triangle Wave Generator
        let sine_val = self.phase.sin();
        let val = (2.0 / PI) * sine_val.asin();

        // Increment phase
        self.phase += self.phase_inc;
        if self.phase >= 2.0 * PI {
            self.phase -= 2.0 * PI;
        }

        val * self.adsr_val
    }
}

pub struct MidiSynth {
    voices: Vec<Voice>,
    sample_rate: f32,
    pub bypassed: bool,
}

impl MidiSynth {
    pub fn new(sample_rate: f32) -> Self {
        let mut voices = Vec::with_capacity(16);
        for _ in 0..16 {
            voices.push(Voice::new());
        }
        Self {
            voices,
            sample_rate,
            bypassed: true, // Dimulai dalam keadaan mati/bypassed secara default
        }
    }

    pub fn set_bypass(&mut self, bypassed: bool) {
        self.bypassed = bypassed;
        if bypassed {
            // Matikan semua suara jika di-bypass
            for v in &mut self.voices {
                v.stage = AdsrStage::Idle;
                v.adsr_val = 0.0;
            }
        }
    }

    pub fn is_active(&self) -> bool {
        if self.bypassed {
            return false;
        }
        self.voices.iter().any(|v| v.stage != AdsrStage::Idle)
    }

    pub fn note_on(&mut self, note: u8) {
        if self.bypassed {
            return;
        }

        // Cari voice yang kosong (Idle)
        let mut target_idx = None;
        for (i, v) in self.voices.iter().enumerate() {
            if matches!(v.stage, AdsrStage::Idle) {
                target_idx = Some(i);
                break;
            }
        }

        // Jika tidak ada yang idle, rebut voice yang sedang di tahap Release
        if target_idx.is_none() {
            for (i, v) in self.voices.iter().enumerate() {
                if matches!(v.stage, AdsrStage::Release) {
                    target_idx = Some(i);
                    break;
                }
            }
        }

        // Jika tetap tidak ada, curi saja voice index pertama
        if target_idx.is_none() {
            target_idx = Some(0);
        }

        if let Some(idx) = target_idx {
            let freq = 440.0 * 2.0_f32.powf((note as f32 - 69.0) / 12.0);
            self.voices[idx].trigger_on(note, freq, self.sample_rate);
        }
    }

    pub fn note_off(&mut self, note: u8) {
        for v in &mut self.voices {
            if v.note == note && !matches!(v.stage, AdsrStage::Idle) {
                v.trigger_off();
            }
        }
    }

    pub fn process(&mut self) -> (f32, f32) {
        if self.bypassed {
            return (0.0, 0.0);
        }

        let mut sum = 0.0;
        let mut active = false;

        for v in &mut self.voices {
            if !matches!(v.stage, AdsrStage::Idle) {
                sum += v.process(self.sample_rate);
                active = true;
            }
        }

        if !active {
            return (0.0, 0.0);
        }

        // Batasi amplitudo dan beri sedikit attenuation untuk mencegah clipping
        let out = (sum * 0.25).clamp(-0.95, 0.95);
        (out, out)
    }
}
