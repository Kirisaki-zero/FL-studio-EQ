use crate::audio::state::ReverbConfig;

// Freeverb tunings at 44100Hz
const COMB_TUNING: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNING: [usize; 4] = [225, 341, 441, 556];
const STEREO_SPREAD: usize = 23;

struct CombFilter {
    buffer: Vec<f32>,
    pos: usize,
    filter_store: f32,
}

impl CombFilter {
    fn new(size: usize) -> Self {
        Self {
            buffer: vec![0.0; size],
            pos: 0,
            filter_store: 0.0,
        }
    }

    fn process(&mut self, input: f32, damp1: f32, damp2: f32, feedback: f32) -> f32 {
        let output = self.buffer[self.pos];
        
        self.filter_store = (output * damp2) + (self.filter_store * damp1);
        self.buffer[self.pos] = input + (self.filter_store * feedback);
        
        self.pos += 1;
        if self.pos >= self.buffer.len() {
            self.pos = 0;
        }
        output
    }
}

struct AllpassFilter {
    buffer: Vec<f32>,
    pos: usize,
}

impl AllpassFilter {
    fn new(size: usize) -> Self {
        Self {
            buffer: vec![0.0; size],
            pos: 0,
        }
    }

    fn process(&mut self, input: f32, feedback: f32) -> f32 {
        let buf_out = self.buffer[self.pos];
        let output = -input + buf_out;
        self.buffer[self.pos] = input + (buf_out * feedback);
        
        self.pos += 1;
        if self.pos >= self.buffer.len() {
            self.pos = 0;
        }
        output
    }
}

pub struct Reverb {
    config: ReverbConfig,
    
    // Comb filters (8 per channel)
    combs_l: Vec<CombFilter>,
    combs_r: Vec<CombFilter>,
    
    // Allpass filters (4 per channel)
    allpasses_l: Vec<AllpassFilter>,
    allpasses_r: Vec<AllpassFilter>,

    // Pre-delay
    pre_delay_buffer_l: Vec<f32>,
    pre_delay_buffer_r: Vec<f32>,
    pre_write_pos: usize,
    sample_rate: f32,
}

impl Reverb {
    pub fn new(sample_rate: f32) -> Self {
        let ratio = sample_rate / 44100.0;
        
        let mut combs_l = Vec::new();
        let mut combs_r = Vec::new();
        for &tune in &COMB_TUNING {
            let size = (tune as f32 * ratio) as usize;
            combs_l.push(CombFilter::new(size));
            combs_r.push(CombFilter::new(size + STEREO_SPREAD));
        }

        let mut allpasses_l = Vec::new();
        let mut allpasses_r = Vec::new();
        for &tune in &ALLPASS_TUNING {
            let size = (tune as f32 * ratio) as usize;
            allpasses_l.push(AllpassFilter::new(size));
            allpasses_r.push(AllpassFilter::new(size + STEREO_SPREAD));
        }

        Self {
            config: ReverbConfig::default(),
            combs_l,
            combs_r,
            allpasses_l,
            allpasses_r,
            pre_delay_buffer_l: vec![0.0; (sample_rate * 0.5) as usize], // up to 500ms
            pre_delay_buffer_r: vec![0.0; (sample_rate * 0.5) as usize],
            pre_write_pos: 0,
            sample_rate,
        }
    }

    pub fn set_config(&mut self, config: ReverbConfig) {
        self.config = config;
    }

    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.config.bypassed || self.config.wet <= 0.0 {
            return (l, r);
        }

        // 1. Pre-delay
        let pre_samples = (self.config.pre * 0.001 * self.sample_rate) as usize;
        let pre_samples = pre_samples.clamp(1, self.pre_delay_buffer_l.len() - 1);
        
        let mut pre_read_pos = self.pre_write_pos as isize - pre_samples as isize;
        if pre_read_pos < 0 {
            pre_read_pos += self.pre_delay_buffer_l.len() as isize;
        }
        let pre_read_pos = pre_read_pos as usize;

        let in_l = self.pre_delay_buffer_l[pre_read_pos];
        let in_r = self.pre_delay_buffer_r[pre_read_pos];

        self.pre_delay_buffer_l[self.pre_write_pos] = l;
        self.pre_delay_buffer_r[self.pre_write_pos] = r;
        self.pre_write_pos += 1;
        if self.pre_write_pos >= self.pre_delay_buffer_l.len() {
            self.pre_write_pos = 0;
        }

        // Initial mix
        let input_mix = (in_l + in_r) * 0.5 * 0.015; // gain stage before filters

        // Config mappings
        let room_size = (self.config.size * 0.28) + 0.7; // mapped to 0.7 - 0.98
        let damp1 = self.config.damp * 0.4;
        let damp2 = 1.0 - damp1;
        let diff = self.config.diff * 0.5; // allpass feedback
        
        // Comb filters
        let mut out_l = 0.0;
        let mut out_r = 0.0;

        for comb in &mut self.combs_l {
            out_l += comb.process(input_mix, damp1, damp2, room_size);
        }
        for comb in &mut self.combs_r {
            out_r += comb.process(input_mix, damp1, damp2, room_size);
        }

        // Allpass filters
        for ap in &mut self.allpasses_l {
            out_l = ap.process(out_l, diff);
        }
        for ap in &mut self.allpasses_r {
            out_r = ap.process(out_r, diff);
        }

        // Stereo Width
        let wet1 = self.config.width / 2.0 + 0.5;
        let wet2 = (1.0 - self.config.width) / 2.0;

        let rev_l = out_l * wet1 + out_r * wet2;
        let rev_r = out_r * wet1 + out_l * wet2;

        let wet = self.config.wet;
        let dry = 1.0 - (wet * 0.5);

        (l * dry + rev_l * wet, r * dry + rev_r * wet)
    }
}
