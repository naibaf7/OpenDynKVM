//! L2CAP on a BR/EDR link (Core 5.4, Vol 3, Part A), as much as a HID host needs: basic frames, and the signaling
//! channel's commands for two channels (HID control, PSM 0x11, and interrupt, 0x13) in basic mode, opened by either
//! side: by the host to a pad it pairs, by the pad when it reconnects; and SDP's (PSM 1), which a pad that reconnects
//! opens first (`crate::sdp`). A channel is open once both sides' configuration
//! requests were accepted; nothing but the MTU is ever offered, and whatever the other side asks for is accepted.

use heapless::Vec;

/// The signaling channel's CID on ACL-U.
pub const SIGNALING: u16 = 0x0001;
pub const PSM_SDP: u16 = 0x0001;
pub const PSM_HID_CONTROL: u16 = 0x0011;
pub const PSM_HID_INTERRUPT: u16 = 0x0013;
/// The MTU offered: L2CAP's default, more than any pad's report.
pub const MTU: u16 = 672;
/// The first CID handed out (the dynamic range's start).
const FIRST_CID: u16 = 0x0040;

mod op {
    pub const COMMAND_REJECT: u8 = 0x01;
    pub const CONNECTION_REQUEST: u8 = 0x02;
    pub const CONNECTION_RESPONSE: u8 = 0x03;
    pub const CONFIGURATION_REQUEST: u8 = 0x04;
    pub const CONFIGURATION_RESPONSE: u8 = 0x05;
    pub const DISCONNECTION_REQUEST: u8 = 0x06;
    pub const DISCONNECTION_RESPONSE: u8 = 0x07;
    pub const ECHO_REQUEST: u8 = 0x08;
    pub const ECHO_RESPONSE: u8 = 0x09;
    pub const INFORMATION_REQUEST: u8 = 0x0A;
    pub const INFORMATION_RESPONSE: u8 = 0x0B;
}

/// A basic frame's channel and payload, from a whole L2CAP PDU (header included).
pub fn frame(pdu: &[u8]) -> Option<(u16, &[u8])> {
    let len = u16::from_le_bytes(pdu.get(..2)?.try_into().ok()?) as usize;
    let cid = u16::from_le_bytes(pdu.get(2..4)?.try_into().ok()?);
    Some((cid, pdu.get(4..4 + len)?))
}

/// Writes a basic frame (header and `payload`) to the start of `out`; its length, or None if it doesn't fit.
pub fn write_frame(cid: u16, payload: &[u8], out: &mut [u8]) -> Option<usize> {
    let n = 4 + payload.len();
    let o = out.get_mut(..n)?;
    o[..2].copy_from_slice(&(payload.len() as u16).to_le_bytes());
    o[2..4].copy_from_slice(&cid.to_le_bytes());
    o[4..].copy_from_slice(payload);
    Some(n)
}

/// One signaling command, ready to go as a C-frame's payload.
pub type Command = Vec<u8, 24>;
/// The commands to send after something happened.
pub type Commands = Vec<Command, 4>;

fn command(code: u8, id: u8, data: &[u8]) -> Command {
    let mut c = Command::new();
    let _ = c.extend_from_slice(&[code, id]);
    let _ = c.extend_from_slice(&(data.len() as u16).to_le_bytes());
    let _ = c.extend_from_slice(data);
    c
}

fn words(ws: &[u16]) -> Vec<u8, 16> {
    let mut v = Vec::new();
    for w in ws {
        let _ = v.extend_from_slice(&w.to_le_bytes());
    }
    v
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channel {
    pub psm: u16,
    /// Our CID (theirs sends to it), and theirs (0 until their connection response names it).
    pub local: u16,
    pub remote: u16,
    /// Our configuration request was accepted; we accepted theirs.
    ours: bool,
    theirs: bool,
}

impl Channel {
    pub fn open(&self) -> bool {
        self.ours && self.theirs
    }
}

/// What the commands that came did to the channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Happened {
    Opened(u16),
    Closed(u16),
    /// The other side refused our connection request to this PSM.
    Refused(u16),
}

/// The signaling state of one ACL link.
#[derive(Debug, Default)]
pub struct Signaling {
    id: u8,
    channels: Vec<Channel, 3>,
}

impl Signaling {
    pub fn new() -> Self {
        Self::default()
    }

    fn next_id(&mut self) -> u8 {
        self.id = self.id.wrapping_add(1).max(1);
        self.id
    }

    fn new_cid(&self) -> u16 {
        (FIRST_CID..).find(|c| !self.channels.iter().any(|ch| ch.local == *c)).unwrap_or(FIRST_CID)
    }

    pub fn channel(&self, psm: u16) -> Option<&Channel> {
        self.channels.iter().find(|c| c.psm == psm)
    }

    /// The channel data on our CID `cid` belongs to.
    pub fn by_local(&self, cid: u16) -> Option<&Channel> {
        self.channels.iter().find(|c| c.local == cid)
    }

    fn config_request(&mut self, remote: u16) -> Command {
        let id = self.next_id();
        // Flags 0, then the MTU option (type 1, length 2).
        let mut d = words(&[remote, 0]);
        let _ = d.extend_from_slice(&[0x01, 0x02]);
        let _ = d.extend_from_slice(&MTU.to_le_bytes());
        command(op::CONFIGURATION_REQUEST, id, &d)
    }

    /// Opens a channel to `psm` (the host's side: a pad it pairs); the request to send. None if there is no room.
    pub fn connect(&mut self, psm: u16) -> Option<Command> {
        let local = self.new_cid();
        self.channels.push(Channel { psm, local, remote: 0, ours: false, theirs: false }).ok()?;
        let id = self.next_id();
        Some(command(op::CONNECTION_REQUEST, id, &words(&[psm, local])))
    }

    /// Closes the channel to `psm`; the request to send (the channel goes when the response comes, or now if it never
    /// got a CID of theirs).
    pub fn disconnect(&mut self, psm: u16) -> Option<Command> {
        let ch = *self.channel(psm)?;
        if ch.remote == 0 {
            self.channels.retain(|c| c.psm != psm);
            return None;
        }
        let id = self.next_id();
        Some(command(op::DISCONNECTION_REQUEST, id, &words(&[ch.remote, ch.local])))
    }

    /// The link went: every channel with it.
    pub fn reset(&mut self) {
        self.channels.clear();
    }

    /// A signaling C-frame's payload came: each command in it handled. The answers go into `out`; what happened to the
    /// channels is returned.
    pub fn receive(&mut self, mut data: &[u8], out: &mut Commands) -> Vec<Happened, 4> {
        let mut happened = Vec::new();
        while data.len() >= 4 {
            let (code, id) = (data[0], data[1]);
            let len = u16::from_le_bytes([data[2], data[3]]) as usize;
            let Some(body) = data.get(4..4 + len) else { break };
            data = &data[4 + len..];
            if let Some(h) = self.command(code, id, body, out) {
                let _ = happened.push(h);
            }
        }
        happened
    }

    fn word(body: &[u8], at: usize) -> u16 {
        body.get(at..at + 2).map_or(0, |w| u16::from_le_bytes([w[0], w[1]]))
    }

    fn command(&mut self, code: u8, id: u8, body: &[u8], out: &mut Commands) -> Option<Happened> {
        let w = |at| Self::word(body, at);
        match code {
            op::CONNECTION_REQUEST => {
                let (psm, scid) = (w(0), w(2));
                let hid = psm == PSM_HID_CONTROL || psm == PSM_HID_INTERRUPT || psm == PSM_SDP;
                // A second request for a channel it has: the first went (its side restarted).
                self.channels.retain(|c| c.psm != psm);
                let local = self.new_cid();
                let accepted =
                    hid && self.channels.push(Channel { psm, local, remote: scid, ours: false, theirs: false }).is_ok();
                // Result 0: success; 2: PSM not supported; 4: no resources.
                let result = if accepted {
                    0
                } else if hid {
                    4
                } else {
                    2
                };
                let _ = out.push(command(
                    op::CONNECTION_RESPONSE,
                    id,
                    &words(&[if accepted { local } else { 0 }, scid, result, 0]),
                ));
                if accepted {
                    let c = self.config_request(scid);
                    let _ = out.push(c);
                }
                None
            }
            op::CONNECTION_RESPONSE => {
                let (dcid, scid, result) = (w(0), w(2), w(4));
                let ch = self.channels.iter_mut().find(|c| c.local == scid)?;
                match result {
                    0 => {
                        ch.remote = dcid;
                        let c = self.config_request(dcid);
                        let _ = out.push(c);
                        None
                    }
                    // Pending: the response with the outcome comes later.
                    1 => None,
                    _ => {
                        let psm = ch.psm;
                        self.channels.retain(|c| c.local != scid);
                        Some(Happened::Refused(psm))
                    }
                }
            }
            op::CONFIGURATION_REQUEST => {
                let dcid = w(0);
                let Some(ch) = self.channels.iter_mut().find(|c| c.local == dcid) else {
                    // Reason 2: invalid CID, with the CIDs asked for.
                    let _ = out.push(command(op::COMMAND_REJECT, id, &words(&[2, dcid, 0])));
                    return None;
                };
                // Whatever it asks for is fine (a pad asks for its MTU, at most a flush timeout): success, no options.
                let _ = out.push(command(op::CONFIGURATION_RESPONSE, id, &words(&[ch.remote, 0, 0])));
                let was = ch.open();
                ch.theirs = true;
                (!was && ch.open()).then_some(Happened::Opened(ch.psm))
            }
            op::CONFIGURATION_RESPONSE => {
                let (scid, result) = (w(0), w(4));
                let ch = self.channels.iter_mut().find(|c| c.local == scid)?;
                if result != 0 {
                    let psm = ch.psm;
                    self.channels.retain(|c| c.local != scid);
                    return Some(Happened::Refused(psm));
                }
                let was = ch.open();
                ch.ours = true;
                (!was && ch.open()).then_some(Happened::Opened(ch.psm))
            }
            op::DISCONNECTION_REQUEST => {
                let (dcid, scid) = (w(0), w(2));
                let _ = out.push(command(op::DISCONNECTION_RESPONSE, id, &words(&[dcid, scid])));
                let psm = self.by_local(dcid)?.psm;
                self.channels.retain(|c| c.local != dcid);
                Some(Happened::Closed(psm))
            }
            op::DISCONNECTION_RESPONSE => {
                let scid = w(2);
                let psm = self.by_local(scid)?.psm;
                self.channels.retain(|c| c.local != scid);
                Some(Happened::Closed(psm))
            }
            op::ECHO_REQUEST => {
                let _ = out.push(command(op::ECHO_RESPONSE, id, &[]));
                None
            }
            op::INFORMATION_REQUEST => {
                // Extended features (2): none; fixed channels (3): signaling only; anything else: not supported.
                let c = match w(0) {
                    2 => command(op::INFORMATION_RESPONSE, id, &[2, 0, 0, 0, 0, 0, 0, 0]),
                    3 => command(op::INFORMATION_RESPONSE, id, &[3, 0, 0, 0, 0x02, 0, 0, 0, 0, 0, 0, 0]),
                    t => command(op::INFORMATION_RESPONSE, id, &words(&[t, 1])),
                };
                let _ = out.push(c);
                None
            }
            op::COMMAND_REJECT | op::ECHO_RESPONSE | op::INFORMATION_RESPONSE => None,
            _ => {
                // Reason 0: command not understood.
                let _ = out.push(command(op::COMMAND_REJECT, id, &[0, 0]));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(code: u8, id: u8, ws: &[u16]) -> std::vec::Vec<u8> {
        command(code, id, &words(ws)).to_vec()
    }

    #[test]
    fn a_pad_that_reconnects_opens_both_channels() {
        let mut s = Signaling::new();
        let mut out = Commands::new();
        // It asks for HID control from its CID 0x0041.
        assert!(s.receive(&cmd(op::CONNECTION_REQUEST, 7, &[PSM_HID_CONTROL, 0x0041]), &mut out).is_empty());
        assert_eq!(out[0].as_slice(), cmd(op::CONNECTION_RESPONSE, 7, &[0x0040, 0x0041, 0, 0]).as_slice());
        assert_eq!(out[1].as_slice(), &[op::CONFIGURATION_REQUEST, 1, 8, 0, 0x41, 0, 0, 0, 1, 2, 0xA0, 0x02]);
        out.clear();
        // Its configuration request (MTU 672), then its response to ours: open.
        let mut req = cmd(op::CONFIGURATION_REQUEST, 8, &[0x0040, 0]);
        req[2] = 8;
        req.extend_from_slice(&[1, 2, 0xA0, 0x02]);
        assert!(s.receive(&req, &mut out).is_empty());
        assert_eq!(out[0].as_slice(), cmd(op::CONFIGURATION_RESPONSE, 8, &[0x0041, 0, 0]).as_slice());
        let rsp = cmd(op::CONFIGURATION_RESPONSE, 1, &[0x0040, 0, 0]);
        assert_eq!(s.receive(&rsp, &mut out).as_slice(), &[Happened::Opened(PSM_HID_CONTROL)]);
        // Interrupt the same way; both commands may come in one frame.
        out.clear();
        s.receive(&cmd(op::CONNECTION_REQUEST, 9, &[PSM_HID_INTERRUPT, 0x0042]), &mut out);
        let local = s.channel(PSM_HID_INTERRUPT).unwrap().local;
        assert_eq!(local, 0x0041);
        let mut both = cmd(op::CONFIGURATION_REQUEST, 10, &[local, 0]);
        both.extend(cmd(op::CONFIGURATION_RESPONSE, 2, &[local, 0, 0]));
        assert_eq!(s.receive(&both, &mut out).as_slice(), &[Happened::Opened(PSM_HID_INTERRUPT)]);
        assert_eq!(s.by_local(0x0041).map(|c| c.remote), Some(0x0042));
        // It closes control.
        out.clear();
        let gone = s.receive(&cmd(op::DISCONNECTION_REQUEST, 11, &[0x0040, 0x0041]), &mut out);
        assert_eq!(gone.as_slice(), &[Happened::Closed(PSM_HID_CONTROL)]);
        assert_eq!(out[0].as_slice(), cmd(op::DISCONNECTION_RESPONSE, 11, &[0x0040, 0x0041]).as_slice());
    }

    #[test]
    fn the_host_opens_a_channel_to_a_pad_it_pairs() {
        let mut s = Signaling::new();
        let mut out = Commands::new();
        let req = s.connect(PSM_HID_CONTROL).unwrap();
        assert_eq!(req.as_slice(), cmd(op::CONNECTION_REQUEST, 1, &[PSM_HID_CONTROL, 0x0040]).as_slice());
        // Pending first, then success with its CID.
        assert!(s.receive(&cmd(op::CONNECTION_RESPONSE, 1, &[0, 0x0040, 1, 0]), &mut out).is_empty());
        assert!(out.is_empty());
        s.receive(&cmd(op::CONNECTION_RESPONSE, 1, &[0x0070, 0x0040, 0, 0]), &mut out);
        assert_eq!(out[0][0], op::CONFIGURATION_REQUEST);
        assert_eq!(&out[0][4..6], &[0x70, 0]);
        out.clear();
        assert!(s.receive(&cmd(op::CONFIGURATION_RESPONSE, 2, &[0x0040, 0, 0]), &mut out).is_empty());
        let opened = s.receive(&cmd(op::CONFIGURATION_REQUEST, 3, &[0x0040, 0]), &mut out);
        assert_eq!(opened.as_slice(), &[Happened::Opened(PSM_HID_CONTROL)]);
        // A refused one goes.
        s.connect(PSM_HID_INTERRUPT).unwrap();
        let refused = s.receive(&cmd(op::CONNECTION_RESPONSE, 4, &[0, 0x0041, 3, 0]), &mut out);
        assert_eq!(refused.as_slice(), &[Happened::Refused(PSM_HID_INTERRUPT)]);
        assert!(s.channel(PSM_HID_INTERRUPT).is_none());
    }

    #[test]
    fn other_requests_are_answered() {
        let mut s = Signaling::new();
        let mut out = Commands::new();
        // SDP: yes (a pad that reconnects asks it first); RFCOMM: not supported.
        s.receive(&cmd(op::CONNECTION_REQUEST, 1, &[PSM_SDP, 0x0050]), &mut out);
        assert_eq!(out[0].as_slice(), cmd(op::CONNECTION_RESPONSE, 1, &[0x0040, 0x0050, 0, 0]).as_slice(), "SDP");
        out.clear();
        s.receive(&cmd(op::CONNECTION_REQUEST, 4, &[0x0003, 0x0051]), &mut out);
        assert_eq!(out[0].as_slice(), cmd(op::CONNECTION_RESPONSE, 4, &[0, 0x0051, 2, 0]).as_slice(), "RFCOMM: no");
        out.clear();
        s.receive(&cmd(op::INFORMATION_REQUEST, 2, &[2]), &mut out);
        assert_eq!(out[0].as_slice(), &[op::INFORMATION_RESPONSE, 2, 8, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
        out.clear();
        s.receive(&[op::ECHO_REQUEST, 3, 2, 0, 0xAB, 0xCD], &mut out);
        assert_eq!(out[0].as_slice(), &[op::ECHO_RESPONSE, 3, 0, 0]);
        out.clear();
        s.receive(&[0x55, 4, 0, 0], &mut out);
        assert_eq!(out[0].as_slice(), &[op::COMMAND_REJECT, 4, 2, 0, 0, 0]);
        let mut pdu = [0u8; 16];
        let n = write_frame(SIGNALING, &[1, 2, 3], &mut pdu).unwrap();
        assert_eq!(frame(&pdu[..n]), Some((SIGNALING, &[1u8, 2, 3][..])));
    }
}
