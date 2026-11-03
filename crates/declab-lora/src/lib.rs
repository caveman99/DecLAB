//! LoRa and Meshtastic for DecLAB: a LoRa receiver that finds the spreading
//! factor by itself and reads the coding rate from each frame's header
//! ([`rx`]), LoRa's coding chain ([`coding`]), a modulator for simulations
//! ([`tx`]), and Meshtastic's packets on top ([`meshtastic`]).
//!
//! The plug-in that wraps this is `declab-decoders/src/meshtastic.rs`.

pub mod coding;
pub mod meshtastic;
pub mod rx;
pub mod tx;

/// The official Meshtastic protobufs (the `meshtastic` crate), and prost's
/// `Message` that encodes and decodes them.
pub use ::meshtastic::{Message, protobufs};
pub use num_complex::Complex32 as Iq;
