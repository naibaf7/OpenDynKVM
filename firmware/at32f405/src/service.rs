//! Handles messages from the RP2354B: reports, descriptor sets, control replies, commands.

use core::cell::RefCell;

use critical_section::Mutex;
use defmt::{info, warn};
use embassy_time::Timer;
use heapless::Vec;
use kvm_proto::crc::crc32;
use kvm_proto::descset::{DescSet, MAX_LEN};
use kvm_proto::link::msg::{Command, CtrlStatus, Reassembly, hello_flags};
use kvm_proto::link::{LinkMsg, PROTOCOL_VERSION};
use kvm_usbdev::Cmd;

use crate::{FW_VERSION, board, link, usb};

static STAGING: Mutex<RefCell<Reassembly<MAX_LEN>>> = Mutex::new(RefCell::new(Reassembly::new()));
static COMMITTED: Mutex<RefCell<Vec<u8, MAX_LEN>>> = Mutex::new(RefCell::new(Vec::new()));

/// What a `DescCommit` did with the uploaded set.
enum Commit {
    New,
    Unchanged,
    Rejected,
}

/// Copies the committed descriptor set into `buf` (for the USB core).
pub fn committed_set(buf: &mut [u8]) -> usize {
    critical_section::with(|cs| {
        let set = COMMITTED.borrow_ref(cs);
        let n = set.len().min(buf.len());
        buf[..n].copy_from_slice(&set[..n]);
        n
    })
}

pub async fn hello(role: u8) {
    let flags = if usb::running() { 0 } else { hello_flags::NO_CRYSTAL };
    link::send(&LinkMsg::Hello { proto: PROTOCOL_VERSION, fw_version: FW_VERSION, role, flags }).await;
}

#[embassy_executor::task]
pub async fn run(role: u8) {
    hello(role).await;
    let mut desc_crc = 0u32;
    let mut reply = Reassembly::<1024>::new();
    let mut reply_tag = 0u8;
    loop {
        let msg = link::INBOX.receive().await;
        let Ok(m) = LinkMsg::decode(&msg) else {
            warn!("undecodable link message type {=u8:#x}", msg.first().copied().unwrap_or(0));
            continue;
        };
        match m {
            LinkMsg::Report { ep, data } => usb::queue_report(ep, data),
            LinkMsg::Hello { proto, .. } => {
                // The RP2354B's link (re)started: tell it who this is. It doesn't answer this one (software.md §6.2).
                info!("RP2354B says hello (protocol {})", proto);
                hello(role).await;
            }
            LinkMsg::DescBegin { total_len, crc32: c } => {
                desc_crc = c;
                let ok = critical_section::with(|cs| STAGING.borrow_ref_mut(cs).start(total_len as usize).is_ok());
                if !ok {
                    warn!("descriptor set too large: {}", total_len);
                }
            }
            LinkMsg::DescData { offset, data } => {
                critical_section::with(|cs| {
                    let _ = STAGING.borrow_ref_mut(cs).put(offset as usize, data);
                });
            }
            LinkMsg::DescCommit => {
                let commit = critical_section::with(|cs| {
                    let staging = STAGING.borrow_ref(cs);
                    let data = staging.data();
                    if !staging.is_complete() || crc32(data) != desc_crc || DescSet::parse(data).is_err() {
                        return Commit::Rejected;
                    }
                    let mut set = COMMITTED.borrow_ref_mut(cs);
                    if set.as_slice() == data {
                        return Commit::Unchanged;
                    }
                    set.clear();
                    if set.extend_from_slice(data).is_ok() { Commit::New } else { Commit::Rejected }
                });
                match commit {
                    Commit::New => {
                        info!("descriptor set {=u32:#010x} committed", desc_crc);
                        usb::COMMANDS.send(Cmd::NewSet).await;
                    }
                    // The same set again (a repeated upload): taking it would only drop the PC's connection for a
                    // moment, and back-to-back repeats would keep it off the bus.
                    Commit::Unchanged => {}
                    Commit::Rejected => warn!("descriptor set rejected (incomplete, CRC or format)"),
                }
            }
            LinkMsg::CtrlReply { tag, status, total_len, offset, data } => {
                if offset == 0 {
                    reply_tag = tag;
                    let _ = reply.start(total_len as usize);
                }
                if tag != reply_tag || reply.put(offset as usize, data).is_err() {
                    continue;
                }
                if reply.is_complete() {
                    let data = Vec::from_slice(reply.data()).unwrap_or_default();
                    usb::CTRL_REPLY.signal(usb::CtrlReply { tag, stall: status == CtrlStatus::Stall, data });
                }
            }
            LinkMsg::Command(c) => match c {
                Command::Attach => usb::COMMANDS.send(Cmd::Attach).await,
                Command::Detach => usb::COMMANDS.send(Cmd::Detach).await,
                Command::RemoteWakeup => usb::COMMANDS.send(Cmd::RemoteWakeup).await,
                Command::RequestStatus => usb::STATUS_DIRTY.signal(()),
                Command::HubReset => {
                    board::hub_reset(true);
                    Timer::after_millis(20).await;
                    board::hub_reset(false);
                }
            },
            // Messages that only travel AT32F405 → RP2354B.
            LinkMsg::CtrlRequest { .. }
            | LinkMsg::SetReport { .. }
            | LinkMsg::ProtocolChanged { .. }
            | LinkMsg::Status(_) => {}
        }
    }
}
