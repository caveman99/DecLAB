<!-- made by CasualArclamp -->
# DecLAB — working notes

Read [`docs/DESIGN.md`](docs/DESIGN.md) first: the agreed scope, the plug-in
decoder API and the milestones. Update it when a decision changes.

DecLAB brings together the DVB modes of DecDVB-T (`..\DecDVB-T`), DecSAT
(`..\DecDVB`) and DecDAB (`..\DecDAB`). Those projects stay where they are;
code is copied in, not linked.

## Commands

```bash
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run --release -p declab-gui
cargo run --release -p declab-gui -- out/sim_fm_98.000MHz_10Msps.cs8 --decoder wfm --chan 98.8M --screenshot out/x.png --after 4
cargo run --release -p declab-gui -- --radio airspy --freq 98M --decoder wfm --wheel   # opens with the decoder wheel up
cargo run --release -p declab-gui -- --radio airspy --freq 546M --chan 546M --decoder dvbt --optimise --screenshot out/x.png --after 90   # Airspy gain optimiser (also "Optimise gain" in the gain menu); logs each setting to stderr
cargo run --release -p declab-gui -- ..\DecPagers\out\sim_pagers_153.000MHz_1Msps.cs8 --decoder pagers --chan 153.15M --screenshot out/x.png --after 10   # pagers (make the file with `decpagers sim`)
cargo run -p declab-gui --example make_icon   # redraws apps/declab-gui/assets/icon.ico and icon-256.png (preview out/icon-preview.png)
cargo run --release -p declab-pmr --example dmr_make -- out/sim_dmr.cf32 15   # simulated DMR repeater: call, Tier III CSBKs, LRRP
cargo run --release -p declab-gui -- out/sim_dmr.cf32 --rate 48000 --decoder voice --screenshot out/x.png --after 8   # Digital voice: DMR, NXDN48, NXDN96, P25, TETRA detected by itself
cargo run --release -p declab-gui -- out/sim_dmr.cf32 --rate 48000 --decoder voice --tab hex --screenshot out/x.png --after 9   # Digital voice's data tabs: heard, hex, lrrp, ip
cargo run --release -p declab-cli -- run nfm --radio airspy --freq 446.05M --set squelch=noise --set squelch_noise=10   # squelch: off, level, snr or noise, each with its threshold
cargo test -p declab-decoders --release squelch_sweep -- --ignored --nocapture   # each squelch method's reading against C/N
cargo run --release -p declab-pmr --example dmr_sweep -- 4 800   # DMR sensitivity vs C/N (and CPU)
cargo run --release -p declab-pmr --example dmr_file -- IN.cf32   # every DMR event of a real 24 kS/s channel, and the counters
cargo run --release -p declab-pmr --example nxdn_make -- out/sim_nxdn.cf32 12 [96]   # simulated NXDN48 (or NXDN96) calls
cargo run --release -p declab-gui -- out/sim_nxdn.cf32 --rate 48000 --decoder voice --screenshot out/x.png --after 6
cargo run --release -p declab-cli -- run voice --radio airspy --freq FREQ --offset OFFSET --record-channel out/x.cf32 --seconds 1800   # long recording of just the decoder's channel (cf32 at its rate, 72 kS/s for voice); replay with --file out/x.cf32 --rate 72000
cargo run --release -p declab-pmr --example nxdn_file -- IN.cf32 [96] [bits]   # every NXDN frame of a real 24 kS/s channel: LICH, pass or not, eye SNR (bits: the frame's bits)
cargo run --release -p declab-pmr --example nxdn_sweep -- 4 800 - [96]   # NXDN48 (or 96) sensitivity, SNR gate on noise, CPU
cargo run --release -p declab-pmr --example p25_make -- out/sim_p25.cf32 14   # simulated P25 call and control channel (then --decoder voice)
cargo run --release -p declab-pmr --example p25_sweep -- 4 300   # P25 sensitivity, SNR gate on noise, CPU
cargo run --release -p declab-pmr --example tetra_make -- out/sim_tetra.cf32 12   # simulated TETRA cell and call (then --rate 72000 --decoder voice)
cargo run --release -p declab-pmr --example tetra_sweep -- 4 500 [diff]   # TETRA sensitivity, gate on noise, CPU (diff: differential detection)
cargo run --release -p declab-pmr --example tetra_file -- IN.cf32   # a 72 kS/s TETRA carrier's fit slot by slot (lock losses, re-alignments)
cargo run --release -p declab-cli -- run voice --radio rtlsdr --freq 439.5M --gain 40.2 --card
cargo run --release -p declab-cli -- run dab --file ../DecDAB/captures/11d_long_222.064MHz_4.096Msps_amp.cs8 --service "classic fm" --wav out/x.wav --card   # DAB/DAB+ (DecDAB's captures); AAC by libxaac
cargo run --release -p declab-gui -- ../DecDAB/captures/11d_long_222.064MHz_4.096Msps_amp.cs8 --decoder dab --screenshot out/x.png --after 12
cargo run --release -p declab-cli -- run wfm --radio airspy --freq 98M --bias-tee   # antenna-port power for an LNA (off unless given; also --bias-tee on declab-gui)
cargo run --release -p declab-decoders --example sat_make -- out/sim_sat_gse.cf32 gse 10 6 -120000   # simulated satellite carrier (s2ts, s2x, gse, dvbs), 8 MS/s
cargo run --release -p declab-cli -- run sat --file out/sim_sat_gse.cf32 --rate 8M --bandwidth 3M --pcap out/x.pcap --ts out/x.ts --set symbol_rate=2000000 --card   # DVB-S/S2/S2X; GSE to PCAP, TS to a file
cargo run --release -p declab-gui -- out/sim_sat_s2x.cf32 --rate 8M --decoder sat --chan 0 --screenshot out/x.png --after 7
cargo run --release -p declab-cli --example retune_probe -- FILE voice HZ_A HZ_B 3   # retune a running decoder from one carrier to another mid-file, as a click does; prints the view each 0.25 s
cargo run --release -p declab-cli --example vfo_probe -- FILE pagers HZ_1 voice HZ_2   # two VFOs on a recording, as the GUI runs them; both views each 0.5 s
cargo test -p declab-decoders --release relock_after_retune -- --ignored --nocapture   # Digital voice: time to relock after a retune or a drag, each mode pair
cargo run --release -p declab-lora --example mesh_make -- out/sim_meshtastic.cf32 [SNR_DB]   # simulated Meshtastic traffic: LongFast, MediumFast, a private key
cargo run --release -p declab-gui -- out/sim_meshtastic.cf32 --rate 1M --decoder meshtastic --screenshot out/x.png --after 12
cargo run --release -p declab-cli -- run meshtastic --radio rtlsdr --freq 869.525M --set bandwidth=250k   # EU_868 LongFast; any SF found by itself
cargo test -p declab-decoders --release mesh_sweep -- --ignored --nocapture   # Meshtastic: packets decoded against SNR per SF, CPU
cargo run --release -p declab-cli -- list
cargo run --release -p declab-cli -- selftest level
cargo run --release -p declab-cli -- run wfm --file x_98.000MHz_10Msps.cs8 --offset 1.2M --realtime
cargo run --release -p declab-cli -- run nfm --radio airspy --freq 446.05M --offset 0
cargo run --release -p declab-cli -- run usb --radio hackrf --freq 14.2M --gain 1,32,20
```

Only an Airspy R2 is plugged in at the moment (the HackRF is not). All
three drivers (HackRF, Airspy, RTL-SDR) are pure Rust over nusb and need the
WinUSB driver. The user's own
DecDVB-T GUI may be holding the Airspy ("Access is denied"): never kill it.

The GUI keeps its settings in `%APPDATA%\DecLAB\prefs.txt`; screenshot runs
neither read nor write it.

libxaac (DAB+ audio) builds from the `third_party/libxaac` submodule
(`git submodule update --init`); the first build takes a minute.

## Adding a decoder

Copy `crates/declab-decoders/src/level.rs`, implement `Decoder` and
`DecoderFactory` (see `crates/declab-api/src/lib.rs`), add `mod` and the
factory to `ALL` in that crate's `lib.rs`.

## House rules

- **Licence:** GPL-3.0-or-later. Credit borrowed algorithms at the function or
  module that borrows them, as the source projects do.
- **CPU efficiency is the main requirement.** Measure each change.
- **Never commit** IQ captures, reference clones, built exes, the user's
  location or personal email address.
- **Specs are the authority:** cite the ETSI clause or table next to
  constants and bit layouts.
- The user has a strong DSP background and is newer to Rust: explain Rust
  idioms in comments, not DSP basics.
