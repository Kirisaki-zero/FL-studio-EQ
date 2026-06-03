use super::state::BandConfig;
use biquad::*;

pub struct EqCh {
    filters: Vec<DirectForm2Transposed<f32>>,
    sr: f32,
}

impl EqCh {
    pub fn new(sr: f32) -> Self {
        Self { filters: vec![], sr }
    }

    pub fn update(&mut self, bands: &[BandConfig]) {
        let mut active_idx = 0;
        for b in bands {
            if b.muted || b.freq >= self.sr / 2.0 { continue; }
            let ft = match b.shape.as_str() {
                "Low Shelf"        => biquad::Type::LowShelf(b.gain),
                "High Shelf"       => biquad::Type::HighShelf(b.gain),
                "LP" | "Low Pass"  => biquad::Type::LowPass,
                "HP" | "High Pass" => biquad::Type::HighPass,
                "Notch"            => biquad::Type::Notch,
                "Band Pass"        => biquad::Type::BandPass,
                _                  => biquad::Type::PeakingEQ(b.gain),
            };
            
            // PERBAIKAN FATAL: Urutan yang benar adalah (Type, SampleRate, Frequency, Q)
            if let Ok(c) = biquad::Coefficients::<f32>::from_params(
                ft, self.sr.hz(), b.freq.hz(), b.q,
            ) {
                if active_idx < self.filters.len() {
                    // Update memori filter lama (Anti-Clicking / Zipper Noise)
                    self.filters[active_idx].update_coefficients(c);
                } else {
                    // Buat filter baru jika kurang
                    self.filters.push(DirectForm2Transposed::<f32>::new(c));
                }
                active_idx += 1;
            }
        }
        // Hapus sisa filter yang tidak terpakai
        self.filters.truncate(active_idx);
    }

    pub fn run(&mut self, s: f32) -> f32 {
        self.filters.iter_mut().fold(s, |acc, f| f.run(acc))
    }
}
