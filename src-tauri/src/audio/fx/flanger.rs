use crate::audio::state::FlangerConfig;
use std::f32::consts::PI;

pub struct Flanger {
    buffer_l: Vec<f32>,
    buffer_r: Vec<f32>,
    write_pos: usize,
    buffer_len: usize,
    sample_rate: f32,
    
    // Config
    config: FlangerConfig,

    // LFO state
    lfo_phase: f32,
}

impl Flanger {
    pub fn new(sample_rate: f32) -> Self {
        // Max delay 10ms is plenty for flanger
        let max_samples = (sample_rate * 0.01) as usize; 
        Self {
            buffer_l: vec![0.0; max_samples],
            buffer_r: vec![0.0; max_samples],
            write_pos: 0,
            buffer_len: max_samples,
            sample_rate,
            config: FlangerConfig::default(),
            lfo_phase: 0.0,
        }
    }

    pub fn set_config(&mut self, config: FlangerConfig) {
        self.config = config;
    }

    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.config.bypassed || self.config.wet <= 0.0 {
            return (l, r);
        }

        // Base delay around 1ms
        let base_delay_samples = 0.001 * self.sample_rate;
        // Depth (modulation swing) up to 5ms
        let depth_samples = self.config.depth * 0.005 * self.sample_rate;
        
        // LFO Phase increment
        let phase_inc = (2.0 * PI * self.config.rate) / self.sample_rate;
        
        // Compute LFO output (0.0 to 1.0 using sine for sweeping)
        // Flanger sweep usually sounds best when oscillating between 0 and 1
        let lfo_val = (self.lfo_phase.sin() + 1.0) * 0.5;

        // Calculate delay in samples (fractional)
        let delay = base_delay_samples + (lfo_val * depth_samples);

        // Calculate read index with wrapping
        let mut read_pos = self.write_pos as f32 - delay;
        if read_pos < 0.0 {
            read_pos += self.buffer_len as f32;
        }

        let get_interpolated = |buf: &[f32], pos: f32, len: usize| -> f32 {
            let mut idx_int = pos.floor() as usize;
            if idx_int >= len {
                idx_int %= len;
            }
            let frac = pos - idx_int as f32;
            let idx_next = if idx_int + 1 >= len { 0 } else { idx_int + 1 };
            
            let y0 = buf[idx_int];
            let y1 = buf[idx_next];
            y0 + (y1 - y0) * frac
        };

        // Read from buffer using linear interpolation
        let wet_l = get_interpolated(&self.buffer_l, read_pos, self.buffer_len);
        let wet_r = get_interpolated(&self.buffer_r, read_pos, self.buffer_len);

        // Feedback calculation (with soft limiting to prevent harsh clipping)
        let feed = self.config.feed;
        let mut feed_l = wet_l * feed;
        let mut feed_r = wet_r * feed;
        
        // Simple soft clipper for feedback
        feed_l = feed_l.tanh();
        feed_r = feed_r.tanh();

        // Write input + feedback to buffer
        self.buffer_l[self.write_pos] = l + feed_l;
        self.buffer_r[self.write_pos] = r + feed_r;

        // Advance write pointer
        self.write_pos += 1;
        if self.write_pos >= self.buffer_len {
            self.write_pos = 0;
        }

        // Advance LFO phase (shared between L & R for standard flanger)
        self.lfo_phase += phase_inc;
        if self.lfo_phase >= 2.0 * PI {
            self.lfo_phase -= 2.0 * PI;
        }

        // Output mix
        let wet_level = self.config.wet;
        let dry_level = 1.0 - (wet_level * 0.5); // slight compensation

        let out_l = l * dry_level + wet_l * wet_level;
        let out_r = r * dry_level + wet_r * wet_level;

        (out_l, out_r)
    }
}
