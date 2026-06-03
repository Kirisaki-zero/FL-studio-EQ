# FL EQ Studio v4 (Tauri Edition)

Desktop aplikasi audio processor & visualizer dengan antarmuka premium yang terinspirasi dari FL Studio. Aplikasi ini dirancang untuk pemrosesan audio real-time dengan latensi sangat rendah menggunakan backend Rust dan frontend HTML5/JS (Tauri).

---

## 🚀 Fitur Utama & Penjelasan Singkat
FL EQ Studio v4 menggabungkan visualisasi audio tingkat lanjut dengan modul pemrosesan efek (FX Rack) yang kuat. Pengguna dapat memutar file audio FLAC secara real-time, mengutak-atik Equalizer parametrik/grafis, mengompresi dinamis, dan menambahkan efek spatial/modulasi seperti Reverb, Delay, Chorus, Flanger, dan Distortion—semuanya divisualisasikan secara langsung melalui Oscilloscope, Goniometer, dan Spectrum Analyzer.

---

## 🛠️ Detail Teknologi, Sistem, & Algoritma

### 1. Teknologi Stack
* **Frontend**: HTML5, Vanilla JavaScript, CSS3 (Custom styling dengan HSL tailoring & dark mode premium), Tabler Icons, Google Fonts (JetBrains Mono & Syne).
* **Backend**: Rust, Tauri v2 (Framework desktop lightweight), CPAL (Cross-Platform Audio Library) untuk output audio hardware, Claxon (FLAC decoder), Biquad (desain koefisien filter IIR).

### 2. Arsitektur & Komunikasi Sistem
* **Lock-Free Audio Thread**: Pemrosesan audio utama berjalan pada thread khusus dengan prioritas tinggi. Thread ini bersifat *zero-allocation* dan bebas hambatan (lock-free) untuk mencegah stuttering/drop-out audio.
* **Memory-Mapped Audio (Mmap)**: Sinyal FLAC yang didekodekan disimpan ke dalam temporary file dan diakses menggunakan `memmap2`. Hal ini memungkinkan streaming data audio berukuran besar dengan konsumsi RAM yang sangat minimal dan waktu load yang instan.
* **High-Precision Resampling**: Menggunakan interpolasi linier presisi tinggi untuk mengubah sample rate audio sumber (misal 44.1kHz atau 48kHz) ke sample rate hardware keluaran perangkat secara dinamis.
* **Lock-Free IPC (Inter-Process Communication)**: Pembaruan parameter dari UI ke audio thread menggunakan channel `crossbeam-channel` non-blocking (SPSC/MPSC) yang dikirimkan setiap kali slider/knob bergeser.

### 3. Algoritma DSP (Digital Signal Processing)
* **Equalizer (Parametric & Graphic)**:
  * Menggunakan filter IIR Biquad (Direct Form II Transposed) untuk pemrosesan audio berkualitas tinggi.
  * Mendukung tipe filter: *Peaking EQ*, *Low Shelf*, *High Shelf*, *Low Pass*, *High Pass*, dan *Notch*.
  * Dilengkapi update koefisien *anti-clicking* untuk mencegah zipper-noise saat knob frekuensi/gain diputar secara cepat.
  * Menggabungkan input dari Parametric EQ (7 band) dan Graphic EQ (EQUO/7 Band) ke dalam satu rantai filter serial di backend.
* **Compressor**:
  * Menggunakan detektor puncak stereo-linked.
  * Rantai *Gain Computer* dengan interpolasi kuadratik untuk kurva *Soft-Knee* yang halus.
  * Envelope generator menggunakan pemulusan eksponensial (Exponential Smoothing) untuk fase Attack dan Release yang akurat.
  * Sistem umpan balik Gain Reduction (GR) real-time ke UI meter.
* **Distortion**:
  * Menyediakan 4 tipe waveshaper: *Tanh Soft Clipping*, *Hard Clipping*, *Foldback*, dan *Asymmetrical Fuzz*.
  * Post-distortion *One-Pole Low-Pass Filter* untuk kontrol Tone warna suara.
  * *Gain Compensation* otomatis untuk menjaga keseimbangan volume saat drive dinaikkan.
* **Flanger**:
  * Modulasi delay baris sangat pendek (base delay 1ms, deviasi modulasi hingga 5ms) yang digerakkan oleh LFO sinus bersama.
  * Feedback dilengkapi dengan pembatas lunak (`tanh`) untuk mencegah resonansi ekstrem yang merusak speaker.
* **Chorus**:
  * Dual modulated delay line (base delay 15ms, LFO depth hingga 10ms).
  * LFO kanan memiliki offset fase (0-360 derajat) terhadap LFO kiri untuk memperlebar citra stereo (stereo widening).
  * Pembacaan delay menggunakan interpolasi linier fraksional.
* **Delay**:
  * Jalur delay stereo (hingga 2000ms) dengan filter Low-Pass IIR satu kutub (One-Pole) di dalam loop feedback untuk simulasi delay analog/vintage yang meredup di frekuensi tinggi.
* **Reverb (Freeverb)**:
  * Menggunakan model Freeverb yang dimodifikasi. Terdiri dari 8 Comb filter yang berjalan paralel untuk menghasilkan densitas pantulan awal, diikuti oleh 4 Allpass filter serial per channel untuk difusi gema.
  * Fitur tambahan berupa buffer Pre-Delay (hingga 200ms) dan kontrol lebar stereo (Stereo Width).

### 4. Sistem Visualisasi Real-Time
* **Oscilloscope**: Mengirimkan data sampel secara real-time dari thread audio melalui lock-free ring buffer (`ringbuf`) ke UI untuk digambar pada Canvas 2D dengan frame-rate tinggi.
* **Lissajous Goniometer**: Menganalisis korelasi fase antara channel kiri (L) dan kanan (R) secara real-time untuk memetakan lebar stereo, keseimbangan (balance), dan arah fase sinyal.
* **VU Meter**: Menghitung amplitudo puncak absolut dalam format desibel (dBFS) secara terpisah untuk sinyal Left, Right, Mid, dan Side (pemrosesan M/S visualizer).

---

## 📝 To-Do List (Fitur yang Belum Diimplementasikan di Backend)

Daftar berikut diambil dari elemen antarmuka (`ui/index.html`) yang saat ini masih berupa mockup visual/simulasi lokal dan belum terhubung ke sistem pemrosesan audio backend Rust:

### 1. 🎛️ Mixer Routing & Fader System
- [ ] **Master Volume & Gain (`masterVolTrack`)**: Slider volume Master di tab Mixer saat ini hanya mengubah persentase di UI. Perlu dihubungkan ke pengontrol master gain pada akhir rantai pemrosesan audio di Rust.
- [ ] **Insert Tracks (125 Saluran)**: Tab Insert menampilkan saluran mixer terpisah dengan efek tersendiri. Rantai audio saat ini hanya mendukung 1 jalur master global (semua efek berjalan seri). Perlu arsitektur multi-track mixer di backend.
- [ ] **Volume per Insert & Bus Groups**: Fader volume untuk track Insert dan Bus (seperti Drum Bus, Vocal Bus) saat ini hanya simulasi lokal di JavaScript. Perlu dibuatkan matrix routing volume di audio engine.
- [ ] **Current Track Simulation**: Tab "Current" bertugas menganalisis trek aktif secara dinamis. Diperlukan sistem pengalihan rute audio visualizer ke trek yang sedang dipilih pengguna.
- [ ] **Dynamic Signal Routing Map**: Tampilan grafis aliran sinyal (`routingCV`) saat ini digambar statis di Canvas. Perlu dibuat dinamis berdasarkan konfigurasi input-output trek aktif.

### 2. 🔌 Plugin Library & Slots (Dynamic Loading)
- [ ] **Dynamic Slot Loading**: Slot plugin pada Master Track (seperti slot Maximus, Limiter, EQ 2) saat ini hanya berupa tombol toggle visual. Diperlukan backend manager yang dapat memuat, menonaktifkan (bypass), memindahkan, atau menghapus plugin secara dinamis dari memori audio thread.
- [ ] **Plugin Library Integration**: Daftar library di tab "Plugin" (termasuk Fruity Convolver, Maximus, Dynamic EQ) adalah data statis. Setiap jenis pemrosesan ini perlu diimplementasikan sebagai DSP module terpisah yang dapat dimasukkan ke dalam slot audio thread.

### 3. 🎹 MIDI Synthesizer & Piano Roll
- [ ] **MIDI Audio Generation**: Menekan tuts piano di tab MIDI saat ini hanya memicu visualisasi roll di Canvas tanpa mengeluarkan suara. Perlu dibuatkan synthesizer sederhana (misal oscillator gelombang sinus/gergaji atau sampler basic) di Rust untuk menerjemahkan input note MIDI menjadi sinyal audio.
- [ ] **Piano Roll Editor / Timeline**: Fitur untuk merekam, meletakkan, dan memutar ulang susunan note MIDI dari timeline (Piano Roll editor) belum terhubung ke sequencer audio.

### 4. 🎵 Library Lagu & Pemutar Audio Lokal
- [ ] **Lagu Tab Player**: Pemutar musik di dalam tab "Lagu" menggunakan HTML5 `new Audio(url)` bawaan browser. Hal ini menyebabkan lagu yang diputar di tab tersebut melewati (bypass) seluruh efek DSP dan visualizer di Rust. Perlu diintegrasikan agar pemutaran lagu dari library lokal dikirim ke backend Tauri menggunakan perintah `play_audio`.
- [ ] **Real Waveform Preview**: Tampilan waveform lagu di tab lagu (`waveCV`) saat ini hanya berupa simulasi gelombang sinus animasi, bukan visualisasi bentuk gelombang asli dari file audio yang sedang diputar.

### 5. 🛡️ Fitur Tambahan & Pemolesan EQ
- [ ] **M/S EQ Processing**: Tombol "M/S" di topbar mengubah visualisasi VU meter menjadi Mid/Side, tetapi pemrosesan EQ di Rust (`eq.rs`) saat ini masih menerapkan filter yang sama secara stereo. Perlu pemisahan pemrosesan sinyal Mid (L+R) dan Side (L-R).
- [ ] **Linear Phase EQ**: Tombol "LINEAR" di topbar saat ini hanya simulasi. Struktur IIR Biquad saat ini memiliki pergeseran fase (minimum phase). Diperlukan modul filter FIR (Finite Impulse Response) dengan algoritma konvolusi/FFT untuk mendukung pemrosesan linear phase tanpa distorsi fase.
