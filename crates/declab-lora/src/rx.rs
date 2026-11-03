//! The LoRa receiver: finds frames of any spreading factor in a channel
//! sampled at twice the LoRa bandwidth, synchronises to them and hands back
//! their payloads. The coding rate comes from each frame's header; the
//! spreading factor is found by trying them all.
//!
//! **Detection, by brute force over SF 7–12.** For each spreading factor
//! every other sample (one per chip) is cut into windows of N = 2^SF,
//! dechirped and FFT'd. A preamble's upchirps all land in one bin whatever
//! the windows' alignment, since the preamble repeats; any other SF leaves a
//! residual chirp spread over the band. [`DETECT_RUN`] windows in a row with
//! their peak in the same bin (±1), well above the mean bin, start a frame.
//! The cost is one N-point FFT per N chips per SF.
//!
//! **Synchronisation** (as gr-lora_sdr's `frame_sync` does it, by other
//! means for the fractions). With the windows late by δ chips on the
//! symbols and a carrier offset of c bins, an upchirp peaks at δ + c and a
//! downchirp at −δ + c.
//! - The fraction of c is the turn of the preamble's peak from one window to
//!   the next: the preamble repeats, so only the carrier offset turns it.
//! - With it removed, of the two half-chip phases of the twice-oversampled
//!   stream the one with the sharper preamble peak is within a quarter chip
//!   of the symbols.
//! - The integer bins of the preamble (u) and of a downchirp (d) then give
//!   c = (u + d)/2, taken within ±N/4, and δ = u − c.
//! - The downchirps begin on that grid where two whole downchirps line up
//!   best (three places tried); the header follows 2.25 symbols on, after the
//!   two sync-word symbols' upchirps, which are read for the network's sync
//!   word.
//! - The rest of the timing, a fraction of a chip, is where the preamble's
//!   peak sits between bins on windows moved onto that grid.
//!
//! **Sampling the symbols.** Each symbol's chips are taken at their exact
//! times, interpolated from the twice-oversampled stream. Taken a fraction r
//! of a chip off, a symbol's phase turns by 2πr where its chirp wraps,
//! splitting the peak: −3 dB at a quarter chip.
//!
//! **Clock offset.** A transmitter's crystal moves its carrier and its chip
//! rate alike, as the receiver's moves its LO and ADC, so with the carrier
//! frequency known the symbols drift by N·c_Hz/f_c chips each, counted from
//! where the timing was measured, and the sampling follows them.
//! Unfollowed, a 255-byte SF12 frame with 12 ppm between the ends slides 20
//! chips.

use std::sync::Arc;

use num_complex::Complex32 as Iq;
use rustfft::{Fft, FftPlanner};

use crate::coding::{self, HEADER_SYMBOLS, Header};

pub const SF_MIN: u8 = 7;
pub const SF_MAX: u8 = 12;
/// Samples per chip of the stream the receiver takes.
pub const OS: usize = 2;
/// Windows in a row with one peak that make a preamble.
const DETECT_RUN: usize = 4;
/// Windows looked through after the detection for the downchirps: a long
/// preamble (Meshtastic sends 16 symbols) and the sync word.
const MAX_SYNC_WINDOWS: usize = 40;
/// Windows of the preamble used for the fractional carrier offset and the
/// phase.
const MAX_PREAMBLE_WINDOWS: usize = 8;

/// A frame received.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub sf: u8,
    pub header: Header,
    /// The CRC matched (None: the frame has none).
    pub crc_ok: Option<bool>,
    /// The payload as decoded with low-data-rate optimisation on.
    pub ldro: bool,
    /// The sync word (Meshtastic's is 0x2B, LoRaWAN's 0x34).
    pub sync_word: u8,
    pub payload: Vec<u8>,
    /// Codewords the Hamming code corrected or could not check.
    pub bad_codewords: usize,
    /// Signal to noise ratio in the LoRa bandwidth, dB.
    pub snr_db: f32,
    /// Mean power of the frame's samples, dB full scale.
    pub level_db: f32,
    /// The carrier offset, Hz (the transmitter high: positive).
    pub cfo_hz: f64,
    /// Sample (at the receiver's rate) of the first header symbol.
    pub at: u64,
    /// Symbols after the preamble and sync: header and payload.
    pub symbols: usize,
}

/// Counts since the receiver was made.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Preambles found.
    pub preambles: u64,
    /// Preambles without a usable downchirp after them.
    pub sync_failed: u64,
    /// Headers whose checksum failed.
    pub header_errors: u64,
    pub frames: u64,
    pub crc_errors: u64,
}

/// One spreading factor's tables and its detector.
struct Sf {
    sf: u8,
    n: usize,
    fft: Arc<dyn Fft<f32>>,
    /// The base upchirp, one sample per chip.
    up: Vec<Iq>,
    /// Its conjugate: multiplying by it dechirps upchirps.
    down: Vec<Iq>,
    /// Next detection window (stream index, phase 0).
    next: u64,
    /// The last window's peak bin and how many windows in a row had it.
    run_bin: usize,
    run: usize,
}

/// A window's spectrum peak.
#[derive(Clone, Copy)]
struct Peak {
    bin: usize,
    power: f32,
    mean: f32,
}

impl Peak {
    /// Clear of the noise: the largest of N noise bins is about ln N times
    /// the mean, rarely ln N + 4.
    fn strong(&self, n: usize) -> bool {
        self.power > self.mean * ((n as f32).ln() + 4.0)
    }
}

fn near(a: usize, b: usize, n: usize) -> bool {
    let d = (a + n - b) % n;
    d <= 1 || d == n - 1
}

/// Looking for the downchirps after a preamble.
struct Sync {
    s: usize,
    /// The last detection window (stream index, phase 0).
    det: u64,
    k_u: usize,
    /// The next window to look at, in windows from `det`.
    m: i64,
    /// Preamble windows: offset from `det`, the peak's complex value and
    /// power.
    pre: Vec<(i64, Iq, f32)>,
    /// The first window that held a downchirp, and its peak power.
    down: Option<(u64, f32)>,
    /// The window chosen as a whole downchirp (stream index, phase 0).
    w_d: Option<u64>,
}

/// Demodulating a frame's symbols.
struct Demod {
    s: usize,
    /// Stream index of the first header symbol's window.
    start: u64,
    /// Where that symbol really starts (the window less the fractional
    /// timing found on the preamble), in stream samples.
    t0: f64,
    /// Where the timing was measured (the preamble windows' mean): the
    /// clock offset's drift counts from there.
    anchor: f64,
    /// Integer carrier offset, bins (0..N).
    c_int: usize,
    /// Dechirping chirp with the fractional carrier offset taken out.
    chirp: Vec<Iq>,
    /// Stream samples each symbol starts earlier than nominal (clock offset).
    drift: f64,
    sync_word: u8,
    cfo_hz: f64,
    bins: Vec<u16>,
    header: Option<Header>,
    need: usize,
    /// The LDRO setting being decoded; whether the other one was tried
    /// (after a CRC failure), and what the first try gave.
    ldro: bool,
    alt_tried: bool,
    fallback: Option<(coding::Payload, bool, usize)>,
    sig: f64,
    noise: f64,
    power: f64,
}

enum State {
    Search,
    Sync(Sync),
    Demod(Demod),
}

pub struct Receiver {
    bw_hz: f64,
    /// RF frequency of the channel, Hz (0: unknown, no clock tracking).
    fc_hz: f64,
    sfs: Vec<Sf>,
    buf: Vec<Iq>,
    /// Stream index of `buf[0]`.
    base: u64,
    state: State,
    spec: Vec<Iq>,
    scratch: Vec<Iq>,
    acc: Vec<f32>,
    stats: Stats,
}

/// Where between bins the peak at `k` of the unwindowed DFT `x` lies,
/// −0.5..0.5 (Jacobsen's three-bin estimator with Candan's tan(π/N)/(π/N)
/// correction for a rectangular window: E. Jacobsen, P. Kootsookos, "Fast,
/// Accurate Frequency Estimators", IEEE SP Magazine 2007; Ç. Candan, IEEE
/// SP Letters 2011).
fn frac_bin(x: &[Iq], k: usize) -> f32 {
    let n = x.len();
    let (a, b, c) = (x[(k + n - 1) % n], x[k], x[(k + 1) % n]);
    let den = b * 2.0 - a - c;
    if den.norm_sqr() == 0.0 {
        return 0.0;
    }
    let s = std::f32::consts::PI / n as f32;
    (((a - c) / den).re * s.tan() / s).clamp(-0.5, 0.5)
}

impl Receiver {
    /// A receiver for a LoRa channel `bw_hz` wide, fed at `OS × bw_hz`.
    pub fn new(bw_hz: f64) -> Self {
        let mut planner = FftPlanner::new();
        let sfs: Vec<Sf> = (SF_MIN..=SF_MAX)
            .map(|sf| {
                let n = 1usize << sf;
                let up: Vec<Iq> = (0..n)
                    .map(|k| {
                        let k = k as f64;
                        let ph = 2.0 * std::f64::consts::PI * (k * k / (2.0 * n as f64) - 0.5 * k);
                        Iq::new(ph.cos() as f32, ph.sin() as f32)
                    })
                    .collect();
                Sf {
                    sf,
                    n,
                    fft: planner.plan_fft_forward(n),
                    down: up.iter().map(|z| z.conj()).collect(),
                    up,
                    next: 0,
                    run_bin: 0,
                    run: 0,
                }
            })
            .collect();
        let max_n = 1 << SF_MAX;
        let scratch_len = sfs
            .iter()
            .map(|s| s.fft.get_inplace_scratch_len())
            .max()
            .unwrap_or(0);
        Receiver {
            bw_hz,
            fc_hz: 0.0,
            sfs,
            buf: Vec::new(),
            base: 0,
            state: State::Search,
            spec: vec![Iq::new(0.0, 0.0); max_n],
            scratch: vec![Iq::new(0.0, 0.0); scratch_len],
            acc: vec![0.0; max_n],
            stats: Stats::default(),
        }
    }

    pub fn bandwidth(&self) -> f64 {
        self.bw_hz
    }

    /// The channel's RF frequency, for the clock-offset tracking.
    pub fn set_carrier(&mut self, fc_hz: f64) {
        self.fc_hz = fc_hz;
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// A frame is being received (preamble found, not yet finished).
    pub fn busy(&self) -> Option<u8> {
        match &self.state {
            State::Search => None,
            State::Sync(s) => Some(self.sfs[s.s].sf),
            State::Demod(d) => Some(self.sfs[d.s].sf),
        }
    }

    /// Forget everything (a retune).
    pub fn reset(&mut self) {
        self.base += self.buf.len() as u64;
        self.buf.clear();
        self.state = State::Search;
        let at = self.base;
        for s in &mut self.sfs {
            (s.next, s.run) = (at, 0);
        }
    }

    /// Take `x` (the next samples, at `OS` per chip); frames found go to `out`.
    pub fn process(&mut self, x: &[Iq], out: &mut Vec<Frame>) {
        self.buf.extend_from_slice(x);
        // Rust note: `std::mem::replace` takes the state out so the step
        // functions can borrow `self` whole; each puts back the next state.
        loop {
            let state = std::mem::replace(&mut self.state, State::Search);
            let (next, moved) = match state {
                State::Search => self.search(),
                State::Sync(s) => self.sync(s),
                State::Demod(d) => self.demod(d, out),
            };
            self.state = next;
            if !moved {
                break;
            }
        }
        self.trim();
    }

    fn end(&self) -> u64 {
        self.base + self.buf.len() as u64
    }

    /// The spectrum of the N chips from stream index `start` (every
    /// `OS`th sample) times `chirp`, or else the base chirp that dechirps
    /// `upchirps` (or downchirps), into `self.spec[..n]`.
    fn spectrum(&mut self, s: usize, start: u64, upchirps: bool, chirp: Option<&[Iq]>) -> Peak {
        let sf = &self.sfs[s];
        let n = sf.n;
        let at = (start - self.base) as usize;
        let c: &[Iq] = match chirp {
            Some(c) => c,
            None if upchirps => &sf.down,
            None => &sf.up,
        };
        let x = &self.buf[at..at + OS * n];
        for ((y, xs), &cc) in self.spec[..n].iter_mut().zip(x.as_chunks::<OS>().0).zip(c) {
            *y = xs[0] * cc;
        }
        sf.fft
            .process_with_scratch(&mut self.spec[..n], &mut self.scratch);
        let mut best = (0, 0.0f32);
        let mut total = 0.0f32;
        for (k, z) in self.spec[..n].iter().enumerate() {
            let p = z.norm_sqr();
            total += p;
            if p > best.1 {
                best = (k, p);
            }
        }
        Peak {
            bin: best.0,
            power: best.1,
            mean: total / n as f32,
        }
    }

    fn search(&mut self) -> (State, bool) {
        let end = self.end();
        for s in 0..self.sfs.len() {
            let n = self.sfs[s].n;
            while self.sfs[s].next + (OS * n) as u64 <= end {
                let at = self.sfs[s].next;
                let p = self.spectrum(s, at, true, None);
                let sf = &mut self.sfs[s];
                sf.next += (OS * n) as u64;
                if !p.strong(n) {
                    sf.run = 0;
                    continue;
                }
                if sf.run > 0 && near(p.bin, sf.run_bin, n) {
                    sf.run += 1;
                } else {
                    sf.run = 1;
                }
                sf.run_bin = p.bin;
                if sf.run >= DETECT_RUN {
                    sf.run = 0;
                    self.stats.preambles += 1;
                    let sync = Sync {
                        s,
                        det: at,
                        k_u: p.bin,
                        m: 1 - DETECT_RUN as i64,
                        pre: Vec::new(),
                        down: None,
                        w_d: None,
                    };
                    return (State::Sync(sync), true);
                }
            }
        }
        (State::Search, false)
    }

    /// Back to searching from stream index `from` on, every SF.
    fn resume(&mut self, from: u64) -> (State, bool) {
        for s in &mut self.sfs {
            if s.next < from {
                (s.next, s.run) = (from, 0);
            }
        }
        (State::Search, true)
    }

    fn sync(&mut self, mut y: Sync) -> (State, bool) {
        let n = self.sfs[y.s].n;
        let step = (OS * n) as u64;
        let end = self.end();
        loop {
            if let Some(w_d) = y.w_d {
                // Room for the downchirps' start to be tried a symbol either
                // side, with two whole downchirps from each.
                if w_d + 3 * step + OS as u64 > end {
                    return (State::Sync(y), false);
                }
                return match self.lock(&y, w_d) {
                    Some(d) => (State::Demod(d), true),
                    None => {
                        self.stats.sync_failed += 1;
                        self.resume(w_d + step)
                    }
                };
            }
            let w = (y.det as i64 + step as i64 * y.m) as u64;
            if w + step > end {
                return (State::Sync(y), false);
            }
            let pu = self.spectrum(y.s, w, true, None);
            let x_ku = self.spec[y.k_u];
            let pd = self.spectrum(y.s, w, false, None);
            let is_down = pd.strong(n) && pd.power > pu.power;
            match y.down {
                None if is_down => y.down = Some((w, pd.power)),
                None => {
                    if pu.strong(n) && near(pu.bin, y.k_u, n) {
                        y.pre.push((y.m, x_ku, x_ku.norm_sqr()));
                    }
                }
                // The window after the first downchirp window: the better
                // of the two is the whole downchirp.
                Some((first, p)) => y.w_d = Some(if is_down && pd.power > p { w } else { first }),
            }
            y.m += 1;
            if y.w_d.is_none() && y.m > MAX_SYNC_WINDOWS as i64 {
                self.stats.sync_failed += 1;
                return self.resume(w + step);
            }
        }
    }

    /// Carrier offset, timing and the header's start, from the preamble
    /// windows and the downchirp window starting at `w_d`.
    fn lock(&mut self, y: &Sync, w_d: u64) -> Option<Demod> {
        let s = y.s;
        let n = self.sfs[s].n;
        let step = (OS * n) as u64;
        // Strong preamble windows only: one straddling the preamble's end
        // turns its peak by a part-window, not by the offset.
        let mut powers: Vec<f32> = y.pre.iter().map(|p| p.2).collect();
        powers.sort_by(f32::total_cmp);
        let median = *powers.get(powers.len() / 2)?;
        let pre: Vec<&(i64, Iq, f32)> = y.pre.iter().filter(|p| p.2 >= 0.5 * median).collect();
        let turn: Iq = pre
            .windows(2)
            .filter(|w| w[1].0 == w[0].0 + 1)
            .map(|w| w[1].1 * w[0].1.conj())
            .sum();
        if turn.norm_sqr() == 0.0 {
            return None;
        }
        let c_frac = turn.arg() / (2.0 * std::f32::consts::PI);
        let rot = |k: usize| {
            let ph = -2.0 * std::f32::consts::PI * c_frac * k as f32 / n as f32;
            Iq::new(ph.cos(), ph.sin())
        };
        let chirp_up: Vec<Iq> = self.sfs[s]
            .down
            .iter()
            .enumerate()
            .map(|(k, &c)| c * rot(k))
            .collect();
        let chirp_down: Vec<Iq> = self.sfs[s]
            .up
            .iter()
            .enumerate()
            .map(|(k, &c)| c * rot(k))
            .collect();

        // The half-chip phase with the sharper preamble peak, and the
        // preamble's bin on it.
        let windows: Vec<u64> = pre
            .iter()
            .rev()
            .take(MAX_PREAMBLE_WINDOWS)
            .map(|p| (y.det as i64 + p.0 * step as i64) as u64)
            .collect();
        let mut best = (0usize, 0usize, -1.0f32);
        for phase in 0..OS {
            self.acc[..n].fill(0.0);
            for &w in &windows {
                self.spectrum(s, w + phase as u64, true, Some(&chirp_up));
                for (a, z) in self.acc[..n].iter_mut().zip(&self.spec[..n]) {
                    *a += z.norm_sqr();
                }
            }
            let (k, p) = self.acc[..n]
                .iter()
                .enumerate()
                .fold((0, 0.0f32), |b, (k, &p)| if p > b.1 { (k, p) } else { b });
            if p > best.2 {
                best = (phase, k, p);
            }
        }
        let (phase, u, _) = best;
        let d = self
            .spectrum(s, w_d + phase as u64, false, Some(&chirp_down))
            .bin;

        // c = (u + d)/2 within ±N/4 (u + d is even unless a bin slipped:
        // then both neighbours are tried), δ = u − c; the downchirps start on
        // that grid at one of three places. Score: two whole downchirps.
        let sum = (u + d) % n;
        let mut cands = vec![sum / 2, sum / 2 + n / 2];
        if sum % 2 == 1 {
            cands.extend([sum / 2 + 1, sum / 2 + 1 + n / 2]);
        }
        let mut pick: Option<(f32, usize, u64)> = None;
        for c in cands {
            let c = c % n;
            let delta = (u + n - c) % n;
            // Symbol starts are ≡ (w + phase) − OS·δ (mod OS·N).
            let grid = (w_d + phase as u64 + step * (n as u64) - (OS * delta) as u64) % step;
            let d0 = w_d + phase as u64 - (w_d + phase as u64 + step - grid) % step;
            for dd in [d0.saturating_sub(step), d0, d0 + step] {
                if dd < self.base + 4 * step || dd + 2 * step > self.end() {
                    continue;
                }
                let mut score = 0.0;
                for k in 0..2 {
                    self.spectrum(s, dd + k * step, false, Some(&chirp_down));
                    score += self.spec[c].norm_sqr();
                }
                if pick.is_none_or(|p| score > p.0) {
                    pick = Some((score, c, dd));
                }
            }
        }
        let (_, c_int, dstart) = pick?;

        // The sync word: two upchirps at 8 × each nibble, before the downchirps.
        let mut nib = [0u8; 2];
        for (k, v) in nib.iter_mut().enumerate() {
            let b = self
                .spectrum(s, dstart - (2 - k as u64) * step, true, Some(&chirp_up))
                .bin;
            *v = ((((b + n - c_int) % n + 4) / 8) & 0x0f) as u8;
        }
        let c_signed = if c_int > n / 2 {
            c_int as f64 - n as f64
        } else {
            c_int as f64
        };
        let cfo_hz = (c_signed + f64::from(c_frac)) * self.bw_hz / n as f64;
        let drift = if self.fc_hz > 0.0 {
            step as f64 * cfo_hz / self.fc_hz
        } else {
            0.0
        };
        // What is left of the timing, a fraction of a chip: on preamble
        // windows moved onto the symbol grid (so no wrap falls inside them)
        // the peak sits that far past c. Measured there, the clock drift
        // counts from there.
        let (mut late, mut anchor, mut count) = (0.0f64, 0.0f64, 0usize);
        for &w in &windows {
            let k = ((dstart as f64 - w as f64) / step as f64).round() as i64;
            let a = (dstart as i64 - k * step as i64) as u64;
            if a < self.base || self.spectrum(s, a, true, Some(&chirp_up)).bin != c_int {
                continue;
            }
            late += f64::from(frac_bin(&self.spec[..n], c_int));
            anchor += a as f64;
            count += 1;
        }
        if count == 0 {
            return None;
        }
        let (late, anchor) = (late / count as f64, anchor / count as f64);
        let start = dstart + 2 * step + step / 4;
        Some(Demod {
            s,
            start,
            t0: start as f64 - OS as f64 * late,
            anchor,
            c_int,
            chirp: chirp_up,
            drift,
            sync_word: nib[0] << 4 | nib[1],
            cfo_hz,
            bins: Vec::new(),
            header: None,
            need: HEADER_SYMBOLS,
            ldro: coding::ldro_rule(self.sfs[s].sf, self.bw_hz),
            alt_tried: false,
            fallback: None,
            sig: 0.0,
            noise: 0.0,
            power: 0.0,
        })
    }

    /// Where symbol `j` starts, stream samples (fractional): nominally a
    /// symbol after the last, earlier by the clock offset's drift since the
    /// timing was measured.
    fn symbol_start(&self, d: &Demod, j: usize) -> f64 {
        let step = (OS * self.sfs[d.s].n) as f64;
        let nominal = d.t0 + j as f64 * step;
        nominal - d.drift * (nominal - d.anchor) / step
    }

    /// As [`Receiver::spectrum`], the chips sampled at their exact times
    /// from `at` (a fractional stream index) by an 8-tap Blackman-windowed
    /// sinc on the twice-oversampled stream. One sample a chip taken off
    /// the chip boundaries turns the phase at the chirp's wrap by 2π × the
    /// offset, splitting the peak (−3 dB at a quarter chip).
    fn spectrum_at(&mut self, s: usize, at: f64, chirp: &[Iq]) -> Peak {
        let n = self.sfs[s].n;
        let i0 = at.floor();
        let mu = at - i0;
        let mut h = [0.0f32; 8];
        for (k, t) in h.iter_mut().enumerate() {
            let x = k as f64 - 3.0 - mu;
            let sinc = if x.abs() < 1e-9 {
                1.0
            } else {
                (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
            };
            let w = 0.42
                + 0.5 * (std::f64::consts::PI * x / 4.0).cos()
                + 0.08 * (std::f64::consts::PI * x / 2.0).cos();
            *t = (sinc * w) as f32;
        }
        let gain: f32 = h.iter().sum();
        for t in &mut h {
            *t /= gain;
        }
        let first = (i0 as u64 - 3 - self.base) as usize;
        let x = &self.buf[first..first + OS * (n - 1) + 8];
        for (j, (y, &c)) in self.spec[..n].iter_mut().zip(chirp).enumerate() {
            let v: Iq = x[OS * j..OS * j + 8]
                .iter()
                .zip(&h)
                .map(|(&a, &b)| a * b)
                .sum();
            *y = v * c;
        }
        self.sfs[s]
            .fft
            .process_with_scratch(&mut self.spec[..n], &mut self.scratch);
        let mut best = (0, 0.0f32);
        let mut total = 0.0f32;
        for (k, z) in self.spec[..n].iter().enumerate() {
            let p = z.norm_sqr();
            total += p;
            if p > best.1 {
                best = (k, p);
            }
        }
        Peak {
            bin: best.0,
            power: best.1,
            mean: total / n as f32,
        }
    }

    fn demod(&mut self, mut d: Demod, out: &mut Vec<Frame>) -> (State, bool) {
        let (n, sf) = (self.sfs[d.s].n, self.sfs[d.s].sf);
        loop {
            while d.bins.len() < d.need {
                let at = self.symbol_start(&d, d.bins.len());
                if at as u64 + (OS * n) as u64 + 8 > self.end() {
                    return (State::Demod(d), false);
                }
                // Rust note: the chirp is taken out of `d` for the call, as
                // `spectrum_at` borrows the whole receiver mutably.
                let chirp = std::mem::take(&mut d.chirp);
                let p = self.spectrum_at(d.s, at, &chirp);
                d.chirp = chirp;
                d.bins.push(((p.bin + n - d.c_int) % n) as u16);
                let noise = (p.mean * n as f32 - p.power) / (n - 1) as f32;
                d.sig += f64::from(p.power - noise).max(0.0);
                d.noise += f64::from(noise);
                d.power += f64::from(p.mean) / n as f64;
                if d.bins.len() == HEADER_SYMBOLS {
                    let Some((h, _)) = coding::decode_header(&d.bins, sf) else {
                        self.stats.header_errors += 1;
                        let from = self.symbol_start(&d, HEADER_SYMBOLS) as u64;
                        return self.resume(from);
                    };
                    d.header = Some(h);
                    d.need = coding::frame_symbols(h, sf, d.ldro);
                }
            }
            // Decoded as the LDRO rule says; on a CRC failure the other
            // setting is tried (an SX128x has no LDRO, whatever the rule),
            // waiting for its extra symbols if it has more.
            let h = d.header.expect("header decoded");
            let k = coding::frame_symbols(h, sf, d.ldro);
            let got = coding::decode_payload(&d.bins[..k], sf, h, d.ldro).map(|p| (p, d.ldro, k));
            let ok = got.as_ref().is_some_and(|g| g.0.crc_ok != Some(false));
            if !ok && h.crc && !d.alt_tried {
                d.alt_tried = true;
                d.fallback = got;
                d.ldro = !d.ldro;
                d.need = coding::frame_symbols(h, sf, d.ldro).max(d.bins.len());
                continue;
            }
            let got = if ok { got } else { d.fallback.take().or(got) };
            return self.finish(d, got, out);
        }
    }

    fn finish(
        &mut self,
        d: Demod,
        got: Option<(coding::Payload, bool, usize)>,
        out: &mut Vec<Frame>,
    ) -> (State, bool) {
        let (n, sf) = (self.sfs[d.s].n, self.sfs[d.s].sf);
        let h = d.header.expect("header decoded");
        let symbols = got.as_ref().map_or(d.need, |g| g.2);
        if let Some((p, ldro, _)) = got {
            self.stats.frames += 1;
            if p.crc_ok == Some(false) {
                self.stats.crc_errors += 1;
            }
            let snr = d.sig / (n as f64 * d.noise.max(1e-30));
            out.push(Frame {
                sf,
                header: h,
                crc_ok: p.crc_ok,
                ldro,
                sync_word: d.sync_word,
                payload: p.bytes,
                bad_codewords: p.bad_codewords,
                snr_db: (10.0 * snr.max(1e-6).log10()) as f32,
                level_db: (10.0 * (d.power / d.bins.len() as f64).max(1e-30).log10()) as f32,
                cfo_hz: d.cfo_hz,
                at: d.start,
                symbols,
            });
        }
        let from = self.symbol_start(&d, symbols) as u64;
        self.resume(from)
    }

    /// Drop samples nothing will look at again.
    fn trim(&mut self) {
        let keep = match &self.state {
            State::Search => self
                .sfs
                .iter()
                .map(|s| s.next.saturating_sub((OS * s.n * DETECT_RUN) as u64))
                .min()
                .unwrap_or(0),
            State::Sync(y) => y
                .det
                .saturating_sub((OS * self.sfs[y.s].n * DETECT_RUN) as u64),
            // The whole frame: a retry with the other LDRO setting may end
            // it earlier than the symbols already taken.
            State::Demod(d) => d.start.saturating_sub(4 * OS as u64),
        };
        let drop = keep.saturating_sub(self.base) as usize;
        // Draining shifts the buffer: only when it is worth it.
        if drop > 0 && drop >= self.buf.len() / 2 {
            let drop = drop.min(self.buf.len());
            self.buf.drain(..drop);
            self.base += drop as u64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tx::{self, Noise, TxConfig};

    /// Frames at every SF through the receiver: a carrier offset, a start
    /// that is no whole number of chips, noise; one after another.
    fn receive(
        cfg: &TxConfig,
        payloads: &[Vec<u8>],
        cfo_hz: f64,
        ppm: f64,
        fc_hz: f64,
        snr_db: f64,
        seed: u64,
    ) -> Vec<Frame> {
        let rate = OS as f64 * cfg.bw_hz;
        let mut x = vec![Iq::new(0.0, 0.0); 1237 + seed as usize % 7];
        for p in payloads {
            x.extend(tx::modulate(p, cfg, rate, cfo_hz, ppm));
            x.extend(vec![Iq::new(0.0, 0.0); 3000 + (1 << cfg.sf) * 5]);
        }
        // No channel filter here: the noise is given per sample, as the
        // receiver's one sample a chip sees it (the plug-in filters to the
        // LoRa band before; unfiltered, white noise over 2 × BW would fold
        // in 3 dB more).
        Noise::new(seed).add(&mut x, snr_db, rate, rate);
        let mut rx = Receiver::new(cfg.bw_hz);
        rx.set_carrier(fc_hz);
        let mut out = Vec::new();
        for chunk in x.chunks(10_000) {
            rx.process(chunk, &mut out);
        }
        out
    }

    #[test]
    fn every_sf_and_rate_decodes_with_an_offset_and_noise() {
        for sf in SF_MIN..=SF_MAX {
            for cr in [1u8, 4] {
                let cfg = TxConfig {
                    cr,
                    ..TxConfig::meshtastic(sf, 125e3, 1)
                };
                let payloads: Vec<Vec<u8>> = (0..2)
                    .map(|k| {
                        (0..30 + 11 * k)
                            .map(|i| (i * 7 + k + usize::from(sf)) as u8)
                            .collect()
                    })
                    .collect();
                // 3.3 kHz is 26 bins at SF10.
                let got = receive(
                    &cfg,
                    &payloads,
                    3_300.0,
                    0.0,
                    0.0,
                    0.0,
                    u64::from(sf) * 10 + u64::from(cr),
                );
                assert_eq!(got.len(), 2, "sf {sf} cr {cr}: {got:?}");
                for (f, p) in got.iter().zip(&payloads) {
                    assert_eq!(
                        (f.sf, f.header.cr, f.crc_ok, f.sync_word),
                        (sf, cr, Some(true), 0x2B)
                    );
                    assert_eq!(&f.payload, p);
                    assert!(
                        (f.cfo_hz - 3_300.0).abs() < 30.0,
                        "sf {sf}: cfo {}",
                        f.cfo_hz
                    );
                    assert!((f.snr_db - 0.0).abs() < 2.0, "sf {sf}: snr {}", f.snr_db);
                }
            }
        }
    }

    /// SF12 at 125 kHz (LDRO on), 255 bytes, both ends 12 ppm apart at
    /// 869.5 MHz: 10.4 kHz off, and the chips slide 13 chips over the frame
    /// unless followed.
    #[test]
    fn a_long_sf12_frame_with_a_clock_offset() {
        let cfg = TxConfig::meshtastic(12, 125e3, 4);
        assert!(cfg.ldro);
        let p: Vec<u8> = (0..255).map(|i| (i * 13) as u8).collect();
        let fc = 869.5e6;
        let got = receive(
            &cfg,
            std::slice::from_ref(&p),
            12e-6 * fc,
            12.0,
            fc,
            -5.0,
            3,
        );
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(
            (got[0].crc_ok, got[0].ldro, &got[0].payload),
            (Some(true), true, &p)
        );
        // Without the carrier frequency nothing follows the drift.
        let got = receive(
            &cfg,
            std::slice::from_ref(&p),
            12e-6 * fc,
            12.0,
            0.0,
            -5.0,
            3,
        );
        assert!(got.iter().all(|f| f.crc_ok != Some(true)), "{got:?}");
    }

    #[test]
    fn frac_bin_finds_a_tone_between_bins() {
        let n = 256;
        for f in [-0.45f32, -0.3, 0.0, 0.17, 0.4] {
            let mut x: Vec<Iq> = (0..n)
                .map(|k| {
                    Iq::from_polar(
                        1.0,
                        2.0 * std::f32::consts::PI * (20.0 + f) * k as f32 / n as f32,
                    )
                })
                .collect();
            rustfft::FftPlanner::new()
                .plan_fft_forward(n)
                .process(&mut x);
            assert!(
                (frac_bin(&x, 20) - f).abs() < 0.01,
                "{f}: {}",
                frac_bin(&x, 20)
            );
        }
    }

    /// LongFast (SF11, 250 kHz, 4/5, no LDRO: every bin counts), 255
    /// bytes, 15 ppm apart at 869.525 MHz: the symbols slide 7 chips.
    #[test]
    fn a_long_longfast_frame_with_a_clock_offset() {
        let cfg = TxConfig::meshtastic(11, 250e3, 1);
        assert!(!cfg.ldro);
        let p: Vec<u8> = (0..255).map(|i| (i * 31 + 7) as u8).collect();
        let fc = 869.525e6;
        let got = receive(
            &cfg,
            std::slice::from_ref(&p),
            15e-6 * fc,
            15.0,
            fc,
            -3.0,
            5,
        );
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!((got[0].crc_ok, &got[0].payload), (Some(true), &p));
        assert_eq!(got[0].bad_codewords, 0);
    }

    #[test]
    fn noise_alone_finds_nothing() {
        let bw = 250e3;
        let mut x = vec![Iq::new(0.0, 0.0); 2_000_000];
        Noise::new(9).add(&mut x, 0.0, bw, OS as f64 * bw);
        let mut rx = Receiver::new(bw);
        let mut out = Vec::new();
        for chunk in x.chunks(65_536) {
            rx.process(chunk, &mut out);
        }
        assert!(out.is_empty());
        assert_eq!(rx.stats().preambles, 0, "{:?}", rx.stats());
    }
}
