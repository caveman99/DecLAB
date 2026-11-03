// made by CasualArclamp
//! Every decoder DecLAB knows about.
//!
//! To add one: put it in a file of its own in this folder (copy `level.rs`),
//! add `mod yourfile;` below and its factory to [`ALL`]. That is the whole
//! registration.

use declab_api::DecoderFactory;

mod analog;
mod analog_rds;
mod analog_squelch;
mod dab;
mod dmr;
mod dvbt;
#[cfg(feature = "ui")]
mod gate_panel;
mod heard;
mod level;
mod meshtastic;
mod nxdn;
mod p25;
mod pagers;
mod sat;
mod tetra;
mod voice;

use analog::Mode;

/// The mode picker's list, in the order it shows them within a family.
pub static ALL: &[&dyn DecoderFactory] = &[
    &dvbt::Factory,
    &dab::Factory,
    &sat::Factory,
    &analog::Factory(Mode::Wfm),
    &analog::Factory(Mode::Nfm),
    &analog::Factory(Mode::Am),
    &analog::Factory(Mode::Usb),
    &analog::Factory(Mode::Lsb),
    &analog::Factory(Mode::Dsb),
    &analog::Factory(Mode::Cw),
    &voice::Factory,
    &pagers::Factory,
    &meshtastic::Factory(meshtastic::Band::SubGhz),
    &meshtastic::Factory(meshtastic::Band::Wide),
    &level::Factory,
];

/// Look a decoder up by its [`DecoderInfo::id`](declab_api::DecoderInfo::id).
pub fn find(id: &str) -> Option<&'static dyn DecoderFactory> {
    ALL.iter().copied().find(|f| f.info().id == id)
}
