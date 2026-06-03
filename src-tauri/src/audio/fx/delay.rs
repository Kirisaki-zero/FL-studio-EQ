use crate::audio::state::DelayConfig;
use std::f32::consts::PI;

pub struct Delay {
    buffer_l: Vec<f32>,
    buffer_r: Vec<f32>,
    write_pos: usize,
    buffer_len: usize,
    sample_rate: f32,
    
    // Config
    config: DelayConfig,

    // Feedback LP filter state
    lp_l: f32,
    lp_r: f32,
}

impl Delay {
    pub fn new(sample_rate: f32) -> Self {
        // Max delay 2000ms + some padding
        let max_samples = (sample_rate * 2.5) as usize; 
        Self {
            buffer_l: vec![0.0; max_samples],
            buffer_r: vec![0.0; max_samples],
            write_pos: 0,
            buffer_len: max_samples,
            sample_rate,
            config: DelayConfig::default(),
            lp_l: 0.0,
            lp_r: 0.0,
        }
    }

    pub fn set_config(&mut self, config: DelayConfig) {
        self.config = config;
    }

    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.config.bypassed || self.config.wet <= 0.0 {
            return (l, r);
        }

        let delay_samples = (self.config.time * 0.001 * self.sample_rate) as usize;
        let delay_samples = delay_samples.clamp(1, self.buffer_len - 1);

        // Calculate read position
        let mut read_pos = self.write_pos as isize - delay_samples as isize;
        if read_pos < 0 {
            read_pos += self.buffer_len as isize;
        }
        let read_pos = read_pos as usize;

        // Read from buffer
        let delayed_l = self.buffer_l[read_pos];
        let delayed_r = self.buffer_r[read_pos];

        // Apply LP filter on the delayed signal
        // Simple one-pole IIR low-pass filter
        let cutoff = self.config.cut;
        let rc = 1.0 / (2.0 * PI * cutoff);
        let dt = 1.0 / self.sample_rate;
        let alpha = dt / (rc + dt);

        self.lp_l = self.lp_l + alpha * (delayed_l - self.lp_l);
        self.lp_r = self.lp_r + alpha * (delayed_r - self.lp_r);

        // Feedback calculation (Ping-Pong / Pan)
        let feed = self.config.feed;
        let _pan = self.config.pan; // -1 (L) to +1 (R)

        // Ping-pong effect can be achieved by feeding L into R and R into L based on pan.
        // For standard delay, we just feed back L to L, R to R.
        // We'll blend cross-feedback based on Pan if we wanted true ping-pong, but let's stick to standard stereo delay.
        // Actually, FL Studio Delay often supports ping pong. Let's do simple straight feedback for now.
        let feed_l = self.lp_l * feed;
        let feed_r = self.lp_r * feed;

        // Write to buffer
        self.buffer_l[self.write_pos] = l + feed_l;
        self.buffer_r[self.write_pos] = r + feed_r;

        // Increment write position
        self.write_pos += 1;
        if self.write_pos >= self.buffer_len {
            self.write_pos = 0;
        }

        // Output mix
        let wet = self.config.wet;
        let out_l = l + self.lp_l * wet;
        let out_r = r + self.lp_r * wet;

        (out_l, out_r)
    }
}
