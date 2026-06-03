use crate::audio::state::ChorusConfig;
use std::f32::consts::PI;

pub struct Chorus {
    buffer_l: Vec<f32>,
    buffer_r: Vec<f32>,
    write_pos: usize,
    buffer_len: usize,
    sample_rate: f32,
    
    // Config
    config: ChorusConfig,

    // LFO state
    lfo_phase_l: f32,
    lfo_phase_r: f32,
}

impl Chorus {
    pub fn new(sample_rate: f32) -> Self {
        // Max delay 50ms is plenty for chorus
        let max_samples = (sample_rate * 0.05) as usize; 
        Self {
            buffer_l: vec![0.0; max_samples],
            buffer_r: vec![0.0; max_samples],
            write_pos: 0,
            buffer_len: max_samples,
            sample_rate,
            config: ChorusConfig::default(),
            lfo_phase_l: 0.0,
            lfo_phase_r: 0.0,
        }
    }

    pub fn set_config(&mut self, config: ChorusConfig) {
        self.config = config;
    }

    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.config.bypassed || self.config.wet <= 0.0 {
            return (l, r);
        }

        // Write input to buffer
        self.buffer_l[self.write_pos] = l;
        self.buffer_r[self.write_pos] = r;

        // Base delay around 15ms
        let base_delay_samples = 0.015 * self.sample_rate;
        // Depth (modulation swing) up to 10ms
        let depth_samples = self.config.depth * 0.010 * self.sample_rate;
        
        // LFO Phase increment
        let phase_inc = (2.0 * PI * self.config.rate) / self.sample_rate;
        
        // Compute LFO outputs (-1.0 to 1.0)
        let lfo_val_l = self.lfo_phase_l.sin();
        let lfo_val_r = self.lfo_phase_r.sin();

        // Calculate delay in samples (fractional)
        let delay_l = base_delay_samples + (lfo_val_l * depth_samples);
        let delay_r = base_delay_samples + (lfo_val_r * depth_samples);

        // Calculate read index with wrapping
        let mut read_pos_l = self.write_pos as f32 - delay_l;
        if read_pos_l < 0.0 {
            read_pos_l += self.buffer_len as f32;
        }
        
        let mut read_pos_r = self.write_pos as f32 - delay_r;
        if read_pos_r < 0.0 {
            read_pos_r += self.buffer_len as f32;
        }

        // Linear interpolation helper
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
        let wet_l = get_interpolated(&self.buffer_l, read_pos_l, self.buffer_len);
        let wet_r = get_interpolated(&self.buffer_r, read_pos_r, self.buffer_len);

        // Advance write pointer
        self.write_pos += 1;
        if self.write_pos >= self.buffer_len {
            self.write_pos = 0;
        }

        // Advance LFO phases
        self.lfo_phase_l += phase_inc;
        if self.lfo_phase_l >= 2.0 * PI {
            self.lfo_phase_l -= 2.0 * PI;
        }
        
        // Maintain relative phase offset for Right channel
        // Phase parameter is in degrees (0 - 360)
        let phase_offset_rad = self.config.phase * (PI / 180.0);
        self.lfo_phase_r = self.lfo_phase_l + phase_offset_rad;
        if self.lfo_phase_r >= 2.0 * PI {
            self.lfo_phase_r -= 2.0 * PI;
        }

        // Output mix
        let wet_level = self.config.wet;
        let dry_level = 1.0 - (wet_level * 0.5); // slight compensation

        let out_l = l * dry_level + wet_l * wet_level;
        let out_r = r * dry_level + wet_r * wet_level;

        (out_l, out_r)
    }
}
