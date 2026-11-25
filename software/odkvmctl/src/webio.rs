//! The KVM from a browser page (the web build, wasm32: software.md §9 "On the web"): its control interface over
//! WebHID, or its wireless module's control service over Web Bluetooth (both Chromium-based browsers only), as
//! [`Transport`]s. The page's event loop delivers the reports, so everything here is async; the small JS below keeps
//! each link's reports in a queue until the client reads them, and sends one write at a time (Web Bluetooth refuses a
//! second while one is under way).
//!
//! The browser asks the user which device to give the page (`requestDevice`, after a click); one given before is taken
//! again without asking ([`granted`]).

#![cfg(target_arch = "wasm32")]

use std::time::Duration;

use js_sys::{Promise, Uint8Array};
use kvm_proto::link::bt::control;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::transport::{Link, Report, Transport};

#[wasm_bindgen(inline_js = r#"
const SERVICE = '98141409-422d-48a6-aba2-21d691a393cc';
const REQUEST = '98141410-422d-48a6-aba2-21d691a393cc';
const REPLY = '98141411-422d-48a6-aba2-21d691a393cc';
const USAGE_PAGE = 0xFF4B;

function link(kind) {
  return { kind, queue: [], waiter: null, gone: null, chain: Promise.resolve(), name: '' };
}
function push(l, bytes) {
  if (l.waiter) {
    const w = l.waiter;
    l.waiter = null;
    clearTimeout(w.timer);
    w.resolve(bytes);
  } else {
    l.queue.push(bytes);
    if (l.queue.length > 256) l.queue.shift();
  }
}
function end(l, why) {
  l.gone = why;
  if (l.waiter) {
    const w = l.waiter;
    l.waiter = null;
    clearTimeout(w.timer);
    w.reject(why);
  }
}
function bytes(view) {
  return new Uint8Array(view.buffer, view.byteOffset, view.byteLength).slice();
}
function isControl(d) {
  return d.collections.some((c) => c.usagePage === USAGE_PAGE && c.usage === 1);
}

export function hid_supported() { return !!navigator.hid; }
export function ble_supported() { return !!navigator.bluetooth; }

// The devices this page was given before: a browser that doesn't answer (some never do without a window) is none.
function soon(p) {
  return Promise.race([p, new Promise((r) => setTimeout(() => r([]), 2000))]);
}
export async function hid_granted() {
  if (!navigator.hid) return null;
  const all = await soon(navigator.hid.getDevices());
  return all.find(isControl) || null;
}
export async function hid_request() {
  const all = await navigator.hid.requestDevice({ filters: [{ usagePage: USAGE_PAGE, usage: 1 }] });
  return all.find(isControl) || null;
}
export async function hid_open(dev) {
  const l = link('usb');
  l.dev = dev;
  l.name = dev.productName || 'OpenDynKVM';
  if (!dev.opened) await dev.open();
  l.onReport = (e) => { if (e.reportId === 0) push(l, bytes(e.data)); };
  dev.addEventListener('inputreport', l.onReport);
  l.onGone = (e) => { if (e.device === dev) end(l, 'disconnected'); };
  navigator.hid.addEventListener('disconnect', l.onGone);
  return l;
}

export async function ble_granted() {
  if (!navigator.bluetooth || !navigator.bluetooth.getDevices) return null;
  const all = await soon(navigator.bluetooth.getDevices());
  return all.find((d) => (d.name || '').startsWith('OpenDynKVM')) || null;
}
export async function ble_request() {
  return navigator.bluetooth.requestDevice({
    filters: [{ services: [SERVICE] }, { namePrefix: 'OpenDynKVM' }],
    optionalServices: [SERVICE],
  });
}
export async function ble_open(device) {
  const l = link('wireless');
  l.device = device;
  l.name = device.name || 'OpenDynKVM';
  const server = await device.gatt.connect();
  const svc = await server.getPrimaryService(SERVICE);
  l.request = await svc.getCharacteristic(REQUEST);
  l.reply = await svc.getCharacteristic(REPLY);
  l.onReport = (e) => push(l, bytes(e.target.value));
  l.reply.addEventListener('characteristicvaluechanged', l.onReport);
  l.onGone = () => end(l, 'disconnected');
  device.addEventListener('gattserverdisconnected', l.onGone);
  await l.reply.startNotifications();
  return l;
}

export function link_send(l, data) {
  if (l.gone) return Promise.reject(l.gone);
  const send = () => l.kind === 'usb' ? l.dev.sendReport(0, data) : l.request.writeValueWithoutResponse(data);
  const p = l.chain.catch(() => {}).then(send);
  l.chain = p;
  return p;
}
export function link_read(l, ms) {
  if (l.queue.length) return Promise.resolve(l.queue.shift());
  if (l.gone) return Promise.reject(l.gone);
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { l.waiter = null; resolve(null); }, ms);
    l.waiter = { resolve, reject, timer };
  });
}
export function link_name(l) { return l.name; }
export function link_close(l) {
  try {
    if (l.kind === 'usb') {
      l.dev.removeEventListener('inputreport', l.onReport);
      navigator.hid.removeEventListener('disconnect', l.onGone);
      l.dev.close();
    } else {
      l.reply.removeEventListener('characteristicvaluechanged', l.onReport);
      l.device.removeEventListener('gattserverdisconnected', l.onGone);
      l.device.gatt.disconnect();
    }
  } catch (_) {}
  end(l, 'closed');
}
export function sleep_ms(ms) { return new Promise((r) => setTimeout(r, ms)); }
"#)]
extern "C" {
    fn hid_supported() -> bool;
    fn ble_supported() -> bool;
    #[wasm_bindgen(catch)]
    fn hid_granted() -> Result<Promise, JsValue>;
    #[wasm_bindgen(catch)]
    fn hid_request() -> Result<Promise, JsValue>;
    #[wasm_bindgen(catch)]
    fn hid_open(dev: &JsValue) -> Result<Promise, JsValue>;
    #[wasm_bindgen(catch)]
    fn ble_granted() -> Result<Promise, JsValue>;
    #[wasm_bindgen(catch)]
    fn ble_request() -> Result<Promise, JsValue>;
    #[wasm_bindgen(catch)]
    fn ble_open(device: &JsValue) -> Result<Promise, JsValue>;
    fn link_send(link: &JsValue, data: &Uint8Array) -> Promise;
    fn link_read(link: &JsValue, ms: f64) -> Promise;
    fn link_name(link: &JsValue) -> String;
    fn link_close(link: &JsValue);
    fn sleep_ms(ms: f64) -> Promise;
}

/// What a page's promise refused with, as text.
fn why(e: JsValue) -> String {
    e.as_string()
        .or_else(|| js_sys::Reflect::get(&e, &"message".into()).ok().and_then(|m| m.as_string()))
        .unwrap_or_else(|| format!("{e:?}"))
}

async fn wait(p: Result<Promise, JsValue>) -> Result<JsValue, String> {
    JsFuture::from(p.map_err(why)?).await.map_err(why)
}

/// Waits `d`, the page's event loop running meanwhile.
pub async fn sleep(d: Duration) {
    let _ = JsFuture::from(sleep_ms(d.as_secs_f64() * 1000.0)).await;
}

/// The browser offers WebHID: the KVM's control interface on a PC port.
pub fn usb_supported() -> bool {
    hid_supported()
}

/// The browser offers Web Bluetooth: the wireless module's control service, for a computer paired as one of its hosts.
pub fn wireless_supported() -> bool {
    ble_supported()
}

/// Asks the browser for a KVM (its chooser, after a click on the page) and opens it.
pub async fn request(link: Link) -> Result<Box<dyn Transport>, String> {
    let dev = match link {
        Link::Usb => wait(hid_request()).await?,
        Link::Bluetooth => wait(ble_request()).await?,
        Link::Simulated => return Ok(Box::new(crate::sim::SimTransport::new())),
    };
    if dev.is_null() || dev.is_undefined() {
        return Err(crate::fl!("no-kvm-found"));
    }
    open(link, &dev).await
}

/// A KVM this page was given before, opened again without asking (USB first); None if there is none, or it isn't there.
pub async fn granted() -> Option<Box<dyn Transport>> {
    for (link, found) in [(Link::Usb, hid_granted()), (Link::Bluetooth, ble_granted())] {
        let Ok(dev) = wait(found).await else { continue };
        if dev.is_null() || dev.is_undefined() {
            continue;
        }
        if let Ok(t) = open(link, &dev).await {
            return Some(t);
        }
    }
    None
}

async fn open(link: Link, dev: &JsValue) -> Result<Box<dyn Transport>, String> {
    let l = match link {
        Link::Bluetooth => wait(ble_open(dev)).await?,
        _ => wait(hid_open(dev)).await?,
    };
    let name = link_name(&l);
    let desc = match link {
        Link::Bluetooth => crate::fl!("web-describe-wireless", name = name),
        _ => crate::fl!("web-describe-usb", name = name),
    };
    Ok(Box::new(WebLink { l, link, desc }))
}

/// A KVM the page opened: its JS side (the queue of reports, the device), how it is reached, and what to call it.
pub struct WebLink {
    l: JsValue,
    link: Link,
    desc: String,
}

impl Drop for WebLink {
    fn drop(&mut self) {
        link_close(&self.l);
    }
}

#[async_trait::async_trait(?Send)]
impl Transport for WebLink {
    async fn write(&mut self, report: &Report) -> Result<(), String> {
        let data = Uint8Array::from(&report[..]);
        JsFuture::from(link_send(&self.l, &data)).await.map(|_| ()).map_err(why)
    }

    async fn read(&mut self, timeout: Duration) -> Result<Option<Report>, String> {
        let v = JsFuture::from(link_read(&self.l, timeout.as_secs_f64() * 1000.0)).await.map_err(why)?;
        if v.is_null() || v.is_undefined() {
            return Ok(None);
        }
        Ok(Some(control::pad(&Uint8Array::new(&v).to_vec())))
    }

    fn describe(&self) -> String {
        self.desc.clone()
    }

    fn link(&self) -> Link {
        self.link
    }

    fn window(&self) -> usize {
        match self.link {
            Link::Bluetooth => control::WINDOW,
            _ => 1,
        }
    }
}
