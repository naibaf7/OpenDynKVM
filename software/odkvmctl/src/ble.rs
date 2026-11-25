//! The KVM's control service over Bluetooth LE (software.md §8.1 "Over Bluetooth"), for a computer paired with the
//! Bluetooth module as one of its hosts: the same 64-byte reports as the PCs' control interface, written to one
//! characteristic of the service ([`control::REQUEST`]) and notified by another ([`control::REPLY`]).
//!
//! - **Finding it:** the system already holds the link (the KVM is its keyboard and mouse), so the KVM is looked for
//!   among the connected devices that have the service, not by scanning.
//! - **Linux:** BlueZ over D-Bus, in pure Rust (zbus): `WriteValue` without response, `StartNotify` and the `Value`
//!   property's changes, which every program that asked gets (as each reads every answer on hidraw).
//! - **Windows, macOS:** WinRT's GATT client and CoreBluetooth (bluest).
//!
//! Either way a thread of its own takes the notifications into a channel, which [`BleTransport::read`] waits on.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use kvm_proto::link::bt::control;

use crate::transport::{Found, Link, Report, Transport};

/// What the notifications' thread passes on: a report, or why the link ended.
type Incoming = Result<Report, String>;

/// How long opening may wait for the notifications to be switched on.
#[cfg(any(windows, target_os = "macos"))]
const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(5);

/// A KVM reached over its Bluetooth module's control service.
pub struct BleTransport {
    rx: Receiver<Incoming>,
    link: sys::Link,
    desc: String,
}

impl BleTransport {
    pub fn open(f: &Found) -> Result<Self, String> {
        let (link, rx) = sys::open(f)?;
        Ok(Self { rx, link, desc: crate::fl!("ble-describe", name = f.product.clone(), address = f.serial.clone()) })
    }
}

/// The KVMs this computer is connected to as a Bluetooth host, with the control service; none where there is no
/// Bluetooth (no adapter, no BlueZ).
pub fn find() -> Vec<Found> {
    sys::find().unwrap_or_default()
}

impl Transport for BleTransport {
    fn write(&mut self, report: &Report) -> Result<(), String> {
        self.link.write(report)
    }

    fn read(&mut self, timeout: Duration) -> Result<Option<Report>, String> {
        match self.rx.recv_timeout(timeout) {
            Ok(Ok(r)) => Ok(Some(r)),
            Ok(Err(e)) => Err(e),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(crate::fl!("ble-lost")),
        }
    }

    fn describe(&self) -> String {
        self.desc.clone()
    }

    fn link(&self) -> Link {
        Link::Bluetooth
    }

    fn window(&self) -> usize {
        control::WINDOW
    }
}

/// A Bluetooth found entry for `path` (the system's handle on the device or service).
fn found(path: &str, name: String, address: String) -> Option<Found> {
    Some(Found {
        link: Link::Bluetooth,
        path: std::ffi::CString::new(path).ok()?,
        vid: 0,
        pid: 0,
        product: if name.is_empty() { "OpenDynKVM".into() } else { name },
        serial: address,
    })
}

#[cfg(target_os = "linux")]
mod sys {
    //! BlueZ: the GATT objects it exports for a connected device, over the system bus.

    use std::collections::HashMap;
    use std::sync::mpsc::{Receiver, channel};

    use kvm_proto::link::bt::control;
    use zbus::blocking::{Connection, MessageIterator, Proxy};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    use super::Incoming;
    use crate::transport::{Found, Report};

    type Props = HashMap<String, OwnedValue>;
    type Managed = HashMap<OwnedObjectPath, HashMap<String, Props>>;

    const BLUEZ: &str = "org.bluez";
    const SERVICE: &str = "org.bluez.GattService1";
    const CHARACTERISTIC: &str = "org.bluez.GattCharacteristic1";
    const DEVICE: &str = "org.bluez.Device1";

    fn err(e: impl std::fmt::Display) -> String {
        format!("Bluetooth (BlueZ): {e}")
    }

    fn managed(conn: &Connection) -> Result<Managed, String> {
        let p = Proxy::new(conn, BLUEZ, "/", "org.freedesktop.DBus.ObjectManager").map_err(err)?;
        p.call("GetManagedObjects", &()).map_err(err)
    }

    fn text(p: &Props, k: &str) -> String {
        p.get(k).and_then(|v| v.downcast_ref::<&str>().ok().map(str::to_string)).unwrap_or_default()
    }

    fn flag(p: &Props, k: &str) -> bool {
        p.get(k).and_then(|v| v.downcast_ref::<bool>().ok()).unwrap_or(false)
    }

    fn object(p: &Props, k: &str) -> Option<String> {
        p.get(k).and_then(|v| v.downcast_ref::<zbus::zvariant::ObjectPath>().ok()).map(|o| o.to_string())
    }

    /// Each connected device's control service: (its path, the device's path and properties).
    fn services(objs: &Managed) -> Vec<(String, String, &Props)> {
        objs.iter()
            .filter_map(|(path, ifaces)| {
                let svc = ifaces.get(SERVICE)?;
                if !text(svc, "UUID").eq_ignore_ascii_case(control::SERVICE) {
                    return None;
                }
                let dev_path = object(svc, "Device")?;
                let dev = objs.iter().find(|(p, _)| p.as_str() == dev_path)?.1.get(DEVICE)?;
                flag(dev, "Connected").then(|| (path.to_string(), dev_path, dev))
            })
            .collect()
    }

    pub fn find() -> Result<Vec<Found>, String> {
        let conn = Connection::system().map_err(err)?;
        let objs = managed(&conn)?;
        Ok(services(&objs)
            .into_iter()
            .filter_map(|(path, _, dev)| super::found(&path, text(dev, "Alias"), text(dev, "Address")))
            .collect())
    }

    /// The writing side, and what stops the notifications when it goes.
    pub struct Link {
        conn: Connection,
        request: String,
        reply: String,
    }

    impl Link {
        pub fn write(&mut self, report: &Report) -> Result<(), String> {
            let ch = Proxy::new(&self.conn, BLUEZ, self.request.as_str(), CHARACTERISTIC).map_err(err)?;
            let opts: HashMap<&str, Value> = HashMap::from([("type", Value::from("command"))]);
            ch.call_method("WriteValue", &(report.to_vec(), opts)).map(|_| ()).map_err(err)
        }
    }

    impl Drop for Link {
        fn drop(&mut self) {
            if let Ok(ch) = Proxy::new(&self.conn, BLUEZ, self.reply.as_str(), CHARACTERISTIC) {
                let _ = ch.call_method("StopNotify", &());
            }
            // Ends the notifications' thread, which waits on this connection's messages.
            let _ = self.conn.clone().close();
        }
    }

    pub fn open(f: &Found) -> Result<(Link, Receiver<Incoming>), String> {
        let conn = Connection::system().map_err(err)?;
        let objs = managed(&conn)?;
        let svc = f.path.to_string_lossy().to_string();
        let (_, dev_path, _) = services(&objs)
            .into_iter()
            .find(|(p, _, _)| *p == svc)
            .ok_or_else(|| crate::fl!("ble-gone"))?;
        let characteristic = |uuid: &str| {
            objs.iter()
                .find(|(_, ifaces)| {
                    ifaces.get(CHARACTERISTIC).is_some_and(|c| {
                        object(c, "Service").as_deref() == Some(svc.as_str())
                            && text(c, "UUID").eq_ignore_ascii_case(uuid)
                    })
                })
                .map(|(p, _)| p.to_string())
                .ok_or_else(|| crate::fl!("ble-gone"))
        };
        let request = characteristic(control::REQUEST)?;
        let reply = characteristic(control::REPLY)?;
        // Listening before the notifications are on: the reply characteristic's values, and the device going.
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.DBus.Properties")
            .map_err(err)?
            .member("PropertiesChanged")
            .map_err(err)?
            .path_namespace(dev_path.clone())
            .map_err(err)?
            .build();
        let messages = MessageIterator::for_match_rule(rule, &conn, Some(64)).map_err(err)?;
        let (tx, rx) = channel::<Incoming>();
        let reply_path = reply.clone();
        std::thread::Builder::new()
            .name("ble-notify".into())
            .spawn(move || {
                for m in messages {
                    let Ok(m) = m else { break };
                    let header = m.header();
                    let Some(path) = header.path().map(|p| p.to_string()) else { continue };
                    let Ok((iface, changed, _)) = m.body().deserialize::<(String, Props, Vec<String>)>() else {
                        continue;
                    };
                    let sent = if path == reply_path && iface == CHARACTERISTIC {
                        match changed.get("Value").and_then(|v| v.downcast_ref::<zbus::zvariant::Array>().ok()) {
                            Some(a) => {
                                let bytes: Vec<u8> = a.iter().filter_map(|b| b.downcast_ref::<u8>().ok()).collect();
                                tx.send(Ok(control::pad(&bytes)))
                            }
                            None => continue,
                        }
                    } else if path == dev_path && iface == DEVICE && changed.contains_key("Connected") {
                        if flag(&changed, "Connected") {
                            continue;
                        }
                        let _ = tx.send(Err(crate::fl!("ble-lost")));
                        break;
                    } else {
                        continue;
                    };
                    if sent.is_err() {
                        break;
                    }
                }
            })
            .map_err(err)?;
        {
            let ch = Proxy::new(&conn, BLUEZ, reply.as_str(), CHARACTERISTIC).map_err(err)?;
            // Another program here may have them on already: BlueZ then counts this one too.
            ch.call_method("StartNotify", &()).map_err(err)?;
        }
        Ok((Link { conn, request, reply }, rx))
    }
}

#[cfg(any(windows, target_os = "macos"))]
mod sys {
    //! WinRT's GATT client (Windows) and CoreBluetooth (macOS), through bluest; its futures need no runtime of their
    //! own, so each call is waited for here.

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{Receiver, channel};
    use std::sync::{Arc, Mutex};
    use std::task::{Poll, Waker};
    use std::time::Duration;

    use bluest::{Adapter, Characteristic, Device, Uuid};
    use futures_lite::StreamExt;
    use futures_lite::future::{block_on, or, poll_fn};
    use kvm_proto::link::bt::control;

    use super::{Incoming, SUBSCRIBE_TIMEOUT};
    use crate::transport::{Found, Report};

    fn err(e: impl std::fmt::Display) -> String {
        format!("Bluetooth: {e}")
    }

    fn uuid(s: &str) -> Uuid {
        Uuid::parse_str(s).expect("a UUID")
    }

    async fn adapter() -> Result<Adapter, String> {
        let a = Adapter::default().await.ok_or_else(|| crate::fl!("ble-no-adapter"))?;
        a.wait_available().await.map_err(err)?;
        Ok(a)
    }

    async fn connected(a: &Adapter) -> Result<Vec<Device>, String> {
        a.connected_devices_with_services(&[uuid(control::SERVICE)]).await.map_err(err)
    }

    pub fn find() -> Result<Vec<Found>, String> {
        block_on(async {
            let a = adapter().await?;
            Ok(connected(&a)
                .await?
                .into_iter()
                .filter_map(|d| {
                    let id = d.id().to_string();
                    super::found(&id, d.name().unwrap_or_default(), id.clone())
                })
                .collect())
        })
    }

    /// Set when the link goes: wakes the notifications' thread, which then lets go of what it holds.
    #[derive(Default)]
    struct Stop {
        done: AtomicBool,
        waker: Mutex<Option<Waker>>,
    }

    impl Stop {
        fn stop(&self) {
            self.done.store(true, Ordering::SeqCst);
            if let Some(w) = self.waker.lock().unwrap().take() {
                w.wake();
            }
        }

        async fn wait(&self) {
            poll_fn(|cx| {
                if self.done.load(Ordering::SeqCst) {
                    return Poll::Ready(());
                }
                *self.waker.lock().unwrap() = Some(cx.waker().clone());
                if self.done.load(Ordering::SeqCst) { Poll::Ready(()) } else { Poll::Pending }
            })
            .await
        }
    }

    /// How long dropping a link waits for its notifications' thread to let go.
    const LET_GO: Duration = Duration::from_secs(2);

    /// The writing side; the adapter and device are kept for as long as the link is used, and the notifications'
    /// thread is stopped when it goes.
    pub struct Link {
        request: Characteristic,
        _keep: (Adapter, Device),
        stop: Arc<Stop>,
        /// Ends (disconnected) once the thread has dropped its stream and characteristic.
        gone: Receiver<()>,
    }

    impl Link {
        pub fn write(&mut self, report: &Report) -> Result<(), String> {
            block_on(self.request.write_without_response(report)).map_err(err)
        }
    }

    /// Windows hands out a device's GATT objects per program, and while one of an earlier link is still held, the
    /// next link's are refused ("access denied"). The notifications' thread waited for a notification that never came
    /// (its stream doesn't end when the link drops), and held its own until one did: after a timeout the window
    /// could no longer reach the KVM until it was restarted, or until the next switch notified it (2026-11-24).
    impl Drop for Link {
        fn drop(&mut self) {
            self.stop.stop();
            let _ = self.gone.recv_timeout(LET_GO);
        }
    }

    pub fn open(f: &Found) -> Result<(Link, Receiver<Incoming>), String> {
        let want = f.path.to_string_lossy().to_string();
        let (a, d, request, reply) = block_on(async {
            let a = adapter().await?;
            let d = connected(&a)
                .await?
                .into_iter()
                .find(|d| d.id().to_string() == want)
                .ok_or_else(|| crate::fl!("ble-gone"))?;
            // The system holds the link; this program's GATT session on it.
            a.connect_device(&d).await.map_err(err)?;
            let svc = d
                .discover_services_with_uuid(uuid(control::SERVICE))
                .await
                .map_err(err)?
                .into_iter()
                .next()
                .ok_or_else(|| crate::fl!("ble-gone"))?;
            let one = |mut v: Vec<Characteristic>| v.pop().ok_or_else(|| crate::fl!("ble-gone"));
            let request = one(svc.discover_characteristics_with_uuid(uuid(control::REQUEST)).await.map_err(err)?)?;
            let reply = one(svc.discover_characteristics_with_uuid(uuid(control::REPLY)).await.map_err(err)?)?;
            Ok::<_, String>((a, d, request, reply))
        })?;
        let (tx, rx) = channel::<Incoming>();
        let (ready_tx, ready_rx) = channel::<Result<(), String>>();
        let (gone_tx, gone) = channel::<()>();
        let stop = Arc::new(Stop::default());
        let stopped = stop.clone();
        std::thread::Builder::new()
            .name("ble-notify".into())
            .spawn(move || {
                block_on(async move {
                    let mut stream = match or(async { Some(reply.notify().await) }, async {
                        stopped.wait().await;
                        None
                    })
                    .await
                    {
                        Some(Ok(s)) => {
                            let _ = ready_tx.send(Ok(()));
                            s
                        }
                        Some(Err(e)) => {
                            let _ = ready_tx.send(Err(err(e)));
                            return;
                        }
                        None => return,
                    };
                    loop {
                        let next = or(async { Some(stream.next().await) }, async {
                            stopped.wait().await;
                            None
                        })
                        .await;
                        let item = match next {
                            // The link went: the stream (it switches the notifications off) and the characteristic
                            // are dropped on the way out.
                            None => return,
                            Some(None) => Err(crate::fl!("ble-lost")),
                            Some(Some(v)) => v.map(|v| control::pad(&v)).map_err(err),
                        };
                        let end = item.is_err();
                        if tx.send(item).is_err() || end {
                            return;
                        }
                    }
                });
                // Only now: everything the thread held is gone.
                drop(gone_tx);
            })
            .map_err(err)?;
        let link = Link { request, _keep: (a, d), stop, gone };
        // Dropping `link` on the way out stops the thread, whether or not it subscribed.
        ready_rx.recv_timeout(SUBSCRIBE_TIMEOUT).map_err(|_| crate::fl!("ble-lost"))??;
        Ok((link, rx))
    }
}

/// No Bluetooth stack this program knows (other Unix systems): no KVM over Bluetooth.
#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod sys {
    use std::sync::mpsc::Receiver;

    use super::Incoming;
    use crate::transport::{Found, Report};

    pub fn find() -> Result<Vec<Found>, String> {
        Ok(Vec::new())
    }

    pub struct Link;

    impl Link {
        pub fn write(&mut self, _: &Report) -> Result<(), String> {
            Err(crate::fl!("ble-no-adapter"))
        }
    }

    pub fn open(_: &Found) -> Result<(Link, Receiver<Incoming>), String> {
        Err(crate::fl!("ble-no-adapter"))
    }
}
