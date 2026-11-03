// made by CasualArclamp
//! The talkgroups and radios heard on a digital voice signal, kept by each
//! voice mode the same way and sent as a [`Heard`] list.
//!
//! A mode says when a call starts ([`HeardList::start`]) and ends
//! ([`HeardList::end`]) from its own call tracking, and adds the speech it
//! decodes ([`HeardList::talk`]). Air time is counted from the speech, not
//! from the clock: a file hands the decoder blocks of a second or more, so
//! stream time is too coarse to time a call by.

use declab_api::{Heard, HeardGroup, HeardRadio, NearbyCell, Sink};

/// Speech with no call open, or this long after the last activity, starts a
/// call of its own (its start was missed), s.
const STALE_S: f64 = 5.0;
/// The list is sent this often, s (ages and the live mark move on).
const SEND_S: f64 = 1.0;
/// Entries kept of each kind (oldest dropped).
const MAX: usize = 500;

struct Group {
    name: String,
    slot: String,
    calls: u32,
    radios: Vec<String>,
    air_s: f64,
    last_s: f64,
    /// A call is open on it, from this radio (if known).
    open: Option<Option<String>>,
    encrypted: bool,
}

struct Radio {
    id: String,
    group: String,
    calls: u32,
    talk_s: f64,
    position_s: Option<f64>,
    last_s: f64,
    /// Talking now, on this group.
    open: Option<String>,
    talked: bool,
    encrypted: bool,
}

/// The last call that ended, kept on the card between calls: a repeater
/// that only bursts (a call, then its beacon) would otherwise show the call
/// for as long as it lasts and then "carrier".
#[derive(Debug, Clone)]
pub(crate) struct LastCall {
    /// The call in a few characters: "TG 2350 ← 2341234".
    pub what: String,
    pub end_s: f64,
    pub len_s: f64,
    pub ber: f64,
}

impl LastCall {
    /// "last: TG 2350 ← 2341234".
    pub fn headline(&self) -> String {
        format!("last: {}", self.what)
    }

    /// "0:12 ago, 2.9 s, BER 0.03 %".
    pub fn detail(&self, now: f64) -> String {
        format!("{} ago, {:.1} s, BER {:.2} %", clock(now - self.end_s), self.len_s, self.ber)
    }
}

/// Seconds as m:ss (h:mm:ss past an hour).
pub(crate) fn clock(s: f64) -> String {
    let s = s.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

#[derive(Default)]
pub(crate) struct HeardList {
    /// A data network's list (see [`Heard::data`]).
    data: bool,
    groups: Vec<Group>,
    radios: Vec<Radio>,
    cells: Vec<NearbyCell>,
    since_s: Option<f64>,
    sent_s: Option<f64>,
    changed: bool,
}

impl HeardList {
    /// The channels and nodes of a data network (see [`Heard::data`]).
    pub fn for_data() -> Self {
        HeardList {
            data: true,
            ..HeardList::default()
        }
    }

    /// A call (a transmission) starts on `group` from `radio`, if known.
    pub fn start(&mut self, now: f64, group: &str, slot: &str, radio: Option<&str>, encrypted: bool) {
        self.end(group);
        let g = self.group(now, group);
        g.calls += 1;
        g.slot = slot.into();
        g.encrypted = encrypted;
        g.open = Some(radio.map(Into::into));
        let Some(radio) = radio else { return };
        if !g.radios.iter().any(|r| r == radio) {
            g.radios.push(radio.into());
        }
        let r = self.radio(now, radio);
        r.calls += 1;
        r.group = group.into();
        r.open = Some(group.into());
        r.talked = true;
        r.encrypted = encrypted;
    }

    /// `speech_s` seconds of speech on `group` from `radio`. Starts a call
    /// when none is open from that radio (its start was missed).
    pub fn talk(
        &mut self,
        now: f64,
        group: &str,
        slot: &str,
        radio: Option<&str>,
        encrypted: bool,
        speech_s: f64,
    ) {
        let open = self.groups.iter().find(|g| g.name == group).is_some_and(|g| {
            g.open.as_ref().is_some_and(|r| r.as_deref() == radio) && now - g.last_s < STALE_S
        });
        if !open {
            self.start(now, group, slot, radio, encrypted);
        }
        let g = self.group(now, group);
        g.air_s += speech_s;
        g.encrypted = encrypted;
        if let Some(radio) = radio {
            let r = self.radio(now, radio);
            r.talk_s += speech_s;
            r.encrypted = encrypted;
        }
    }

    /// The call on `group` has ended.
    pub fn end(&mut self, group: &str) {
        let Some(g) = self.groups.iter_mut().find(|g| g.name == group) else {
            return;
        };
        if let Some(Some(radio)) = g.open.take()
            && let Some(r) = self.radios.iter_mut().find(|r| r.id == radio)
        {
            r.open = None;
        }
        self.changed = true;
    }

    /// A radio's position report.
    pub fn position(&mut self, now: f64, radio: &str) {
        self.radio(now, radio).position_s = Some(now);
    }

    /// A radio seen in signalling only (`how`: "status", "SDS"); its group
    /// column says how, until it talks.
    pub fn seen(&mut self, now: f64, radio: &str, how: &str) {
        let r = self.radio(now, radio);
        if !r.talked {
            r.group = how.into();
        }
    }

    /// The neighbouring cells, as the network last announced them.
    pub fn set_cells(&mut self, cells: Vec<NearbyCell>) {
        if cells != self.cells {
            self.cells = cells;
            self.changed = true;
        }
    }

    fn group(&mut self, now: f64, name: &str) -> &mut Group {
        self.since_s.get_or_insert(now);
        self.changed = true;
        let i = match self.groups.iter().position(|g| g.name == name) {
            Some(i) => i,
            None => {
                self.groups.push(Group {
                    name: name.into(),
                    slot: String::new(),
                    calls: 0,
                    radios: Vec::new(),
                    air_s: 0.0,
                    last_s: now,
                    open: None,
                    encrypted: false,
                });
                self.groups.len() - 1
            }
        };
        let g = &mut self.groups[i];
        g.last_s = now;
        g
    }

    fn radio(&mut self, now: f64, id: &str) -> &mut Radio {
        self.since_s.get_or_insert(now);
        self.changed = true;
        let i = match self.radios.iter().position(|r| r.id == id) {
            Some(i) => i,
            None => {
                self.radios.push(Radio {
                    id: id.into(),
                    group: String::new(),
                    calls: 0,
                    talk_s: 0.0,
                    position_s: None,
                    last_s: now,
                    open: None,
                    talked: false,
                    encrypted: false,
                });
                self.radios.len() - 1
            }
        };
        let r = &mut self.radios[i];
        r.last_s = now;
        r
    }

    /// Send the list when it changed (at most 4 times a second) or a second
    /// has passed.
    pub fn report(&mut self, now: f64, out: &mut Sink) {
        let since_sent = self.sent_s.map_or(f64::INFINITY, |t| now - t);
        if !(since_sent >= SEND_S || since_sent < 0.0 || (self.changed && since_sent >= 0.25)) {
            return;
        }
        self.sent_s = Some(now);
        self.changed = false;
        // Rust note: `sort_by` with `total_cmp` orders f64s (which are not
        // `Ord` because of NaN); reversed for the most recent first.
        self.groups.sort_by(|a, b| b.last_s.total_cmp(&a.last_s));
        self.radios.sort_by(|a, b| b.last_s.total_cmp(&a.last_s));
        self.groups.truncate(MAX);
        self.radios.truncate(MAX);
        let live = |last: f64, open: bool| open && now - last < STALE_S;
        out.heard = Some(Heard {
            now_s: now,
            since_s: self.since_s.unwrap_or(now),
            groups: self
                .groups
                .iter()
                .map(|g| HeardGroup {
                    name: g.name.clone(),
                    slot: g.slot.clone(),
                    calls: g.calls,
                    radios: g.radios.len() as u32,
                    air_s: g.air_s,
                    last_s: g.last_s,
                    live: live(g.last_s, g.open.is_some()),
                    encrypted: g.encrypted,
                })
                .collect(),
            radios: self
                .radios
                .iter()
                .map(|r| HeardRadio {
                    id: r.id.clone(),
                    group: r.group.clone(),
                    calls: r.calls,
                    talk_s: r.talk_s,
                    position_s: r.position_s,
                    last_s: r.last_s,
                    live: live(r.last_s, r.open.is_some()),
                    encrypted: r.encrypted,
                })
                .collect(),
            cells: self.cells.clone(),
            data: self.data,
        });
    }

    pub fn clear(&mut self) {
        *self = HeardList {
            data: self.data,
            ..HeardList::default()
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_calls_and_time() {
        let mut h = HeardList::default();
        let mut out = Sink::default();
        // 2345012 talks on TG 9 (its speech arriving in two batches, as a
        // file's blocks bring it), 2341107 answers with its start missed,
        // then 2345012 again.
        h.start(0.0, "TG 9", "TS1", Some("2345012"), false);
        for _ in 0..25 {
            h.talk(0.0, "TG 9", "TS1", Some("2345012"), false, 0.06);
        }
        for _ in 0..25 {
            h.talk(2.7, "TG 9", "TS1", Some("2345012"), false, 0.06);
        }
        h.end("TG 9");
        for _ in 0..10 {
            h.talk(4.0, "TG 9", "TS1", Some("2341107"), false, 0.06);
        }
        h.end("TG 9");
        h.start(20.0, "TG 9", "TS1", Some("2345012"), false);
        h.talk(20.0, "TG 9", "TS1", Some("2345012"), false, 0.06);
        h.position(21.0, "2347788");
        h.report(21.0, &mut out);
        let l = out.heard.expect("sent");
        assert_eq!(l.groups.len(), 1);
        let g = &l.groups[0];
        assert_eq!((g.calls, g.radios), (3, 2));
        assert!((g.air_s - 3.66).abs() < 1e-9, "{}", g.air_s);
        assert!(g.live);
        let r = |id: &str| l.radios.iter().find(|r| r.id == id).expect(id);
        assert_eq!(r("2345012").calls, 2);
        assert!((r("2345012").talk_s - 3.06).abs() < 1e-9);
        assert!(!r("2341107").live);
        assert_eq!(r("2347788").calls, 0);
        assert_eq!(r("2347788").position_s, Some(21.0));
        // Most recent first.
        assert_eq!(l.radios[0].id, "2347788");
    }
}
