use crate::audio::state::CompressorConfig;

pub struct Compressor {
    sr: f32,
    env: f32,
    pub gr_db: f32, // For meter reading
}

impl Compressor {
    pub fn new(sr: f32) -> Self {
        Self {
            sr,
            env: 0.0,
            gr_db: 0.0,
        }
    }

    /// Process a stereo frame and return the compressed frame (L, R)
    pub fn process(&mut self, l: f32, r: f32, config: &CompressorConfig) -> (f32, f32) {
        if config.bypassed {
            self.gr_db = 0.0;
            self.env = 0.0;
            return (l, r);
        }

        // Stereo-linked peak detection
        let peak = l.abs().max(r.abs()).max(1e-6);
        let peak_db = 20.0 * peak.log10();

        // Soft-Knee Gain Computer
        let diff = peak_db - config.thresh;
        let knee_half = config.knee / 2.0;

        let over_db = if diff < -knee_half {
            0.0
        } else if diff > knee_half {
            diff
        } else {
            // Quadratic interpolation for soft knee
            (diff + knee_half).powi(2) / (2.0 * config.knee.max(0.001))
        };

        // Target Gain Reduction (positive value means reduction)
        let target_gr = over_db * (1.0 - 1.0 / config.ratio.max(1.0));

        // Exponential smoothing (Envelope generator)
        // Attack/Release times are in ms
        let attack_coef = (-1.0 / (config.attack.max(0.1) * 0.001 * self.sr)).exp();
        let release_coef = (-1.0 / (config.release.max(1.0) * 0.001 * self.sr)).exp();

        if target_gr > self.env {
            // Signal is going up -> Attack phase
            self.env = attack_coef * self.env + (1.0 - attack_coef) * target_gr;
        } else {
            // Signal is going down -> Release phase
            self.env = release_coef * self.env + (1.0 - release_coef) * target_gr;
        }

        self.gr_db = self.env;

        // Apply gain reduction and makeup gain
        let total_gain_db = config.makeup - self.env;
        let linear_gain = 10.0f32.powf(total_gain_db / 20.0);

        (l * linear_gain, r * linear_gain)
    }
}
