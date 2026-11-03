<!-- made by CasualArclamp -->
# DecLAB — Design

One receiver for the DVB family and DAB: **DVB-T / DVB-T2** (from DecDVB-T),
**DVB-S, DVB-S2/S2X (TS and GSE/IP) and DVB-RCS2 return bursts** (the DVB
modes of DecSAT, in `..\DecDVB`), **DAB / DAB+** (from DecDAB), and
**analog AM, DSB, USB, LSB, CW, NFM and WFM** (stereo + RDS, from DecSAT).
HackRF One, Airspy R2 and IQ-file replay, egui GUI + CLI, same look as the
other Dec apps.

Status date: 2026-10-27 (kickoff). This document is the source of truth for
scope and milestones; update it when a decision changes.

## 1. Kickoff decisions (2026-10-27)

- **Decoders are plug-in modules.** Each decoder is one file implementing a
  small trait (`declab-api`): the host sends it IQ down-converted to what it
  asked for, plus tuning and stream position; it hands back TS packets, IP
  packets, audio, pictures, text, a services list, measurements and scope
  points. Other people can write more decoders the same way.
- **Only the DVB modes of DecSAT** come across: DVB-S, DVB-S2/S2X TS and GSE,
  DVB-RCS2/TDMA return. Not the Comtech/Intelsat/Q-Flex modems, analog,
  radar, beacons or the PSK toolbox.
- **One GUI with a mode picker** (not DecSAT's many-VFO waterfall, not tabs
  of the old apps). Shared front end, spectrum/waterfall, card, scopes,
  services list, player.
- **Repository:** private GitHub repo CasualArclamp/DecLAB, local git.
- CPU efficiency stays the main requirement, as in DecDVB-T.
- (2026-10-27, during M1) **Airspy R2 as well as the HackRF**: the pure-Rust
  driver from DecDVB-T's Airspy branch (`..\DecDVB-T-airspy`), 10 MS/s IQ.
  DecDAB's Airspy branch has a variant that goes straight to DAB's rate; the
  host's down-converter makes that unnecessary here.
- (2026-10-27) **Analog modes** too: AM, DSB, USB, LSB, CW, NFM, WFM.
- (2026-10-28) **Icon** (user chose from three mockups): a spectrum with a
  TV multiplex (blue), a DAB block (green) and an FM carrier (orange), in
  the Dec apps' dark rounded square; drawn by `declab-gui`'s `make_icon`
  example, in the exe (build.rs, winresource) and the window.
- (2026-10-28) **Airspy gain is one slider** (user): step 0-21 on libairspy's
  Sensitivity or Linearity table, chosen in the gain menu (prefs
  `airspy_mode`, `airspy_step`); the menu shows the LNA/mixer/VGA indices it
  sets. After the optimiser, which can stop between steps, the button shows
  the three indices instead.
- (2026-10-28) **Pagers**: POCSAG 512/1200/2400 and FLEX 1600–6400 from
  DecPagers (`..\DecPagers`), as the `pagers` plug-in
  (`declab-decoders/src/pagers.rs`) over a copy of its protocol crate
  (`crates/declab-pagers`). Its scopes are "Symbols" (soft symbols in time
  order: 2 or 4 bands) and "Eye"; the GUI opens on "Symbols"
  (`DecoderFactory::default_scope`, new). Low-SNR receiver: tone
  correlators, per-tone AFC, Chase BCH (see DecPagers' DESIGN §3a).
- (2026-10-27) **GUI interactions** asked for: middle-drag pans the spectrum
  and waterfall; a middle click (no drag) opens a radial decoder picker like
  a game's weapon wheel — scroll to step round it, or point at a slice, and
  click to choose. UI mockups come before the GUI is built.

## 2. Shape

```
IqSource (HackRF / file)            ── declab-io (one copy, from DecDVB-T's)
  │
  ├─► spectrum + waterfall          ── host, shared
  │
  └─► DDC to the decoder's InputSpec (or untouched: sample_rate = None)
        │
        ▼ Block { iq, tuning, first_sample, dropped_before }
      Box<dyn Decoder>::process  on its own worker thread
        │
        ▼ Sink { lock, ts, ip, audio, pictures, text, services,
        │        measurements, constellation, log, retune_by_hz, custom }
  host: TS hub → player / EPG / Red Button,  IP → PCAP,  audio → speakers,
        card + history plots, services list; Panel (GUI thread) ↔ Commands
```

### The plug-in API (`crates/declab-api`)

| Piece | Role |
|---|---|
| `DecoderFactory` | static description (`DecoderInfo`: id, name, family, standard), the `InputSpec` it wants, `create()`, optional `panel()` |
| `Decoder` | `process(&Block, &mut Sink)`, `command(Command)`, `reset()`, `wants_threads()` |
| `Sink` | everything a call produces; the host drains it after each call, capacity kept so the loop does not allocate |
| `Panel` | the decoder's own settings UI on the GUI thread; talks to the decoder only through `Command` and `Sink::custom` |

Registration is explicit: `crates/declab-decoders/src/lib.rs` holds `ALL`, a
list of factories. A new decoder = one file + `mod` + one list entry
(`level.rs` is the template). Big decoders keep their DSP in crates of their
own and are wrapped by such a file.

**Later (open):** loading third-party decoders from a DLL at run time. Rust has
no stable ABI, so that needs a C-ABI shim (or `abi_stable`) over the same
trait; deferred until the in-tree API has settled on the real decoders.

### Crates

Shared crates are merged once; standard-specific DSP is copied in with a
`declab-` prefix and its history noted in the module docs.

| Crate | From |
|---|---|
| `declab-api` | new |
| `declab-decoders` | new: the one-file adapters + `ALL`; `analog.rs` (7 modes) + `analog_rds.rs` from DecSAT, `level.rs` |
| `declab-engine` | new `host.rs` (source thread, DDC + resampler, worker per decoder), `audio.rs` (cpal, from DecSAT's player); DecDVB-T's waterfall/spectrum/front end/meters |
| `declab-core` | DecDVB-T's core less its DVB tables: Iq, sample formats, errors, CRC, UHF plan |
| `declab-io` | DecDVB-T-airspy's `decdvbt-io` (HackRF, Airspy R2, files, WAV); receive only, no `hackrf_tx` yet; DecSAT's RTL/Pluto/UHD later if wanted |
| `declab-dsp` | DecDVB-T's `decdvbt-dsp` (DDC, resampler, FIR, AGC, DC); DecSAT's timing/carrier/RRC join with DVB-S |
| `declab-ts` | DecDVB-T's `decdvbt-ts` (PSI/SI, EPG with Huffman) — DecSAT's `decsat-ts` compared and merged |
| `declab-mheg` | DecDVB-T's Red Button engine |
| `declab-audio`, `declab-video` | DecDVB-T's player side; DAB audio decoders from DecDAB |
| `decdvbt-core`, `-fec`, `-ofdm`, `-rx`, `-ts`, `-mod` | copied from DecDVB-T (`5e9b249`) under their own names (see §11); `decdvbt-dsp` is `declab-dsp` (identical) |
| `declab-dvbs` | `decsat-dsp`/`-frame`/`-fec`/`-modem` DVB parts, `decsat-gse`, `decsat-ip` |
| `declab-dab` | `decdab-ofdm`, `-fic`, `-msc`, `-fec`, `-codecs`, `-data`, `decdab-fdk-sys` |
| `declab-lora` | new: LoRa receiver and coding (after gr-lora_sdr), modulator, Meshtastic packets (§28) |
| `apps/declab-gui`, `apps/declab-cli` | new, built from the three GUIs' parts |

**Licence clash, settled in M3:** DecDAB uses FDK-AAC for DAB+, DecSAT
avoided FDK because its licence does not mix with the GPL (it uses libxaac).
DecLAB is GPL-3.0-or-later, so DAB+ uses libxaac (§20).

## 3. Milestones

| M | Content | Done when |
|---|---|---|
| M0 | Workspace, `declab-api`, decoder list, example decoder, CLI `list`/`selftest`, GUI mode picker | ✅ 2026-10-27 |
| M1a | Shared front end and host: `declab-core`/`-io`/`-dsp`/`-engine`, DDC + resampler + worker per decoder, waterfall rows, sound out, analog decoders, CLI `run` on files and radios | ✅ 2026-10-27 |
| M1b | GUI from the mockups: source controls, spectrum/waterfall (middle-drag moves the band, scroll zooms), decoder wheel with icons, card, scope, decoder panels, services, status bar, prefs | ✅ 2026-10-27 on files; live in the GUI not yet tried (the Airspy was in use) |
| M2a | DVB-T/T2 as a plug-in: decode, card, constellation, services, now/next, `.ts` out | ✅ 2026-10-28: C23 and C30 decode to DecDVB-T's very bytes at its speed |
| M2b | TS hub, player (video, audio, subtitles), EPG, Red Button, PLP choice, HTTP/UDP out | ✅ 2026-10-28 on captures; not yet live |
| M3 | DAB/DAB+ as a plug-in: audio, DLS, slideshow, SPI; libxaac question | ✅ 2026-10-30: 11D and 12B play (DAB+ by libxaac, DAB by DecDAB's MP2), DLS shown; SPI not yet |
| M4 | DVB-S and DVB-S2/S2X TS + GSE/IP (PCAP) | ✅ 2026-10-31 in simulation only (§23): `sat` plug-in; no real satellite capture tried yet |
| M5 | DVB-RCS2/TDMA return burst analyser | DecSAT's inroute captures decode |
| M6 | Release: CI, portable exe, README | v0.1.0 |
| V1 | RTL-SDR source; DMR Tier I/II/III + packet data + LRRP (`declab-pmr`, `dmr` plug-in), weak-signal receiver | ✅ 2026-10-29 in simulation only: no real DMR capture yet. AMBE+2 vocoder (port of mbelib) plays 2026-10-29 |
| V2 | NXDN48 conventional voice (`nxdn` plug-in), SNR gate in DMR and NXDN | ✅ 2026-10-29 in simulation only; NXDN trunking (CAC), data calls and NXDN96 not yet |
| V2b | One "Digital voice" decoder detecting DMR, NXDN48 and NXDN96; NXDN96 | ✅ 2026-10-29 in simulation only; NXDN96 voice is half-rate (EHR) only |
| V3 | P25 phase 1 in Digital voice: conventional calls (HDU, LDU1/2, TDU, TDULC), control channel TSBKs, IMBE vocoder (port of mbelib) | ✅ 2026-10-29 in simulation only; phase 2 (TDMA) and encrypted voice not decoded |
| V4 | TETRA downlink in Digital voice: cell, calls, status, SDS; π/4-DQPSK coherent receiver; ACELP speech via a separate library | ✅ 2026-10-30 in simulation only; speech needs `tetra_acelp.dll` (private, not GPL); π/8-D8PSK, QAM and the uplink not decoded |
| later | DLL plug-ins; DVB-C (port of `Documents\DecSAT\dvbc_decode.py`); modulators |

## 4. Notes from M1a (2026-10-27)

- **Host shape.** The source thread never waits on a decoder when live
  (blocks dropped and counted); from a file it does, so the CLI decodes a
  file completely. Blocks are pooled `Arc<Vec<Iq>>`s: shared, not copied.
  `Host::finished()` = source ended and every block decoded.
- **Channel chain.** `Ddc` (NCO + one or two decimating FIR stages) to at
  least 2.5x the bandwidth, then `Resampler` (rational polyphase) to the
  exact rate the decoder asked for: e.g. WFM from 10 MS/s is ÷20 then
  ×96/125 to 384 kS/s. `InputSpec::sample_rate = None` with a centred,
  full-width channel passes the source's samples untouched.
- **CPU.** Live WFM from the Airspy at 10 MS/s: 15–18 % of one core for the
  decoder worker, nearly all of it the down-converter. Worth a pass later
  (a cheaper first stage, e.g. half-bands).
- **Fixed against DecSAT's analog code:** SSB/CW shifted once and never
  back (a 1 kHz tone came out at 700 Hz and both sidebands folded); now a
  Weaver demodulator with a 401-tap FIR, opposite sideband ≥ 40 dB down.
  The stereo pilot was judged on its level alone and FM hiss read above the
  threshold (stereo on noise); now judged against a 17.5 kHz guard band.
- **Not verified live:** analog audio on a real station. The aerial here is
  UHF band-pass filtered and shows no FM broadcast; the Airspy path itself
  (streaming, DDC, worker, sound device) was run live.

## 5. Notes from M1b (2026-10-27)

- **Mockups:** the interactive canvas the GUI follows is a private claude.ai
  artifact (main window + decoder wheel).
- **The view and the radio.** The view has its own centre and zoom. Scroll
  zooms about the pointer inside what the source covers. Middle-drag moves
  the view; with a radio the radio follows (a retune at most every 90 ms
  while dragging, one more on release); with a file the view is held inside
  the capture. Rows are drawn where their own frequencies fall, so a drag
  shows at once; when retuned rows arrive the held waterfall slides
  (`History::push_at`).
- **The channel stays on its station** while the band moves:
  `Host::set_channel_at` keeps the decoder's channel at an RF frequency and
  the worker recomputes the offset from each block's centre.
- **The wheel** is an egui Area over everything that takes every click, so
  nothing under it reacts. The pointer changes the slice only when it moves
  (one resting in the centre must not undo a scroll). Icons are drawn with
  the painter (no image assets). Only registered decoders appear.
- **Gestures are unit-tested** headless (`apps/declab-gui/src/sim.rs` feeds
  egui made-up mouse events): middle click opens the wheel, middle-drag pans
  by the distance dragged and opens nothing, left click tunes, scroll zooms,
  wheel point/scroll/click/cancel.
- **Decoder panels:** WFM (de-emphasis 50/75 µs/off, stereo auto/mono), NFM
  (de-emphasis, squelch), CW (note pitch), through `Command::Set`.
- **CPU** on the synthetic FM file (10 MS/s, WFM): about 40 % of one core
  for the whole app, 16 of it the decoder worker. Stopped, the window only
  redraws on input.
- `out/sim_fm_98.000MHz_10Msps.cs8` is a made-up FM band (a stereo station
  at 98.8 MHz with a 440 Hz left / 1 kHz right tone, three mono ones),
  made by `python tools/make_fm.py out/sim_fm_98.000MHz_10Msps.cs8 4`
  (numpy); the capture itself is not committed.

## 6. Live test and RDS (2026-10-27)

- **Live GUI on the Airspy** (with DecDVB-T closed): the FM band shows
  stations here after all (96.1, 97.3, 100.1 MHz among them; the earlier
  CLI probes had missed them). WFM on 96.1 MHz: constant-envelope ring,
  pilot 20-49 dB, stereo, about 17 % of a core for the decoder, 31-50 % for
  the whole app.
- **Fixed from the live run:** ~170 blocks were dropped at every start
  while the decoder's worker waited for the sound device to open; sound now
  has its own thread (`AudioOut::spawn`) and none are dropped.
- **RDS** was slow live (17 groups in 15 s, part of the RadioText, no PS).
  DecSAT's RDS used only the in-phase part of the 57 kHz subcarrier, which
  EN 62106 lets a station put in phase or in quadrature with the pilot's
  third harmonic. Now: I and Q, the BPSK phase followed from the symbols
  squared, chips decided on the derotated signal. Unit-tested with
  synthesized group 0A at 0, 90, 180 and 270 degrees (PS read every time);
  `tools/make_fm.py` puts RDS (PS "DECLAB") on its stereo station, and the
  GUI decodes it at ~10.7 groups/s. **Not yet tried live** (DecDVB-T had
  the Airspy again).
- **Decoder scopes:** `Sink::scopes` carries named scopes beside the
  channel IQ; WFM sends "RDS" (one derotated point per symbol) and an
  "RDS phase" reading. The GUI offers them above the scope
  (`--scope RDS` for screenshots).

### Live RDS check (2026-10-27, later)

- The 96.1 MHz signal that gave RDS earlier had gone. The broadcasts now
  heard, 97.3 and 101.1 MHz, are stereo (pilot 23-32 dB) but give no RDS.
  `tools/rds_look.py` on a 20 s recording (`out/fm_99.200MHz_10Msps.cs8`,
  `declab run … --record`) shows why: the 57 kHz band is at the FM noise
  floor (-65 dB beside -65 dB), the pilot 37 dB above its own. The aerial
  is UHF band-pass filtered; the stations arrive too weak for RDS.
- More Airspy gain makes it worse: at LNA 14 / mixer 12-15 / VGA 10-12 the
  pilot falls to the noise (input -9 to -16 dBFS): the tuner is overloaded,
  most likely by the UHF TV the filter passes. 12/10/8 is the best setting.
- The RDS I/Q rework is therefore verified on synthesized RDS only; it
  needs a station with RDS above the noise (a proper FM aerial).
- A 96-100 % decoder CPU reading seen once live at +3.1 MHz did not
  reproduce: both offsets decode the recording at the same speed (~6x
  real time).

## 7. Dragging, the card, 60 fps (2026-10-27)

- **Smooth band drags.** The engine's waterfall thread used to drop its
  held rows and wait a new quarter-second lead at every retune; a drag
  retunes every 90 ms, so the waterfall froze. Now a retune only drops the
  half-made row. The GUI keeps its rows about one anchor centre: a retuned
  row is shifted by whole bins as it goes in (one row's work), and only a
  move past a quarter of the span re-anchors (one full redraw). A file's
  band can be dragged until its edge reaches the middle of the view.
- **Channel by mouse.** Left-drag an edge of the bracket: bandwidth (both
  edges, centred). Left-drag inside: the channel moves (100 Hz steps, onto
  the raster on release). A move within half the bandwidth, or a new
  bandwidth, does not reset the decoder (it would mute during a drag).
- **The card** (`declab_api::Card` + `Gauge`): DecDVB-T's layout (title bar
  in the family colour with the frequency, a big headline with the margin
  beside it, a bar ticked at what is needed, label/value pairs two a row,
  labelled text). WFM's holds all of RDS: PS, PI, PTY name, TP/TA,
  music/speech, groups, groups/s, block error rate, subcarrier phase, clock
  time (4A), RadioText (A/B flag clears it), AF list, group-type counts; its
  gauge is the pilot against the 15.6 dB of full stereo. A decoder without
  a card gets one made from its measurements.
- **60 fps.** Decoders report every 1/60 s. egui's repaint timer does not
  keep the interval asked (a 16 ms request drew hundreds of frames: 65 %
  of a core idle, 133 % running); the wait is steered from the frame gaps
  seen (DecDVB-T's `pace`). 56-60 fps, whole app ~31-39 % on the FM file.

## 8. RDS for weak signals (2026-10-27)

The demodulator was rewritten (`analog_rds.rs`): the subcarrier to ±2.4 kHz
at ~12 kS/s by the shared `Ddc` (it had a single-pole filter), a biphase
matched filter, Gardner timing, BPSK phase from the squared symbols, soft
differential decisions; block sync on two good blocks in order, held until
30 of the last 50 fail (it was lost on any bad chip pair); bursts of one or
two bit errors corrected; partial groups used; PS and RadioText segments
shown only when the same value came twice.

| band SNR (±2.4 kHz) | old: groups of ~68 in 6 s | new | new BLER |
|---|---|---|---|
| +7.0 dB | 67 | 68 | 0 % |
| +1.0 dB | 0 | 68 | 0 % |
| −1.0 dB | 0 | 64 | 2.6 % |
| −2.6 dB | 0 | 50 | 17 % |
| −3.9 dB | 0 | 34 | 42 % |
| −5.1 dB | 0 | 3 | 65 % |

(`cargo test -p declab-decoders noise_sweep -- --nocapture`.) On the 20 s
recording `out/fm_99.200MHz_10Msps.cs8`, 101.1 MHz went from no groups to
~149 (PS, PI, PTY, TP/TA, RadioText filling in), symbol SNR ~5 dB, BLER
~36 % with ~270 blocks corrected; 97.3 MHz (symbol SNR 1-2 dB) still gives
none. The card shows RDS sync, RDS SNR and corrected blocks.

- **RDS carrier waterfall** (later the same day): `Sink::waterfalls` carries
  rows for a decoder's own small waterfalls (`WaterfallRow`: dB over a span
  about the decoder's 0 Hz); the host queues them, the GUI draws the one
  named like the scope shown, under it (ring texture, ~3 s). The RDS
  decoder makes 128-point Hann FFTs of the filtered subcarrier, three a
  row (~31 rows/s), cropped to ±3 kHz so the colours follow the band and
  not the filter's stopband: the two biphase sidebands about ±1 kHz and
  the null at 0 Hz show on a clean signal.

## 9. Multiplex waterfall, Es/N0, clock time (2026-10-27)

- `WaterfallRow` gained `low_hz` and `marks`. A decoder waterfall named
  like a scope is drawn small under it; any other is drawn large in the
  right-hand column, which held the services. The services (and text, when
  the card has none) now sit under the card.
- WFM sends "FM multiplex": 1024-point Hann FFTs of the demodulated
  multiplex at 384 kS/s, 12 to a row (~31 rows/s), 0-100 kHz, marked at the
  pilot (19), the stereo subcarrier (38) and RDS (57 kHz).
- RDS Es/N0 is now taken along the decision axis (A = mean |re|, σ² = mean
  re² − A², Es/N0 = A²/2σ²). The first figure (re² − im²)/im² was 3 dB
  optimistic; dividing by 2im² read 0.4 dB on air because the pilot's
  phase jitter spreads the cloud across the axis, which the scope shows
  but decisions never see. Now the recording's 101.1 MHz reads 2.0 dB at
  35 % BLER, the synthetic test 1.9 dB at 42 %.
- Clock time (4A) only from blocks that passed their check uncorrected,
  with a year 2000-2099 and an offset within ±13 h (a wrong correction had
  shown 1888-11-24 UTC−11:30).

## 10. Settings window (2026-10-27)

- The decoder-settings box under the band is gone: a **Settings** button
  in the top bar opens one window with General (volume, the running
  decoder's bandwidth) and every decoder's own panel, listed by family,
  the running one marked. The FM multiplex waterfall takes the freed half
  beside the card.
- `Panel` gained `settings()` (its settings as `Command::Set`s) and
  `restore(key, value)`. The GUI keeps a panel for every decoder, sends a
  new decoder its panel's settings when it starts, and keeps them between
  runs as `set.<id>.<key>=<value>` in prefs.txt. So a mode can be set up
  before it runs.

## 11. M2a: DVB-T/T2 (2026-10-28)

- **The receiver came over as it is.** DecDVB-T's `decdvbt-core`, `-fec`,
  `-ofdm`, `-rx`, `-ts` and `-mod` (≈40 000 lines, at DecDVB-T `5e9b249`)
  are copied under their own names rather than renamed `declab-*`: no
  churn in 40 000 lines, and a fix can be compared with DecDVB-T directly.
  Their DSP dependency is `declab-dsp` under the old name (the files are
  identical bar names). DecDVB-T's own tests run here and pass.
- **The plug-in** (`declab-decoders/src/dvbt.rs`) wraps `AutoReceiver`:
  one wheel entry, DVB-T or T2 found blind; the radio's own samples
  (`sample_rate: None`); TS to `Sink::ts` and a `TsAnalyser` for services
  and now/next; DecDVB-T's card (mode, effective SNR against the mode's
  QEF C/N, details); the equalised cells on the scope.
- **Same bytes, same speed.** C23 (DVB-T 64-QAM 2/3): 62 370 packets, byte
  for byte DecDVB-T's, 1.31 s against its 1.38 s for 4 s of signal. C30
  (T2 256-QAM 2/3 rotated): DecDVB-T's whole stream is an exact prefix of
  DecLAB's (its CLI stops before the FEC drains), no continuity jumps,
  ~56 000 packets/s against its ~58 000.
- **Two host fixes found on the way.** A decoder asking for the source's
  own rate with its channel centred now gets the samples untouched (a
  pointless full-rate filter had cost DVB-T 30 %). `Block::live` says
  whether the samples come from a radio: the T2 FEC drops a frame when
  behind only then (from a file, running flat out, it had been dropping).
- `declab run … --ts OUT.ts --card`; the host's `write_ts`.
- The decoder waterfalls are drawn a row to a physical pixel on a whole
  pixel (the RDS one shimmered: 96 rows in 64 points); the multiplex
  waterfall uses 4096-point FFTs (94 Hz bins).

## 12. M2b: everything after the transport stream (2026-10-28)

- **The TS hub is the host's**, not the DVB-T plug-in's: DecDVB-T's
  `hub.rs` is `declab_engine::hub`, and the decoder worker feeds it
  whatever `Sink::ts` a decoder makes (reset on a retune or a new
  decoder). So DVB-S/S2 TS later gets the same services, guide, outputs,
  player and Red Button. `Host::set_outputs / ts_view / epg`.
- **Copied in as they are:** `decdvbt-video` (the player, ffmpeg at run
  time), `decdvbt-audio`, `decdvbt-mheg` (Red Button), and DecDVB-T's GUI
  windows `player.rs` (with `redbutton.rs`), `epg_view.rs`, `ts_viewer.rs`,
  `services.rs`, `external.rs` (VLC). `tv.rs` is the glue (DecDVB-T's
  play/watch/VLC/outputs logic), so `main.rs` only calls it; when the
  decoder makes TS, the right-hand column is DecDVB-T's services list with
  Play / VLC / TS analyser / Guide and Outputs (record, UDP, HTTP).
- **Verified on the captures:** BBC One HD plays from C30 (1080i H.264,
  AAC, DVB subtitles found); the BBC Red Button Text app opens over it on
  `sim_rb_c30` (`DECLAB_REDBUTTON=wait,wait`); the guide shows 1438 events
  across the network; HTTP (curl: 81 211 packets in 3 s, all in sync) and
  UDP (7 packets a datagram, none bad) out.
- **PLP choice:** the DVB-T/T2 decoder sends L1-post's PLPs to its
  Settings panel; the pick goes back as `plp` (`auto` = the first data
  PLP) and is kept between runs.
- **Scopes drawn as fine points:** a texture, a physical pixel to a dot,
  brighter where dots pile up (DecDVB-T's way); DVB sends 6000 cells.
- GUI: `--play SID`, `--window ts|epg`, `--settings` opens on the running
  decoder's page. CLI: `--udp`, `--http`.
- Open: not yet tried live; the whole app reads 130-220 % of a core on
  C30 with the player or the TS analyser open (T2 FEC threads, ffmpeg's
  HD decode and the analyser's report): worth a CPU pass.

## 13. Airspy gain optimiser (2026-10-28)

- `apps/declab-gui/src/gain_opt.rs`, after DecDVB-T's and DecDAB's HackRF
  optimiser. The Airspy has three stages with the AGCs off (LNA 0-14, mixer
  0-15, VGA 0-15), so it climbs libairspy's *sensitivity* ladder (rungs
  0, 2 .. 20, 21: one knob moving all three), then up to three rounds of
  hill climbing, each stage stepped by 2 about the best. 0.8 s to settle and
  1.3 s to measure per setting; about 12 for the sweep and 6 a round.
- **Figure:** the decoder's MER while it is locked (DVB-T/T2), else the
  channel's C/N from the spectrum row (channel mean power over the 10th
  percentile of the bins outside it, the radio's outer 5 % and a guard left
  out), so analog and FM have one too. A C/N reading needs room outside the
  channel: for DVB-T's 8 MHz in the 10 MHz row there is none, and only the
  MER works.
- A setting that clips (> 0.05 %) or runs hotter than -12 dBFS is out; ties
  within 0.3 dB go to the coolest. If the best C/N of the sweep is under
  3 dB (no signal in the channel) or nothing could be measured, the old
  gains are put back. Cancelling puts them back too.
- GUI: "Optimise gain" in the gain menu (sliders locked while it
  runs); `--optimise` starts it 3 s after the radio is up and logs every
  setting to stderr.
- **One optimiser, two searches** (`Plan` in `gain_opt.rs`): the timing,
  figure, limits, past-peak test and "coolest within 0.3 dB" are DecDVB-T's
  HackRF optimiser's; the **HackRF uses its search as it is** (total gain in
  4 dB steps from 20 dB under the present one, LNA first, up to 90 dB or 24
  points, then the LNA/VGA splits 16/24/32/40 of the best total), the Airspy
  the ladder and hill climb. Both have a button in their gain menu. Not in
  DecDVB-T's: the no-signal guard (best C/N under 3 dB: nothing changes).
  The HackRF side is tested on a synthetic front end only (not plugged in).
- Tested on a synthetic front end (unit tests), then live on the Airspy
  (2026-10-28, a different site from the earlier captures: C39, C42 and C45
  are the strong muxes, C32/34/35 weaker). On C42 the default gains (LNA 12 /
  mixer 10 / VGA 8) ran at about -10 dBFS and the sweep found MER rising
  1.5 -> 30.4 dB from the lowest rung, clipping at LNA 14 / MIX 10 / VGA 8
  (MER 17), and chose **LNA 13 / MIX 8 / VGA 5: MER 30.3 dB at -20 dBFS** in
  14 settings (about 35 s), the cooler of two settings within 0.3 dB.

## 14. RTL-SDR and DMR (2026-10-29)

The user asked for RTL-SDR support and digital speech decoding (DMR Tier I,
II and III, then NXDN, P25 and TETRA), optimised for low SNR. Then, mid-task,
for LRRP on DMR.

- **RTL-SDR:** DecPagers' pure-Rust RTL2832U + R820T/R828D driver (from
  DecDAB), copied into `declab-io` behind the `rtlsdr` feature (on by default
  in both apps). It runs at 2.048 MS/s only. CLI: `--radio rtlsdr --gain
  40.2|agc --ppm N`. GUI: "RTL-SDR" in the source list, with a gain menu
  (AGC, the R820T's 29 gain steps, ppm). Not yet run with a dongle in
  DecLAB.
- **`crates/declab-pmr`**: `fsk4` (the 4FSK front end NXDN and P25 will
  share), `fec` (the codes of ETSI TS 102 361-1 annex B, with generator
  matrices copied from the spec tables), and `dmr/` (receiver, BPTC, AMBE
  frame FEC, PDUs, packet data, LRRP, simulator). Constants are cited to
  TS 102 361-1/-2/-3/-4. The AMBE interleave and Golay come from DSD/mbelib
  (ISC licence, notice kept in `ambe.rs`). The LRRP token layouts come from
  SDRTrunk (GPL-3), because LRRP has no public spec.
- **Plug-in `dmr`** (new family "Digital voice"): calls per slot (start,
  end, duration, voice BER), Tier III CSBKs (grants with LPCN and slot,
  Aloha, broadcasts, clears) and the CACH Short LC system ID. Data packets
  are reassembled, and LRRP locations are logged with latitude and
  longitude.
- **Voice (added 2026-10-29, at the user's request):**
  `declab-pmr/src/vocoder/` ports mbelib's AMBE+2 3600×2450 decoder and
  synthesiser to Rust.
  - The codebooks and window are in `ambe_tables.rs`, generated by script
    from mbelib's headers with the ISC notice kept.
  - The port keeps mbelib's order of operations (1-based harmonic loops,
    the previous frame mutated while decoding). The one difference: noise
    comes from a xorshift32, not C's `rand()`.
  - The plug-in plays one slot at a time: the first to carry voice, held
    until its call ends. It skips calls marked private (encrypted).
  - Output is 8 kHz, upsampled ×6 to 48 kHz by a polyphase low-pass, so
    the host's linear resampler has no images to pass.
  - Tests: steady voiced frames come out periodic at the codebook's pitch;
    erasure, tone and repeat (more than 3 data-bit corrections, up to 3
    repeats, then silence) follow mbelib; random frames stay finite.
  - CPU: about 1.3 % of one core while a slot speaks.
  - **Not yet done:** a frame-by-frame comparison against mbelib's own C
    code (a harness was written but running the downloaded C was not
    allowed in the session); and listening to a real call. AMBE+2 may still
    be covered by patents in some countries (mbelib's README says so).

### Weak-signal receiver: what the sweep measured

`cargo run --release -p declab-pmr --example dmr_sweep -- SEEDS OFFSET_HZ
[CUTOFFS]` runs a simulated Tier III site (a group call on TS1, CSBKs on
TS2) at falling C/N, with C/N measured in 12.5 kHz.

- **The discriminator beats the complex-product detector** (matched filter
  on z·z*[n−1], then arg) by 2–3 dB. The product version distorts at large
  phase steps. The plug-in uses the discriminator; the sweep keeps both.
- **Pre-detection low-pass at 4 kHz** (not 5.5 kHz) gains about 1 dB, but
  only with **AFC**. The receiver turns its measured offset into an NCO
  ahead of the filter, and it demodulates in 20 ms slices: run on whole host
  blocks, the AFC ran away, because each burst re-measured an offset already
  corrected.
- **Soft decoding everywhere:** ML for the slot type, EMB and TACT; Chase-II
  for the Hamming rows and the AMBE Golay words; a Pyndiah-style soft
  iteration for BPTC (196,96), worth about 0.5–1 dB on CSBKs; soft Viterbi
  for rate ¾ data.
- **Flywheel:** after a SYNC, bursts are predicted every 30 ms. Voice
  bursts B–F (no SYNC) and a voice A whose SYNC was lost are decoded on
  timing from the eye's energy.
- **Validation:** Hamming (7,4) is perfect, and Golay (20,8) and QR (16,7)
  have small covering radii, so noise passes their distance tests. A burst
  only counts if its BPTC decodes, its CRC passes, or its AMBE Golay
  corrections are few. Before this fix the receiver held "lock" on noise
  and the AFC wandered. The test `does_not_lock_on_noise` guards it.
- **SNR gate** (asked for 2026-10-29: "no false triggers on noise"). The
  level fit on a SYNC's 24 known symbols gives that SYNC's own eye SNR.
  - Measured: over 120 s of pure noise, 130 SYNC candidates turned up and
    the best fitted at 3.6 dB (median −0.2). Real SYNCs at 5 dB C/N (the
    sensitivity limit) have a median of 13 dB, 95 % above 3.2 dB.
  - So a SYNC below **6 dB** is not believed when acquiring, and a locked
    channel stops decoding when its smoothed eye falls below 0 dB (6 dB of
    margin, so a fade does not cut a call).
  - Sensitivity is unchanged within run-to-run noise. Tests: 20 s of
    noise gives no lock and no events (`does_not_lock_on_noise`), and after
    the carrier stops the receiver lets go and makes up no voice
    (`lets_go_when_the_carrier_stops`).
  - The DMR settings panel has a slider (0–20 dB); the card shows how many
    SYNCs the gate turned away.
- **Results** (discriminator, 800 Hz offset, 4 runs): voice frames (the
  protected bits) 100 % to 7 dB, 96 % at 5 dB, 91 % at 4 dB; CSBKs 98 % at
  7 dB, 88 % at 5 dB. The first version managed 41 % voice at 5 dB.
- **CPU:** about 0.4 % of one core decoding and 1.5 % hunting.

### Not yet / to check on real signals

- **CRC-32 byte order:** clause B.3.9 is ambiguous about the octet order
  of the message CRC-32. The assembler accepts either order.
- **MBC last-block CRC:** the scope of the last block's CRC is read from
  the spec text; it is untested against real traffic.
- **Rate 1 data** is not decoded.
- **Header compression:** the network number is configured in the radios,
  not sent, so compressed headers show 12.x.x.x addresses.
- **Real signals (2026-10-30, a user's 25 s Airspy recording in the UHF PMR band):**
  a MOTOTRBO Capacity Plus rest channel (CSBK opcode 62, FID 0x10), a
  Tier II repeater carrying LRRP, and a radio talking (MS-sourced voice).
  `dmr_file` prints every event of a 24 kS/s file. What it showed:
  - Every LRRP data header failed its CRC-16, with any mask, though its
    BPTC needed no correction and the packets after it passed their
    CRC-32 with addresses matching the IP ones. Such a header is now
    followed provisionally and its packet kept only on a good CRC-32
    (`Assembler::provisional`). The cause is not known.
  - The BPTC (196,96) decoder gave up on bursts with 3–4 confident errors
    (one to a row and column: an Airspy sample slip's glitch). A plain hard
    row/column pass and, failing that, a search over the failing rows'
    and columns' crossings (≤ 4 flips, the code's reach) now follow the
    soft pass. Idle bursts failing at 22 dB: 67 → 37.
  - The recording drops samples (about 20 jumps in 25 s, one of 3 ms):
    a burst due a SYNC with none within ±3 samples is now looked for
    ±300 samples (`RESYNC`, at the acquisition threshold), as TETRA does.
    The radio's voice: 381 → 432 frames.
  - LRRP packets assembled: 0 → 3 of the 7 sent (the others lost blocks
    to glitches; unconfirmed data has no retry).
  - Tried and dropped: holding the discriminator through out-of-range
    turns (glitches) cost 2–3 dB in the sweeps (it holds FM clicks too);
    a robust noise update let noise flywheel on as voice after a carrier
    went. The sweep is unchanged by what stayed (82 % voice at 4 dB; the
    91 % above predates later changes).
- **System name** in the card's title from the FIDs seen (0x10 Motorola,
  opcode 62 Capacity Plus, 0x06 Connect Plus, 0x08/0x68 Hytera), ETSI
  Tier III PDUs (with Motorola's too: Capacity Max) and LRRP (Motorola's
  protocol): "DMR · repeater · MOTOTRBO Capacity Plus".

## 15. NXDN48 (2026-10-29)

- **Sources.** The NXDN Forum's specifications sit behind a registration
  form (personal details for each download), so they were not fetched.
  Layouts follow MMDVMHost (GPL-2.0-or-later, credited): it builds and reads
  real NXDN frames for hotspots and repeaters. DSD (ISC) is used for the
  voice frames: NXDN's AMBE+2 interleave is DMR's very table.
- **Frame:** 384 bits, 80 ms. FSW 0xCDF59 (20 bits), LICH (8 bits, each the
  first bit of a ±3 dibit), SACCH (26 bits + CRC-6, K=5 rate ½,
  punctured, 12×5 interleave), then four voice frames or FACCH1 (80 bits +
  CRC-12, punctured, 16×9 interleave) in either half. Everything after the
  FSW is scrambled; the descrambler flips the LLRs.
- **`crates/declab-pmr`:**
  - `conv.rs`: the K=5 code, decoded by a soft Viterbi (punctured bits as
    LLR 0; MMDVMHost slices first).
  - `nxdn/`: receiver, frames, simulator.
  - `fsk4`: now holds the shared 4FSK modulator and noise.
- **Plug-in `nxdn`:** calls from FACCH1 set-up and release, plus the SACCH
  superframe (so a late entry still learns who is talking); RAN; voice
  through the vocoder; the same SNR-gate panel as DMR.
- **Sweep** (`cargo run --release -p declab-pmr --example nxdn_sweep -- 4
  800`, C/N in 6.25 kHz):
  - The pre-detection low-pass is best at 2 kHz (1.5 and 2.6 kHz were
    worse).
  - Voice frames (protected bits): 100 % to 8 dB, 98 % at 6 dB, 95 % at
    5 dB, 84 % at 4 dB.
  - FACCH1: 100 % to 7 dB, 96 % at 5 dB.
  - CPU: about 0.6 % of a core, decoding or hunting.
- **SNR gate (NXDN):** the FSW is only 10 symbols, so the gate's fit adds
  the 8 LICH symbols (±3, sign decided). That flatters noise a little.
  - Over 120 s of noise, the best of ~1100 candidates fitted at 7.4 dB.
    Fitting the FSW alone was worse (11.6 dB).
  - Hence an 8 dB gate. With it, 120 s of noise gave 0 events and never
    locked. Real frames near the limit fit at a median of 11–14 dB.
- **Two fixes that also went into DMR:**
  - **Hunt through a wider filter** (3 kHz NXDN, 5.5 kHz DMR) and narrow
    it on lock. Through the narrow one, an off-tune signal's first frames
    fitted badly and the gate turned them away (FACCH1 at 14 dB C/N was
    only 88 %).
  - **AFC steps shift the already-demodulated samples** by the same amount.
    A discriminator's output only offsets with frequency, so this is exact.
    Before, a frame straddling the lock-time jump decoded badly. It also
    halved the residual BER of strong signals.

## 16. One decoder for digital voice; NXDN96 (2026-10-29)

- **The user asked for one decoder that detects the mode.** The list now
  has a single "Digital voice" entry (`voice.rs`, id `voice`) instead of DMR
  and NXDN.
  - It runs the DMR, NXDN48 and NXDN96 decoders side by side on the same
    24 kS/s channel; each receiver filters to its own band.
  - Their SNR gates keep the absent modes quiet. Whichever locks drives the
    card, scope and sound, and the others keep listening.
  - Log lines are tagged with the mode. "detected X" and "signal lost" mark
    the switches.
  - Settings: one page, one gate slider per mode.
  - Tests: `detects_each_mode_in_turn` (DMR, then NXDN48, then NXDN96, with
    noise between) checks the order, each call, that nothing is detected
    falsely, and that every voice frame is spoken exactly once.
  - About 2–3 % of a core while all three hunt.
- **NXDN96** (asked for as a follow-up). SDRTrunk (`TransmissionMode`, after
  TS 1-A table 3.3-1) and DSD-FME both frame NXDN96 exactly like NXDN48:
  the same 192-symbol frame, sync, LICH, coding and scrambler, at 4800 baud
  (40 ms) with the outer symbol at 2400 Hz. The receiver and simulator take
  a `Mode`.
  - Sweep: best cut-off 4 kHz (hunting 6 kHz). Voice 94 % at 5 dB C/N in
    12.5 kHz. Two minutes of noise: 0 events; its best fit was 8.0 dB, at
    the 8 dB gate, so the CRCs behind the gate matter there.
  - The NXDN48 and NXDN96 receivers each stay silent on the other's signal
    (tested).
  - Voice is decoded as half-rate (EHR) AMBE+2, as both open decoders do.
    Full-rate (EFR) NXDN96 voice has no open decoder ("never observed",
    DSD-FME) and is not decoded.
- **Fix:** a frame accepted on the flywheel after a carrier dropped could
  pass its garbage voice on. Voice is now emitted only from frames that
  check out (a CRC passed, or the Golay words came out near).

## 17. P25 phase 1 and IMBE (2026-10-29)

- **Inside Digital voice** as a fourth mode beside DMR, NXDN48 and NXDN96.
  Frame layouts follow TIA-102.BAAA as MMDVMHost, DSD and SDRTrunk read it:
  - Sync 0x5575F5FF77FF (48 bits); NID = NAC (12) + DUID (4) as BCH(63,16)
    plus a parity bit, decoded soft by ML over the 4096 NACs (or the known
    one once locked).
  - Status symbols at raw bits 70/71 of every 72 are stripped first
    (`p25::stripped`).
  - HDU: Golay(18,6) words around RS(36,20). LDU1 link control: Hamming
    (10,6) around RS(24,12); LDU2 encryption sync: RS(24,16). TDULC:
    Golay(24,12) around RS(24,12). TSDU: rate-½ trellis (the DMR one) +
    CRC-CCITT per TSBK. RS is decoded errors-only (Berlekamp–Massey).
  - TSBKs decoded: grants, identifier updates (channel → MHz), RFSS, network
    and adjacent status; others shown by opcode.
- **IMBE (7200×4400)** ported from mbelib (ISC) as `vocoder/imbe.rs`,
  sharing the synthesiser in `vocoder/mbe.rs` with AMBE+2. The frame FEC
  (Golay(23,12) ×4, Hamming(15,11) ×3, PN over 114 bits) and DSD's
  interleave are in `p25/imbe_frame.rs`. Encrypted calls (ALGID ≠ 0x80)
  are muted and shown as encrypted.
- **Sweep** (`p25_sweep`): best cut-off 3.5 kHz (hunting 5.5 kHz). At 5 dB
  C/N in 12.5 kHz: voice 83 %, link control 100 %, TSBKs 87 %. Two minutes
  of noise: best fit 0.5 dB, 0 events with the 6 dB gate (the long sync
  barely correlates with noise). Under 1 % of a core.
- **GUI check:** `p25_make` writes a call and a control channel; the card
  shows the NAC, the system/site line, the grant in MHz, and plays the call.

## 18. TETRA (2026-10-30)

- **Spec:** ETSI EN 300 392-2 V3.8.1 (downloaded with the user's OK), the
  authority for every layout: modulation clause 5, coding clause 8, bursts
  9.4.4, MAC 21.4, MLE 18.4, CMCE 14.7.
- **Receiver** (`declab-pmr/src/tetra`), 72 kS/s, four samples a symbol:
  - Hunts for the 19-symbol synchronization training sequence with a
    normalised correlator on z[n]·z*[n−4] (magnitude, so an offset only
    turns it); locks when the fit passes the SNR gate and the BSCH passes
    its CRC. The SYNC gives TN/FN/MN and the colour code that scrambles the
    rest.
  - Locked, every 255 symbols is a slot (continuous downlink), classified
    by its training (normal 1, normal 2, synchronization, plus training 3 at
    the edges). The fit drives timing (early/late) and acquisition AFC.
  - **Coherent detection** for low SNR: the π/4 step taken out leaves QPSK;
    the carrier phase comes from the fourth power over ±20 symbols in two
    passes (the first pass's slope taken out); log-MAP over the
    differential encoding gives the LLRs. With no prior on transitions the
    BCJR collapses to a sum over neighbouring symbol pairs (exact, cheap).
    About 2 dB better than differential detection at Es/N0 6–10 dB
    (`detectors_ber`). The AFC follows the phase track's slope over the
    whole slot: the training's ~30 symbols were too noisy to ramp across a
    slot (it cost whole slots at 11 dB).
  - Locked slots are held to the gate less 6 dB (noise fits at about
    −12 dB), which moved the knee by about 2 dB.
- **Channels:** BSCH, AACH (Reed-Muller (30,14), Chase-II over a coset
  table), SCH/HD, BNCH, STCH, SCH/F (RCPC 2/3 soft Viterbi + CRC), TCH/S
  passed on as descrambled soft bits. MAC-RESOURCE, MAC-FRAG/END
  reassembly, SYSINFO, ACCESS-ASSIGN, basic-link LLC, CMCE D-SETUP,
  D-CONNECT, D-TX GRANTED/CEASED, D-RELEASE, D-STATUS, D-SDS-DATA. The
  colour-code scrambler takes e(1) as the MCC's MSB (figure 23.5 is an image
  in the text; confirmed on a real cell, below).
- **Sweep** (`tetra_sweep`, 4 seeds, 500 Hz off, C/N in 25 kHz): everything
  100 % down to 8 dB; at 6 dB SYNC 99 %, signalling 100 %, raw traffic BER
  2.4 %; at 5 dB SYNC 96 %, signalling 92 %, BER 4.4 %; at 4 dB SYNC 69 %.
  Two minutes of noise: 53 000 candidates, the best fit 0.0 dB, one passing
  the BSCH CRC by chance; with the 3 dB gate, no events. CPU 0.8 % locked,
  0.7 % hunting.
- **Real signal** (2026-10-30, a user's 10 MS/s recording in the TETRA band,
  13 carriers): two cells of one network, eye SNR 11–19 dB, SYNC, SYSINFO and
  signalling with ~1 % CRC fails; the signalling is encrypted. The
  recording had five gaps of ~8.2k samples (lost before the recorder: the
  same jump on both cells), each of which cost nine slots and a re-hunt.
  So a bad slot now looks for its training ±300 samples (a lost 8192-sample
  transfer is 59 samples at 10 MS/s, 246 at 2.4 MS/s) at the acquisition
  gate, and carries on numbered as before: 0 lock losses, 0 bad slots on
  the main carrier (was 36). No change in the sweep; CPU 0.95 % locked
  (+0.13 %, the searches in bad slots). `tetra_file` prints a 72 kS/s
  file's fit slot by slot.
- **Scope:** the symbols after the phase track and the π/4 removal, so
  four steady QPSK points (diagonals); the raw star turned with any
  residual offset on a real signal. Differential mode shows the
  derotated differential products.
- **Digital voice** now takes 72 kS/s (25 kHz): TETRA reads it as is, the
  4FSK modes after one shared decimation by 3 (9 kHz low-pass, 49 taps).
- **Speech:** ETSI EN 300 395-2's reference C code (downloaded with the
  user's OK) is ETSI-licensed, so its Rust port lives in a separate private
  crate, `..\DecTETRA-acelp`, built as `tetra_acelp.dll` and loaded at run
  time (`declab_pmr::vocoder::acelp`). Without it TETRA decodes in full and
  says "codec not installed". The portable build copies it when built.
- **Encryption:** speech is muted when the call's MAC signalling was
  encrypted, end-to-end encryption was granted, or traffic appears on a
  cell offering air-interface encryption with no clear set-up heard.
- **Nearby cells:** D-NWRK-BROADCAST (MLE type 010, table 18.2) and its
  "neighbour cell information for CA" elements (table 18.64: an O-bit,
  then a P-bit before each optional part). Each neighbour's main carrier
  uses the serving cell's band and offset unless it carries an extension.
  Checked on the real recording: the serving cell lists 10
  neighbours, among them the other cell decoded from the same
  recording, at the carrier and location area it was found on. Shown in
  the right panel; a click tunes there
  (exactly: rounding to 1 kHz would put a TETRA carrier 500 Hz off).
- **Not decoded:** π/8-D8PSK and QAM (TEDS), the uplink, direct mode,
  advanced-link LLC, MM, and MLE beyond D-NWRK-BROADCAST and the PDU type.

## 19. Heard list (2026-10-30)

The user chose mockup A of five (the "DecLAB UI mockups" canvas): two
tables in the right panel, talkgroups over radios, for every Digital
voice mode.

- `Sink::heard` carries a whole `Heard` list; `declab-decoders/src/heard.rs`
  keeps it the same way for DMR, NXDN, P25 and TETRA. A mode says when a
  call starts and ends (its own call tracking) and adds the speech it
  decodes: 60 ms a DMR burst or TETRA slot, 20 ms an AMBE+2/IMBE frame.
  Air time is counted from speech, not the clock: a file hands the decoder
  blocks of 65 536 samples (2.7 s at 24 kS/s), too coarse to time calls by.
- A call is a transmission (DMR's and P25's PTT, TETRA's floor grant).
  Groups are "TG n" or "private n"; TETRA tells them apart by the basic
  service's communication type (clause 14.8.2).
- Radios seen without talking are listed too: DMR LRRP positions, TETRA
  status and SDS senders, and, on an encrypted cell, the addresses in its
  encrypted MAC signalling (which may be groups as well as radios).
- The list clears on a retune (the decoder's reset) and follows the mode
  Digital voice has detected.
- Click a column's name to sort by it (names A–Z with numbers in order,
  counts and times largest first, last heard most recent first); click
  again to reverse. Kept per table in egui's memory.

## 20. DAB and DAB+ (2026-10-30)

- **Crates:** DecDAB's `decdab-core`, `-dsp`, `-fec`, `-fic`, `-msc`,
  `-ofdm`, `-codecs` and `-data` copied under their own names (as the
  DVB-T crates were), less DecDAB's FDK binding, its MP3 recorder and the
  OFDM probes (they read DecDAB's I/O crate). The plug-in
  (`declab-decoders/src/dab.rs`) follows DecDAB's engine: the host hands
  over 2.048 MS/s, the FIC gives the services, the chosen one's
  sub-channel goes to sound and PAD.
- **AAC by libxaac** (Apache-2.0), DecSAT's binding (`decsat-xaac-sys`,
  the `third_party/libxaac` submodule at DecSAT's commit) and wrapper.
  DecSAT feeds ADTS; for DAB+ that decoded 1024-sample frames whatever the
  frame-length flag said, so DAB+ runs libxaac's MP4 mode: the superframe's
  AudioSpecificConfig (960 frames, SBR/PS explicit) through INIT_PROCESS,
  then access units bare until it reports ready (as its test app does).
  Checked against DecDAB's FDK on Classic FM (11D, HE-AAC v2): correlation
  0.9993, 28.5 dB between the two, equal levels.
- **DAB** (MPEG Layer II): DecDAB's own decoder, unchanged.
- **The service:** the one picked in the list, else the list's first radio
  service (the one the GUI marks). The FIC fills in over a second or two;
  a service started on an early picture (seen: an SId mapped to another's
  sub-channel) restarts when its description changes.
- **Shown:** card with the ensemble, codec, sub-channel, AUs, RS and a MER
  gauge (10.5 dB for clean EEP 3-A, DecDAB's figure); the DLS as the
  service's text; slideshow images as its pictures; the constellation.
- **CLI:** `--service NAME` picks a service, `--wav PATH` writes the sound
  (the host's WAV tap; a new rate or channel count starts it over).
- **Not yet:** SPI/EPG and other packet-mode data, TII, the channel
  sounder, a Band III scan.


## 21. Multi VFO (2026-10-30)

Engine and GUI done; the CLI still runs one decoder.

- **A VFO** is one channel cut out of the radio's stream plus a decoder on it
  and everything that decoder makes. `Host` holds a list of them (at most
  `MAX_VFOS` = 8); each has its own worker thread, down-converter (`Chain`),
  `DecoderView`, panel messages, small waterfalls, `TsHub` (so its own
  services, guide and outputs), TS and WAV files, and audio output. Ids
  (`VfoId`) are never reused.
- **The source thread** hands each block to every worker (`Arc<Vec<Iq>>`,
  shared, not copied) and keeps a lost-sample count per worker. A live radio
  drops blocks for a slow VFO only; a file waits for the slowest one.
- **API:** `add_vfo(factory, hz, bw)`, `remove_vfo`, `vfo_ids`, and
  `set_vfo_channel[_at]`, `vfo_command`, `vfo_view`, `take_vfo_custom`,
  `take_vfo_waterfall_rows`, `vfo_ts_view`, `vfo_epg`, `set_vfo_outputs`,
  `write_vfo_audio`, `write_vfo_ts`, `set_vfo_gain`. The methods without
  `_vfo` act on the **primary** (first) VFO, so the CLI, the GUI and the TV
  windows are unchanged. `set_decoder(Some(f))` replaces the focused VFO in place.
- **Retuning the radio:** a decoder's `retune_by_hz` moves the radio only
  from the primary VFO; any other VFO shifts its own channel. When the
  primary is removed the next VFO takes over.
- **Sound:** every VFO plays; the OS mixes them. `set_volume` is the master,
  `set_vfo_gain` mutes or trims one. (A GUI may prefer to play only the
  selected VFO.)
- **Test:** `crates/declab-engine/tests/multi_vfo.rs` runs two WFM VFOs on
  the synthetic band (94.5 and 100.3 MHz, 600 and 300 Hz tones) and checks
  each WAV holds its own station's tone.
- **Focus:** the methods without `_vfo` act on the *focused* VFO
  (`set_focus`, the first until told); `set_decoder(Some(f))` swaps the
  focused VFO's decoder in place, keeping its position (and giving it a new
  id). Only the first VFO ever retunes the radio.
- **GUI (layout B of the mockups, with a collapsible rail):** `vfo.rs`. The
  rail down the left lists the VFOs as cards (number, decoder, frequency,
  lock and headline, CPU, level meter, speaker, remove); `«` collapses it to
  a column of numbered discs, `»` opens it (kept in prefs as `vfo_rail`). The
  App's own fields (`dec`, `chan_hz`, `view`, `scope`...) always hold the
  *selected* VFO, so the card, Settings and TV windows are unchanged; the
  others wait in `Slot`s and are swapped in on selection (`stash`/`load_slot`).
  On the band: click a channel to select it, a plain click retunes the
  selected one, ctrl+click adds a VFO (same decoder as the selected, muted),
  edges and body drag the selected channel as before; the wheel changes the
  selected VFO's decoder. Other VFOs' views are refreshed every 15 frames.
  `--vfo ID@FREQ` (repeatable) and `--rail-closed` set it up for screenshots.
- **Limits:** the last VFO cannot be removed; settings panels are shared by
  decoder type (two WFM VFOs show one panel, its values sent to whichever is
  selected); only the selected VFO is saved in prefs; the rail's "drag a row
  to reorder" from the mockup is not built; CLI `--vfo` not yet.
- **Next:** per-VFO settings and prefs, reordering, CLI `--vfo`, a shared
  first-stage decimator when VFOs are close together.

## 22. Bias tee (2026-10-31, issue 1)

- **All three radios** can power the antenna port: Airspy R2 (GPIO port 1 pin
  13, 4.5 V), HackRF One (antenna power, 3.3 V) and RTL-SDR Blog dongles
  (GPIO 0). `bias_tee: bool` in `AirspySettings` / `HackRfSettings` /
  `RtlSettings` (false by default), applied at open, and
  `set_bias_tee(bool)` on each `*Control` to switch it live.
- **Off unless asked, off at the end:** every open without the setting drives
  the pin low; closing or dropping a source drives it low again (Airspy and
  HackRF in `Drop`, RTL-SDR in `Rtl2832::close`), so antenna power never
  outlives the program. Tests: the Airspy start sequence carries a 1 only
  when asked, the RTL-SDR's GPO writes are all low by default and the last
  one after close is low, the HackRF default is off. The earlier "never on"
  tests became these.
- **GUI:** a "Bias tee" toggle beside the gain menu of a live radio, with a
  warning in its tooltip. **Not saved in the preferences:** it is off at every
  start of DecLAB (a DC supply on a DC-grounded antenna can damage it);
  `--bias-tee` on the command line (GUI and CLI) turns it on at start.
- **Not verified on hardware** (no radio with the feature was attached): the
  USB encodings and register writes follow libairspy, seify-hackrfone and
  librtlsdr and are unit-tested.

## 23. Satellite: DVB-S, S2, S2X, TS and GSE (2026-10-31, M4)

- **One plug-in, `sat`** (`declab-decoders/src/sat.rs`, family Satellite), on
  DecSAT's receiver (`..\DecDVB`, v0.10.17), copied under the original
  names: `decsat-core`, `-dsp`, `-fec` (LDPC, BCH, demappers, S2X tables),
  `-frame` (PLHEADER, PLS, scrambler, VL-SNR, super-frames), `-gse`, `-ip`,
  `-ts` (deframer, PSI/SI), `-modem` (DVB-S Viterbi + RS), `-mod` (modulator:
  dev-dependency, for the tests and `sat_make`). The new `decsat-rx` holds
  what DecSAT keeps in its engine crate: `demod` (PL demodulator), `identify`
  (symbol rate, roll-off, carrier, S2 or not), `psk` (QPSK for DVB-S), the
  super-frame stripper and the FEC stage (`fec.rs`: the engine's `FecDecoder`
  without its VFO machinery). Not brought over: DVB-RCS2, TDMA, the other
  modems, the beacon/CID decoders' own VFOs (`beacon` and `period` are in
  only because `identify` calls them).
- **Flow:** gather ~0.6 s (longer if a carrier is too slow to show two
  frames), `identify_in`; DVB-S2 verdict → PL demodulator; else a carrier of
  a known symbol rate → the PL demodulator as a probe (it finds headers at
  lower SNR than identify does), and when nothing decodes in 4 s the other
  standard (QPSK → DVB-S) is tried. Lock lost for 4 s → look again. The
  carrier's offset from the channel centre is removed by an oscillator in
  the decoder (no retune: a source-driving retune would reset the decoder).
- **FEC on a thread of its own** (`sat-fec`): LDPC, BCH, BBHEADER, then each
  input stream (ISI) to its stage: transport packets → `Sink::ts` (the first
  stream met; PSI/SI gives the services and now/next), GSE and GSE-HEM → IP
  datagrams in `Sink::ip`. A live source drops frames when the queue (64) is
  full (counted, shown); a file waits.
- **New in the host:** `Sink::ip` is counted (`DecoderView::ip_packets`,
  `ip_bytes`) and written as a raw-IP PCAP (`Host::write_pcap`; CLI
  `--pcap`; GUI "record .pcap" in an IP traffic box where the services list
  would be). The CLI got `--set KEY=VALUE` (a decoder setting as its panel
  sends it). The host's DDC now also decimates a centred channel under half
  the band for decoders that take the source's rate.
- **Settings** (panel): symbol rate (kS/s) and the S2 scrambling code. The
  symbol rate matters below the SNR identify can work at: with it set, a
  QPSK 1/4 GSE carrier decodes at 0 dB Es/N0 (needs -2.35); without it from
  about 1.5 dB.
- **Tested on simulation only** (`cargo run --release -p declab-decoders
  --example sat_make -- OUT s2ts|s2x|gse|dvbs [ES_N0] [SECONDS] [OFFSET_HZ]
  [RS]`, cf32 at 4 S/symbol; and DecSAT's `decsat tx`): 8PSK 3/4 TS, S2X
  (8PSK 25/36), QPSK GSE (546 UDP datagrams in the PCAP), DVB-S 3/4 down to
  6.5 dB, offsets of ±300 kHz, two unit tests. CPU: the demodulator is the
  cost (8PSK 3/4 at 2 MS/s on a 8 MS/s channel, 49 % of a core; the FEC
  thread ~20 %): decimating to 2-3 samples a symbol before the matched
  filter would roughly halve it.
- **Not done:** ISI selection (the first TS stream is output; every stream is
  decoded and listed), BISS descrambling and ffmpeg-less multicast-radio
  relay from DecSAT's `decsat-ts`/`decsat-ip` (the crates are here, unwired),
  DVB-S2 ACM statistics per MODCOD, the Identify card, the live GUI on a
  radio (no dish here).

## 21. Relocking after a retune (2026-10-31)

- **Report:** retuning Digital voice to another carrier sometimes never
  locked, however strong the signal.
- **Cause:** the host resets a decoder only on a jump of more than half
  its channel; a drag moves it in small steps so the sound is not muted.
  While dragged, a locked receiver's AFC follows the carrier off. The 4FSK
  receivers zero their AFC when they lose lock; TETRA's kept it, so after
  a drag onto another cell (8 kHz down, in the test) the new cell sat
  outside its reach and never decoded. Now it hunts afresh too.
- **Test:** `voice::tests::tetra_relocks_after_a_drag`; the ignored
  `relock_after_retune` prints every mode pair, with and without a reset
  and after drags (all relock in 0.1–0.8 s). `retune_probe` (declab-cli)
  does it through the real host on a recording.
- **VFO switch:** the decoder's own waterfalls (FM's multiplex, the small
  ones under the scopes) were cleared on every switch; each VFO now keeps
  its own, and they come back as they were.

## 22. DMR that never locked from some starts (2026-10-31)

- **Report:** a strong DMR carrier (a MOTOTRBO Capacity Plus rest
  channel, 23 dB eye) sat "searching" in the GUI.
- **Cause:** the hunt looked for a SYNC from 23 symbols into the stream,
  but decoding a burst needs the 89 symbols before its SYNC's end (the
  burst's first half and the CACH). A SYNC found in between became a
  candidate that waited forever for samples from before the start, and
  the hunt never ran again. A SYNC comes every 30 ms and the window was
  ~14 ms wide, so about half of all starts (a VFO opened, every retune
  that resets the decoder) never locked. Live it looked random: eight
  runs, two stuck; from a recording, windows starting at 25 or 26 s never
  locked while ones at 24 or 27 s did.
- **Fix:** the hunt starts where a burst is whole, and a candidate whose
  burst start is gone is dropped. NXDN and P25 had a 2-sample version of
  the same window: fixed alike. Test `locks_from_any_start` (36 starts
  across a burst, each locks within 0.5 s). Live: eight starts, eight
  locks.
- **Not a fault:** a Capacity Plus rest channel moves to another
  repeater when a call starts, so its carrier can stop (seen at 31 s on
  the test recording); the decoder then searches until it is back.
- **Tools:** `vfo_probe` (two VFOs on a recording through the real host);
  the CLI now reports blocks a slow decoder dropped.

## 23. Finer spectrum when zoomed in (2026-10-31)

- The spectrum's FFT follows the zoom: about a bin a pixel across the
  view, from 4096 up to 16384 (or the GPU's widest texture, the waterfall
  being one): at 10 MS/s 2.4 kHz bins when zoomed out, 610 Hz zoomed in;
  146 Hz on an RTL-SDR's 2.4 MS/s. It changes once the zoom has held for
  0.25 s (`follow_zoom`), through `Host::set_fft` to the waterfall thread
  (`Msg::Fft`); fewer windows are averaged as it grows, so a row costs
  about the same. Measured: no change in the app's CPU.
- The waterfall keeps its rows when the size changes: rescaled (nearest
  bin when growing, the peak when shrinking, so a one-bin carrier keeps
  its level) instead of starting again (`rows_survive_an_fft_size_change`).
- Finer still would need a zoom FFT of the view's band alone (the history
  then kept in view coordinates); the full band at 16384 bins costs about
  40 MB of history and texture.


## 24. Digital voice data tabs; AM/FM squelch (2026-11-02)

**Data tabs.** The right-hand box of Digital voice has four tabs: *Heard*
(the talkgroup and radio tables, §19), *Hex* (every data message whole: a
heading, then offset, 16 bytes in hex, ASCII), *LRRP* (location messages
as a table, newest first; a click on a position copies it) and *IP* (the
datagrams, newest first, with the PCAP recorder; the picked one, or the
newest, is taken apart: IPv4 and UDP headers, the checksum checked, the
payload in hex). `--tab heard|hex|lrrp|ip` opens on one (screenshots).

The API gained `Sink::packets` (`DataPacket`: time, channel, from, to,
kind, CRC, bytes) and `Sink::locations` (`Location`, from LRRP). DMR sends
every reassembled data packet, its datagram to `Sink::ip` (so `--pcap` and
the GUI's recorder take DMR data too) and each LRRP message; TETRA sends
its SDS. Digital voice forwards them from every mode, the channel prefixed
with the mode. Header-compressed packets (SAP 3, TS 102 361-3 §7.2) are
rebuilt as whole IPv4/UDP datagrams (`lrrp::datagram`) with a correct
header checksum. The host keeps the last 200 messages and locations and
the last 64 datagrams for the GUI (GSE can bring thousands a second: only
those are copied). The hex dump is laid out when a message arrives and
drawn with `show_rows`, so a long dump costs only the rows on screen.

**Squelch.** AM, NFM and WFM have a squelch with three methods
(`analog_squelch.rs`), each with its own threshold:

| method | measures | default | notes |
|---|---|---|---|
| Level | channel power, dBFS | −60 | moves with the gain |
| SNR | C/N by the M2M4 moments estimator | 6 dB | gain-free; AM's modulation reads low (30 % mod tops out near 10 dB) |
| Noise | quieting of the detector above the audio (FM discriminator; AM envelope over its mean), dB below noise alone | 8 dB | the classic FM noise squelch; its 0 dB is measured at each mode's default bandwidth |

Smoothed at 10 Hz, decided every 64 samples with 2 dB hysteresis, the audio
faded over 5 ms. Measured on simulated carriers (`squelch_sweep`), every
method stays shut on noise; SNR 6 dB and Noise 8 dB open at about 5–8 dB
C/N. Cost on broadcast FM: 5.7 % of a core without, 6.1 (level, SNR) to
6.5 % (noise) with (`squelch_cost`). Settings: `squelch` = `off` | `level`
| `snr` | `noise` (a number, as older prefs hold, means level at that dBFS),
`squelch_level`, `squelch_snr`, `squelch_noise`.

## 25. NXDN: frames the flywheel guessed (2026-11-03)

On the air, an NXDN48 repeater with no traffic sent a beacon every 5 s:
two frames (LICH 0xc2, FACCH1 in both halves: IDLE, then TX_REL from the repeater's own
unit ID). After them the flywheel kept decoding frames where the next ones
would have been. Half of those noise "frames" pass the LICH's single
parity bit. Those reading as control channel (RCCH) were believed outright
(nothing else checks a CAC yet), so the card said "control channel"; the
rest went to FACCH1/SACCH, whose short CRCs (SACCH: 6 bits) let the odd one
through as a message of unknown type. Now an RCCH frame counts only where
the FSW was found, and a guessed frame's messages must be of a known type.
A message of unknown type shows its type and bytes rather than "unknown".
`nxdn_file` prints every frame (LICH, pass or not, eye SNR; `bits` adds the
descrambled bits: a real frame starts with the FSW, cdf59).

## 26. NXDN voice from a real repeater (2026-11-03)

A conventional NXDN48 repeater on the composite channel type sent its
voice in frames with LICH 0xef (four voice frames, SACCH superframe), call
set-up in 0xc6 (FACCH1, then voice) and idle in 0xc2. All 0xef frames were
thrown away: the LICH parity followed MMDVMHost's `getParity` ("set for 0x8x
and 0xBx"), which is right only for the conventional values it sends. The
parity is even parity over bits 7–4 (all 216 real frames with their FSW
agree, and so do MMDVMHost's values). With it, 30 minutes of the repeater
(10 calls, six radios, three talkgroups) decode at 0.00–0.15 % voice BER;
the AMBE+2 bit order shared with DMR was right (0 Golay corrections).

Still open: the SACCH never passes its CRC on this repeater. Neither the
interleave read the other way, any of the six puncture phases, a zero CRC
start, nor the frames left scrambled decode it, and an idle frame's SACCH
matches no IDLE message under those variants (16 of 60 bits off at best).
So the RAN is not shown. Messages 0x00 (before VCALL) and 0x11 (after
TX_REL) carry the call's addresses; they are shown with them, unnamed.

`declab run … --record-channel OUT.cf32` writes the decoder's channel as it
is fed (cf32 at its rate, 72 kS/s for Digital voice: 0.6 MB/s against the
radio's 20), for long recordings of one channel. The CLI's log printing
stopped once the view's 300-line log was full (it counted the deque's
length); `DecoderView::log_total` now counts every line.

## 27. The last call stays on the card (2026-11-03)

A repeater that only bursts (a call of a second or two, then its beacon)
had its call on the card for as long as it lasted. Now DMR and NXDN keep
the last call that ended (`heard::LastCall`): between calls the headline is
"last: TG 77 ← 1234" and a "last call" line gives its age, length and
BER; the channel's state (carrier, call, control channel) is a line of its
own. "Control channel" means a control frame in the last 2 s, not one ever.
When the signal drops, Digital voice keeps the last mode's card, titled
"last heard m:ss ago", instead of a bare "searching". A retune clears both.
A headline too long for the card shrinks to fit beside its margin.

## 28. Meshtastic over LoRa (2026-11-03)

Two decoders in a new family, **Data** (pagers moved there too; it shows
the data tabs, its location tab named *Positions*): `meshtastic` below
1 GHz and `meshtastic-2g4` for the SX128x at 2.4 GHz (LORA_24). The host's
channel is fixed per decoder, so each takes the widest LoRa bandwidth of
its band: 1 MS/s with a 500 kHz channel, and 3.25 MS/s with 1625 kHz. The
LoRa bandwidth is a setting (`bandwidth`, panel or `--set bandwidth=250k`;
LongFast's 250 kHz, 812.5 kHz at 2.4 GHz, by default); every LoRa
bandwidth divides those rates into twice itself. The plug-in low-passes
(flat to 0.58 BW, stopped by 0.75 BW) and decimates to 2 × BW.

**LoRa** (`crates/declab-lora`). LoRa has no public specification; the
coding chain (whitening, header and its checksum, payload CRC, Hamming
4/5–4/8, diagonal interleaver, Gray map, reduced rate for the header and
LDRO) is the one gr-lora_sdr implements (Tapparel et al., EPFL, GPL-3.0),
each function naming the block it follows. The receiver is our own:

- The spreading factor is found by brute force: for each SF 7–12, windows
  of 2^SF chips (every other sample) dechirped and FFT'd; four windows in a
  row peaking in one bin start a frame. The coding rate is in each frame's
  header; LDRO follows the 16 ms rule, the other setting tried on a CRC
  failure (an SX128x has none).
- Carrier offset: the fraction from the preamble peak's turn window to
  window, the integer part with the timing from the preamble's and a
  downchirp's bins (u + d)/2; the downchirps' start where two whole ones
  line up best.
- Timing to the fraction of a chip (the preamble's peak between bins on
  windows moved onto the symbol grid), and each symbol sampled at its exact
  chip times by an 8-tap windowed sinc from the 2× stream: one sample a
  chip taken a fraction r of a chip off turns the phase at the chirp's wrap
  by 2πr (−3 dB at a quarter chip). Reduced-rate symbols are divided by 4
  with rounding (gr-lora_sdr truncates), so a bin off by one is no error.
- Clock offset: with the carrier frequency known (the block's tuning), the
  symbols drift by N·c_Hz/f_c chips each, followed from where the timing was
  measured. A 255-byte SF12 frame with 12 ppm between the ends slides 20
  chips unfollowed.

**Meshtastic** (`declab-lora/src/meshtastic.rs`), after the firmware: the
16-byte header (`PacketHeader`), AES-CTR (nonce: packet id 64-bit LE, sender
32-bit LE, a big-endian block counter; `CryptoEngine`), the channel hash
(XOR of name and key; LongFast with the default key is 8), the presets
(`modemPresetToParams`). Each packet is tried unencrypted and with the
default key and its short-PSK siblings (`AQ==` and up, 1–10); a `Data` that
parses (portnum first, a known port) is the proof. The protobufs are the
official ones (the `meshtastic` crate, GPL-3.0, without its default
features: only the generated types). PKI direct messages and channels on
their own keys stay encrypted. Text, position, node info, telemetry,
traceroute, neighbour info and routing are summarised; positions go to
`Sink::locations`, channels and nodes to the heard list (`Heard::data`
names the columns: channel, modem, packets, nodes, air time), every frame
to the log and the hex tab (header and decrypted `Data`).

Measured in simulation (`mesh_sweep`, 250 kHz, 24-byte text, 20 frames a
point, the SNR in the LoRa band, through the plug-in): at Semtech's
demodulation floor (SX1276 table 13) SF8–SF12 decode 12–18 of 20 and all 20
from 1.5 dB above; SF7 is about 1.5 dB short of its floor. CPU: 3.4–4.0 % of
a core at 1 MS/s (9800X3D), the channel filter and the six SF detectors
most of it. Simulated captures: `mesh_make`.

On the air (RTL-SDR Blog V4, EU_868 LongFast at 869.525 MHz, 5 minutes):
eight packets from three nodes, SNR 5–20 dB, carrier offsets −10.4 to
+2.7 kHz, no CRC or header errors. A node's NODEINFO on LongFast was
decrypted with the default key and its channel named from the hash; the
other two nodes' packets, on a channel with its own key (hash 0x8f), stayed
encrypted. A newer firmware's (3.0) burst on the same frequency, 100 ms
long, is not LoRa as this decodes it. The decoder worker (the host's
resampling from 2.048 MS/s included) took 8 % of a core. The official
crate's protobufs (0.1.9) lack the newest hardware models: those show as a
number.
