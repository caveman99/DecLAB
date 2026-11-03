//! A LoRa modulator, for simulated captures and the tests: preamble, sync
//! word, 2.25 downchirps and the coded symbols (as gr-lora_sdr's `modulate`
//! lays them out), at any sample rate, with a clock offset if asked; and an
//! AWGN channel.

use num_complex::Complex32 as Iq;

use crate::coding;

/// What a transmitter is set to.
#[derive(Debug, Clone, Copy)]
pub struct TxConfig {
    pub sf: u8,
    pub bw_hz: f64,
    /// Coding rate 4/(4 + cr).
    pub cr: u8,
    pub crc: bool,
    pub ldro: bool,
    pub sync_word: u8,
    /// Upchirps before the sync word (Meshtastic: 16, 12 at 2.4 GHz).
    pub preamble: usize,
}

impl TxConfig {
    /// Meshtastic's framing for a modem setting.
    pub fn meshtastic(sf: u8, bw_hz: f64, cr: u8) -> Self {
        TxConfig {
            sf,
            bw_hz,
            cr,
            crc: true,
            ldro: coding::ldro_rule(sf, bw_hz),
            sync_word: crate::meshtastic::SYNC_WORD,
            preamble: if bw_hz > 600e3 { 12 } else { 16 },
        }
    }
}

/// Chirps one after another at `rate`, phase continuous, each sample taken
/// of the continuous chirp at its exact time (a running sum of frequencies
/// would put each wrap's frequency step on the sample grid, a phase error
/// of up to a quarter turn after it that no real transmitter makes), and
/// the transmitter's time carried across symbols to the fraction of a
/// sample (so a clock offset drifts the symbols as it would on the air).
struct Chirps {
    n: f64,
    /// Samples per chip.
    k: f64,
    /// Carrier offset, cycles per sample, and its phase so far (cycles).
    cfo: f64,
    cfo_phase: f64,
    /// The chirps' phase at the start of the current one, cycles.
    phase: f64,
    /// Chips into the current chirp at the next sample.
    pos: f64,
    out: Vec<Iq>,
}

impl Chirps {
    /// An upchirp from bin `up`, or a downchirp (None), `symbols` long.
    fn push(&mut self, up: Option<u16>, symbols: f64) {
        let (n, len) = (self.n, symbols * self.n);
        // The phase t chips in, cycles: the frequency (in bandwidths, i.e.
        // cycles a chip) integrated, the upchirp falling a bandwidth at its
        // wrap.
        let phi = |t: f64| match up {
            Some(s) => {
                let s = f64::from(s);
                (s / n - 0.5) * t + t * t / (2.0 * n) - (t - (n - s)).max(0.0)
            }
            None => 0.5 * t - t * t / (2.0 * n),
        };
        while self.pos < len {
            let ph = 2.0 * std::f64::consts::PI * (self.phase + phi(self.pos) + self.cfo_phase);
            self.out.push(Iq::new(ph.cos() as f32, ph.sin() as f32));
            self.cfo_phase = (self.cfo_phase + self.cfo).fract();
            self.pos += 1.0 / self.k;
        }
        self.phase = (self.phase + phi(len)).fract();
        self.pos -= len;
    }
}

/// One frame carrying `payload`, at `rate` samples a second, with a carrier
/// offset of `cfo_hz` and the transmitter's clock `ppm` fast (its chips
/// that much shorter; give the matching carrier offset as well).
pub fn modulate(payload: &[u8], cfg: &TxConfig, rate: f64, cfo_hz: f64, ppm: f64) -> Vec<Iq> {
    let k = rate / cfg.bw_hz / (1.0 + ppm * 1e-6);
    let mut c = Chirps {
        n: f64::from(1u32 << cfg.sf),
        k,
        cfo: cfo_hz / rate,
        cfo_phase: 0.0,
        phase: 0.0,
        pos: 0.0,
        out: Vec::new(),
    };
    for _ in 0..cfg.preamble {
        c.push(Some(0), 1.0);
    }
    c.push(Some(u16::from(cfg.sync_word >> 4) * 8), 1.0);
    c.push(Some(u16::from(cfg.sync_word & 0x0f) * 8), 1.0);
    // Two downchirps and a quarter, each from the top of the band (one
    // sweep 2.25 bandwidths long would leave it).
    for len in [1.0, 1.0, 0.25] {
        c.push(None, len);
    }
    for b in coding::encode(payload, cfg.sf, cfg.cr, cfg.crc, cfg.ldro) {
        c.push(Some(b), 1.0);
    }
    c.out
}

/// Seconds a frame of `len` bytes is on the air.
pub fn airtime_s(len: usize, cfg: &TxConfig) -> f64 {
    let h = coding::Header {
        len,
        cr: cfg.cr,
        crc: cfg.crc,
    };
    let symbols = cfg.preamble as f64 + 4.25 + coding::frame_symbols(h, cfg.sf, cfg.ldro) as f64;
    symbols * f64::from(1u32 << cfg.sf) / cfg.bw_hz
}

/// White Gaussian noise for a channel: a small xorshift generator and
/// Box-Muller, so the simulations repeat exactly from a seed.
pub struct Noise(u64);

impl Noise {
    pub fn new(seed: u64) -> Self {
        Noise(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// A complex sample of total power `power`.
    pub fn sample(&mut self, power: f64) -> Iq {
        let r = (-power * self.uniform().ln()).sqrt();
        let a = 2.0 * std::f64::consts::PI * self.uniform();
        Iq::new((r * a.cos()) as f32, (r * a.sin()) as f32)
    }

    /// Add noise to `x` so a signal of unit power sits `snr_db` above the
    /// noise in `bw_hz` (the noise is white over `rate`).
    pub fn add(&mut self, x: &mut [Iq], snr_db: f64, bw_hz: f64, rate: f64) {
        let power = rate / bw_hz / 10f64.powf(snr_db / 10.0);
        for s in x {
            *s += self.sample(power);
        }
    }
}
