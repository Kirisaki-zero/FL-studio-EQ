# FL EQ Studio v4 (Tauri Edition)

Sebuah aplikasi pemutar audio dan equalizer parametrik *real-time* berperforma tinggi, terinspirasi oleh FL Studio. Dibangun dengan kombinasi antarmuka web modern (HTML/CSS/JS Vanilla) dan backend audio bertenaga **Rust** via **Tauri v2**.

![Tampilan FL EQ Studio](https://raw.githubusercontent.com/Kirisaki-zero/FL-studio-EQ/main/screenshot.png) *(Ganti URL ini dengan screenshot aplikasi Anda nanti)*

## ✨ Fitur Utama

- **Audio Engine Bertenaga Rust**: Memanfaatkan CPAL untuk pemrosesan audio *low-latency* dan Symphonia untuk *decoding* format *lossless* (FLAC, WAV, dll).
- **Parametric Equalizer (7-Band)**: Filter suara interaktif dengan opsi bentuk kurva (Low Shelf, High Shelf, Peaking, High Pass, Low Pass, dll) yang menggunakan *Biquad Filters* matematika akurat.
- **Graphic Equalizer (EQUO)**: Slider EQ presisi untuk 7 frekuensi utama.
- **Efek Studio Lengkap (FX)**:
  - 🎛️ **Compressor**: Kontrol dinamika dengan indikator *Gain Reduction* aktual.
  - 🎸 **Distortion**: Soft-clip / Overdrive untuk memberikan karakter pada suara.
  - 🌊 **Modulation**: Chorus dan Flanger dengan parameter komprehensif.
  - 🌌 **Spatial**: Fruity Reverb (Schroeder algorithm) dan Delay dengan *feedback looping*.
- **Visualisasi Audio Real-time**:
  - **Oscilloscope**: Menggambar gelombang stereo secara langsung pada 60fps.
  - **Spectrum Analyzer**: Menampilkan respons frekuensi lagu (Pink/Sine/Flat noise modes) yang bereaksi terhadap perubahan EQ.
  - **Goniometer**: Visualisasi fase *stereo field*.
  - **VU Meter Akurat**: Menampilkan metrik *Peak* (Stereo L/R, Mid/Side) lengkap dengan penanda level *clipping*.
- **MIDI Synthesizer Bawaan**: Keyboard piano mini yang bisa dimainkan dan suaranya mengalir melewati seluruh rantai FX.

## 🚀 Teknologi yang Digunakan

*   **Frontend**: Vanilla JavaScript, HTML5 Canvas (untuk performa visual 60fps), CSS3 (Flexbox/Grid, Animasi).
*   **Backend / Bridge**: [Tauri v2](https://tauri.app/) (Menggabungkan web dengan native desktop).
*   **Audio DSP**: Rust murni.
    *   `cpal`: Interaksi audio OS native.
    *   `symphonia`: Audio decoder bertenaga (mendukung FLAC, MP3, WAV, dll).
    *   `biquad`: Filter DSP matematis untuk EQ.
    *   `ringbuf`: Pengiriman data *lock-free* antar thread audio dan UI (mencegah patah-patah/stuttering).

## 🛠️ Cara Menginstal & Menjalankan

### Persyaratan Sistem
Pastikan Anda sudah menginstal:
1. [Node.js](https://nodejs.org/) (untuk Vite/NPM)
2. [Rust & Cargo](https://rustup.rs/)
3. [Prasyarat Tauri (Build Tools untuk Windows/Mac/Linux)](https://tauri.app/v1/guides/getting-started/prerequisites)

### Menjalankan di Mode Development
1. Clone repositori ini.
2. Buka terminal di folder root proyek.
3. Jalankan perintah instalasi dependensi web:
   ```bash
   npm install
   ```
4. Jalankan aplikasi di mode *development* (dilengkapi dengan *hot-reload* untuk UI dan Rust):
   ```bash
   npm run tauri dev
   ```

### Membangun File Installer (Production)
Untuk mem-build menjadi aplikasi `.exe` atau `.msi`:
```bash
npm run tauri build
```
File installer akan berada di `src-tauri/target/release/bundle/`.

## 🧠 Struktur Kode & Algoritma (Audio Pipeline)

Alur pemrosesan suara di dalam engine Rust dijalankan secara linear dan murni matematis untuk mempertahankan kualitas suara tertinggi:

1. **Decoder**: Lagu di-*decode* menggunakan Symphonia menjadi sampel Float32.
2. **Buffer MMap**: Data lagu utuh disalin ke sebuah *Mem-mapped temporary file* di disk (untuk meminimalkan penggunaan RAM tanpa membebani CPU saat *playback*).
3. **Pipeline Efek (Per Sample)**:
   `Source (Lagu/Synth) -> Parametric EQ -> Compressor -> Distortion -> Flanger -> Chorus -> Delay -> Reverb -> Output Audio Device`
4. Selama pemrosesan, data diekstrak dan diumpankan ke sebuah `RingBuffer` (Lock-Free SPSC) untuk diambil oleh UI Tauri, sehingga sinkronisasi animasi (VU meter, Oscilloscope) tidak pernah memberatkan thread audio.

## 📝 To-Do List (Roadmap)
- [ ] Implementasi penyimpanan konfigurasi (*Preset Save/Load* ke disk).
- [ ] Optimasi thread latar belakang saat efek di-*bypass* untuk menghemat daya.
- [ ] Fitur *Drag & Drop* untuk memasukkan file lagu ke *Library*.
- [ ] Ekspor audio yang sudah diproses (Bouncing track ke file `.wav`).

---
Dibuat dengan ❤️ untuk eksplorasi DSP (Digital Signal Processing) dan pengembangan antarmuka desktop modern.
