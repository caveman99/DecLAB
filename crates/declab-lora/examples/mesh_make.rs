//! A simulated Meshtastic capture: a node announcing itself, a text
//! message, a position and telemetry on LongFast, a MediumFast packet on the
//! same channel (another spreading factor), and a packet on a key the
//! decoder does not know; 1 MS/s complex f32, a 4.2 kHz carrier offset and
//! noise.
//!
//! ```text
//! cargo run --release -p declab-lora --example mesh_make -- out/sim_meshtastic.cf32 [SNR_DB]
//! cargo run --release -p declab-gui -- out/sim_meshtastic.cf32 --rate 1M --decoder meshtastic --screenshot out/x.png --after 12
//! ```

use std::io::Write;

use declab_lora::Iq;
use declab_lora::Message;
use declab_lora::meshtastic::{self as mesh, DEFAULT_PSK, Header, channel_hash};
use declab_lora::protobufs::{self, PortNum, telemetry};
use declab_lora::tx::{self, Noise, TxConfig};

const RATE: f64 = 1e6;

fn packet(
    from: u32,
    id: u32,
    port: PortNum,
    payload: Vec<u8>,
    key: &[u8],
    channel: &str,
) -> Vec<u8> {
    let h = Header {
        to: mesh::BROADCAST,
        from,
        id,
        hop_limit: 3,
        hop_start: 3,
        channel: channel_hash(channel, key),
        ..Header::default()
    };
    let d = protobufs::Data {
        portnum: port as i32,
        payload,
        ..Default::default()
    };
    mesh::encode(&h, &d, key)
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args
        .first()
        .map_or("out/sim_meshtastic.cf32", String::as_str);
    let snr_db: f64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5.0);

    let node = 0xd1ab_0001;
    let user = protobufs::User {
        id: mesh::node_id(node),
        long_name: "DecLAB simulated node".into(),
        short_name: "DLAB".into(),
        hw_model: protobufs::HardwareModel::HeltecV3 as i32,
        ..Default::default()
    };
    // Null Island, so no one's real position.
    let pos = protobufs::Position {
        latitude_i: Some(12_345),
        longitude_i: Some(-67_890),
        altitude: Some(3),
        sats_in_view: 9,
        ..Default::default()
    };
    let tele = protobufs::Telemetry {
        variant: Some(telemetry::Variant::DeviceMetrics(
            protobufs::DeviceMetrics {
                battery_level: Some(87),
                voltage: Some(4.05),
                channel_utilization: Some(3.2),
                air_util_tx: Some(0.4),
                uptime_seconds: Some(3600),
            },
        )),
        ..Default::default()
    };
    let long_fast = TxConfig::meshtastic(11, 250e3, 1);
    let frames = [
        (
            long_fast,
            packet(
                node,
                1,
                PortNum::NodeinfoApp,
                user.encode_to_vec(),
                &DEFAULT_PSK,
                "LongFast",
            ),
        ),
        (
            long_fast,
            packet(
                node,
                2,
                PortNum::TextMessageApp,
                b"Hello from DecLAB".to_vec(),
                &DEFAULT_PSK,
                "LongFast",
            ),
        ),
        (
            long_fast,
            packet(
                node,
                3,
                PortNum::PositionApp,
                pos.encode_to_vec(),
                &DEFAULT_PSK,
                "LongFast",
            ),
        ),
        (
            long_fast,
            packet(
                node,
                4,
                PortNum::TelemetryApp,
                tele.encode_to_vec(),
                &DEFAULT_PSK,
                "LongFast",
            ),
        ),
        (
            TxConfig::meshtastic(9, 250e3, 1),
            packet(
                0xd1ab_0002,
                5,
                PortNum::TextMessageApp,
                b"MediumFast, same channel".to_vec(),
                &DEFAULT_PSK,
                "MediumFast",
            ),
        ),
        (
            long_fast,
            packet(
                0xd1ab_0003,
                6,
                PortNum::TextMessageApp,
                b"on a private key".to_vec(),
                &[0x5a; 16],
                "Private",
            ),
        ),
    ];

    let mut x = vec![Iq::new(0.0, 0.0); 250_000];
    for (cfg, p) in &frames {
        x.extend(tx::modulate(p, cfg, RATE, 4_200.0, 0.0));
        x.extend(vec![Iq::new(0.0, 0.0); 700_000]);
    }
    // A tenth of full scale, as a real channel sits.
    for s in &mut x {
        *s *= 0.1;
    }
    // The noise is sized for a unit signal: 20 dB down with it.
    Noise::new(1).add(&mut x, snr_db + 20.0, 250e3, RATE);

    let mut w = std::io::BufWriter::new(std::fs::File::create(out)?);
    for s in &x {
        w.write_all(&s.re.to_le_bytes())?;
        w.write_all(&s.im.to_le_bytes())?;
    }
    w.flush()?;
    println!(
        "{out}: {:.1} s at 1 MS/s, {} packets, SNR {snr_db} dB in 250 kHz",
        x.len() as f64 / RATE,
        frames.len()
    );
    Ok(())
}
