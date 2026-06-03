use crate::audio::state::DistortConfig;
use std::f32::consts::PI;

pub struct Distort {
    config: DistortConfig,
    
    // One-pole LPF state for Tone
    lp_l: f32,
    lp_r: f32,
    sample_rate: f32,
}

impl Distort {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            config: DistortConfig::default(),
            lp_l: 0.0,
            lp_r: 0.0,
            sample_rate,
        }
    }

    pub fn set_config(&mut self, config: DistortConfig) {
        self.config = config;
    }

    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.config.bypassed || self.config.mix <= 0.0 {
            return (l, r);
        }

        // 1. Drive (Pre-Gain)
        // Convert drive (0..1) to linear gain multiplier (1x up to 50x)
        let drive_gain = 1.0 + (self.config.drive * 49.0);
        let in_l = l * drive_gain;
        let in_r = r * drive_gain;

        // 2. Waveshaping based on TYPE
        let dist_type = self.config.dist_type.round() as i32;
        
        let shape = |x: f32, mode: i32| -> f32 {
            match mode {
                // Type 0: Soft Clipping (Tanh)
                0 => {
                    x.tanh()
                }
                // Type 1: Hard Clipping
                1 => {
                    let threshold = 0.8;
                    x.clamp(-threshold, threshold) / threshold
                }
                // Type 2: Foldback
                2 => {
                    let threshold = 0.9;
                    if x > threshold {
                        threshold - (x - threshold)
                    } else if x < -threshold {
                        -threshold - (x + threshold)
                    } else {
                        x
                    }
                }
                // Type 3: Asymmetrical Fuzz
                3 => {
                    // Positive side soft clipped, negative side hard clamped
                    if x > 0.0 {
                        (x * 1.5).tanh()
                    } else {
                        x.clamp(-0.6, 0.0)
                    }
                }
                _ => x.tanh(), // Fallback
            }
        };

        let mut shaped_l = shape(in_l, dist_type);
        let mut shaped_r = shape(in_r, dist_type);

        // Gain compensation since drive makes it louder
        // The louder the drive, the more we compensate
        let comp = 1.0 / (1.0 + self.config.drive * 2.0);
        shaped_l *= comp;
        shaped_r *= comp;

        // 3. Tone (Post-Filter)
        // Map tone 0..1 to LPF cutoff freq (500Hz to 20kHz)
        let cutoff = 500.0 * (40.0_f32).powf(self.config.tone);
        // Calculate one-pole alpha coefficient
        let rc = 1.0 / (2.0 * PI * cutoff);
        let dt = 1.0 / self.sample_rate;
        let alpha = dt / (rc + dt);

        self.lp_l = self.lp_l + alpha * (shaped_l - self.lp_l);
        self.lp_r = self.lp_r + alpha * (shaped_r - self.lp_r);

        let filtered_l = self.lp_l;
        let filtered_r = self.lp_r;

        // 4. Mix
        let mix = self.config.mix;
        let out_l = l * (1.0 - mix) + filtered_l * mix;
        let out_r = r * (1.0 - mix) + filtered_r * mix;

        (out_l, out_r)
    }
}
