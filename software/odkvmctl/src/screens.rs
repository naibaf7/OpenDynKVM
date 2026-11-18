//! This computer's screens, for screen hop's arrangement (software.md §4.14, §9): each screen's resolution, display
//! scaling and place in this computer's own arrangement, and where its cursor is.
//!
//! - **Windows:** the monitor API, in physical pixels (the process is per-monitor DPI aware), scaling from each
//!   monitor's DPI. The main screen is at 0,0, and Windows maps an absolute pointer onto it alone
//!   ([`Mapping::MainScreen`]).
//! - **Linux:** an absolute pointer spans the whole desktop ([`Mapping::Desktop`]).
//!   - KDE Plasma on Wayland: `kscreen-doctor -j`, in logical pixels ([`Units::Logical`]), with each output's scaling.
//!     `xrandr` there sees XWayland, whose coordinates are scaled by the largest scaling (a 1920×1080 screen beside
//!     one at 150 % appears as 2880×1620).
//!   - Otherwise `xrandr --listmonitors` (X11; XWayland on other Wayland desktops), physical pixels at 100 %.
//!
//!   The cursor position isn't available to programs on Wayland, so none is reported.
//! - **macOS:** Quartz's display list, in points ([`Units::Logical`]: a display's bounds), each display's scaling from
//!   its mode's pixels per point. How macOS maps an absolute pointer onto several displays isn't known yet, so the
//!   host software places the cursor on all of them where it may ([`PLACEMENT`]); without it the mapping is
//!   [`Mapping::Desktop`], right for one display.
//!
//! **Placing the cursor** ([`place_cursor`], screen hop's optional placement by the host software, protocol 10:
//! [`crate::placer`]): Windows `SetCursorPos`, in physical pixels, while the input desktop is this program's (not the
//! sign-in, lock or administrator prompt); macOS a mouse-moved event (dragged while a button is down) posted to the HID
//! event tap, once allowed under Accessibility; Linux not at all (Wayland lets no program move the cursor, and an
//! absolute pointer spans the desktop anyway).
//!
//! Every screen the system reports goes into the arrangement, also a monitor cabled to both PCs that shows the other
//! one: that is switched off for hop ([`Screen::off`]) and dragged onto the other PC's screen, and still counts for its
//! PC's desktop. Detected places on the computer are kept ([`Screen::fixed`]) wherever a screen is dragged.

use kvm_proto::ctrl::{Host, MAX_SCREENS, Mapping, Placement, SCREEN_PX, SCREEN_SCALE, Screen, Units};

use crate::fl;

/// One of this computer's screens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub name: String,
    pub w: u16,
    pub h: u16,
    /// Display scaling in percent.
    pub scale: u16,
    /// Top-left corner in this computer's arrangement, in its pixels.
    pub desk: [i32; 2],
    pub main: bool,
}

/// What this computer's system reports: its screens, the main one first, and what its coordinates count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub screens: Vec<Detected>,
    pub units: Units,
}

/// How this computer's system maps an absolute pointer onto its screens.
pub const MAPPING: Mapping = if cfg!(windows) { Mapping::MainScreen } else { Mapping::Desktop };

/// This computer's screens, the main one first, and its units.
pub fn detect() -> Result<Detection, String> {
    let (mut screens, units) = imp::detect()?;
    if screens.is_empty() {
        return Err(fl!("screens-none-found"));
    }
    screens.sort_by_key(|d| (!d.main, d.desk[1], d.desk[0]));
    Ok(Detection { screens, units })
}

/// `host`'s screens with their place on their own computer (`desk`) taken from the arrangement where it isn't fixed
/// ([`Screen::fixed`]: detected or typed in), the inverse of [`place`]: a screen touching one already done sits against
/// the same edge there, and one touching none keeps its offset from the main screen. The arrangement page calls it
/// whenever a screen is dropped, resized, rescaled or added. Before, a screen added by hand kept the place it was given
/// ("right of the PC's others"), whichever side it was dragged to: the KVM then moved that PC's cursor towards the wrong
/// side (2026-11-12). A fixed place stays: where a screen stands on the desk needn't be where its PC has it (a monitor
/// cabled to both PCs lies where the other PC's screen is). With no fixed place at all, the main screen is at 0,0.
pub fn desks(screens: &[Screen; MAX_SCREENS], host: Host, units: Units) -> [Screen; MAX_SCREENS] {
    let mut out = *screens;
    let Some(main) = kvm_core::hop::main_screen(screens, host) else { return out };
    let mine: Vec<usize> =
        (0..MAX_SCREENS).filter(|i| screens[*i].host == Some(host) && screens[*i].is_valid()).collect();
    let mut done: Vec<usize> = mine.iter().copied().filter(|&i| screens[i].fixed).collect();
    if done.is_empty() {
        out[main].desk = [0, 0];
        done.push(main);
    }
    // Two arrangement pixels of slack: edges that were snapped together, or nearly.
    const TOUCH: f32 = 2.0;
    let i16_of = |v: f32| v.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    loop {
        let mut progressed = false;
        for &i in &mine {
            if done.contains(&i) {
                continue;
            }
            let s = out[i];
            let (smin, smax) = world(&s);
            let [sw, sh] = s.desk_size(units);
            let found = done.iter().find_map(|&j| {
                let t = out[j];
                let (tmin, tmax) = world(&t);
                let k = t.desk_per_logical(units);
                let [tw, th] = t.desk_size(units);
                // The other axis must share some length, so that a corner alone doesn't count as touching.
                let shares = |a: usize| smin[a] < tmax[a] && tmin[a] < smax[a];
                let along = |a: usize| t.desk[a] as f32 + (smin[a] - tmin[a]) * k;
                if (smin[0] - tmax[0]).abs() <= TOUCH && shares(1) {
                    Some([t.desk[0] as f32 + tw as f32, along(1)])
                } else if (smax[0] - tmin[0]).abs() <= TOUCH && shares(1) {
                    Some([t.desk[0] as f32 - sw as f32, along(1)])
                } else if (smin[1] - tmax[1]).abs() <= TOUCH && shares(0) {
                    Some([along(0), t.desk[1] as f32 + th as f32])
                } else if (smax[1] - tmin[1]).abs() <= TOUCH && shares(0) {
                    Some([along(0), t.desk[1] as f32 - sh as f32])
                } else {
                    None
                }
            });
            if let Some(d) = found {
                out[i].desk = d.map(i16_of);
                done.push(i);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    // Touching none: its offset from the main screen, in the desktop's units.
    let m = out[main];
    let k = m.desk_per_logical(units);
    for &i in mine.iter().filter(|i| !done.contains(i)) {
        let s = out[i];
        out[i].desk = [0, 1].map(|a| i16_of(m.desk[a] as f32 + (s.at[a] as f32 - m.at[a] as f32) * k));
    }
    out
}

/// Where this computer's cursor is, in its own pixels, where the system tells programs.
pub fn cursor() -> Option<[i32; 2]> {
    imp::cursor()
}

/// The screens this computer's host software places the cursor on, where it may ([`crate::placer`]): Windows those it
/// reaches only with relative motion (all but its main screen), macOS all of them. Linux none.
pub const PLACEMENT: Placement = if cfg!(windows) {
    Placement::Others
} else if cfg!(target_os = "macos") {
    Placement::All
} else {
    Placement::Off
};

/// Why this program can't place the cursor now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unplaceable {
    /// The system doesn't let it: macOS until it is allowed under Privacy & Security, Accessibility.
    NotAllowed,
    /// Another desktop has the input: Windows' sign-in, lock screen or administrator prompt.
    OtherDesktop,
    /// The system refused this once (Windows, with an administrator's window in front).
    Refused,
    /// Not on this system (Linux).
    Unsupported,
}

/// Whether this program may place the cursor now ([`place_cursor`]).
pub fn can_place_cursor() -> Result<(), Unplaceable> {
    imp::can_place()
}

/// Puts this computer's cursor at `at`, in its own pixels (those of the screens' `desk`).
pub fn place_cursor(at: [i32; 2]) -> Result<(), Unplaceable> {
    imp::place(at)
}

/// For the thread that places the cursor, once before it does (Windows: per-monitor DPI aware, so that its places
/// count physical pixels whatever the rest of the process is).
pub fn prepare_to_place() {
    imp::prepare_to_place()
}

/// Asks the person to let this program place the cursor, where the system asks for that (macOS: its own prompt, which
/// opens Privacy & Security, Accessibility); elsewhere nothing.
pub fn ask_to_place() {
    imp::ask_to_place()
}

/// A screen's corners in the arrangement (top-left, bottom-right exclusive).
fn world(s: &Screen) -> ([f32; 2], [f32; 2]) {
    let [w, h] = s.size();
    let min = [s.at[0] as f32, s.at[1] as f32];
    (min, [min[0] + w as f32, min[1] + h as f32])
}

fn bbox(it: impl Iterator<Item = ([f32; 2], [f32; 2])>) -> Option<([f32; 2], [f32; 2])> {
    it.reduce(|(a0, a1), (b0, b1)| ([a0[0].min(b0[0]), a0[1].min(b0[1])], [a1[0].max(b1[0]), a1[1].max(b1[1])]))
}

fn i16_of(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// A screen of `pc` as the system reports it: its place on the computer fixed.
fn screen_of(host: Host, d: &Detected) -> Screen {
    Screen {
        w: d.w.clamp(*SCREEN_PX.start(), *SCREEN_PX.end()),
        h: d.h.clamp(*SCREEN_PX.start(), *SCREEN_PX.end()),
        scale: d.scale.clamp(*SCREEN_SCALE.start(), *SCREEN_SCALE.end()),
        desk: d.desk.map(i16_of),
        fixed: true,
        ..Screen::new(host, 0, 0, [0, 0])
    }
}

/// Where `s` goes in the arrangement beside `t` (both of one PC, already placed): against the edge where they touch on
/// their computer, at the same offset along it. None if they don't touch there. Logical sizes at a fractional scaling
/// are rounded by the system: a pixel of slack.
fn beside(s: &Screen, t: &Screen, units: Units) -> Option<[i16; 2]> {
    let ([sw, sh], [tw, th]) = (s.size(), t.size());
    let ([sdw, sdh], [tdw, tdh]) = (s.desk_size(units), t.desk_size(units));
    let along = |axis: usize| t.at[axis] as f32 + (s.desk[axis] - t.desk[axis]) as f32 / t.desk_per_logical(units);
    let meets = |a: i32, b: i32| (a - b).abs() <= 1;
    let (sx, sy, tx, ty) = (s.desk[0] as i32, s.desk[1] as i32, t.desk[0] as i32, t.desk[1] as i32);
    let at = if meets(sx, tx + tdw) {
        [t.at[0] as f32 + tw as f32, along(1)]
    } else if meets(sx + sdw, tx) {
        [t.at[0] as f32 - sw as f32, along(1)]
    } else if meets(sy, ty + tdh) {
        [along(0), t.at[1] as f32 + th as f32]
    } else if meets(sy + sdh, ty) {
        [along(0), t.at[1] as f32 - sh as f32]
    } else {
        return None;
    };
    Some(at.map(|v| v.round().clamp(-30_000.0, 30_000.0) as i16))
}

/// `found` (this computer's screens, the main one first) in place of the screens `host` had: where those were, or right
/// of the others. Screens that touch in the PC's own arrangement touch here too: Windows' pixels are physical and each
/// screen has its own scaling, so a screen's place follows from the neighbour it touches, not from its position alone.
pub fn place(screens: &[Screen; MAX_SCREENS], host: Host, found: &Detection) -> [Screen; MAX_SCREENS] {
    let units = found.units;
    let mut out = *screens;
    let origin = match bbox(screens.iter().filter(|s| s.host == Some(host)).map(world)) {
        Some((min, _)) => min,
        None => {
            bbox(screens.iter().filter(|s| s.host.is_some()).map(world)).map_or([0.0; 2], |(min, max)| [max[0], min[1]])
        }
    };
    for s in out.iter_mut().filter(|s| s.host == Some(host)) {
        *s = Screen::NONE;
    }
    let mut placed: Vec<Screen> = found.screens.iter().map(|d| screen_of(host, d)).collect();
    // From the first (the main screen) on, each next to one already placed that it touches.
    let mut done = vec![false; placed.len()];
    if let Some(d) = done.first_mut() {
        *d = true;
    }
    for _ in 0..placed.len() {
        for i in 0..placed.len() {
            if done[i] {
                continue;
            }
            if let Some(at) = (0..placed.len()).filter(|j| done[*j]).find_map(|j| beside(&placed[i], &placed[j], units))
            {
                placed[i].at = at;
                done[i] = true;
            }
        }
    }
    // Touching none: at its own position, scaled as the main screen is.
    let k = placed.first().map_or(1.0, |m| 1.0 / m.desk_per_logical(units));
    for (s, d) in placed.iter_mut().zip(&done) {
        if !d {
            s.at = s.desk.map(|v| (v as f32 * k).round() as i16);
        }
    }
    let min = placed.iter().fold([i16::MAX; 2], |m, s| [m[0].min(s.at[0]), m[1].min(s.at[1])]);
    let free: Vec<usize> = (0..MAX_SCREENS).filter(|i| out[*i].host.is_none()).collect();
    for (mut s, slot) in placed.into_iter().zip(free) {
        let at = |a: usize| (origin[a] + (s.at[a] as i32 - min[a] as i32) as f32).round().clamp(-30_000.0, 30_000.0);
        s.at = [at(0) as i16, at(1) as i16];
        out[slot] = s;
    }
    out
}

/// `host`'s screens in the arrangement made what this computer's system reports (`now`), the arrangement otherwise left
/// as it is ("Detect" on the page):
/// - A screen that is there already (the same resolution and scaling; the one at the same place on the computer first)
///   keeps its place here and whether screen hop uses it, and takes the system's place on the computer.
/// - A new one goes beside a screen it touches on the computer.
/// - One the system no longer reports goes.
///
/// With none of them there yet, they are placed afresh ([`place`]).
pub fn merge(screens: &[Screen; MAX_SCREENS], host: Host, now: &Detection) -> [Screen; MAX_SCREENS] {
    let mine: Vec<usize> = (0..MAX_SCREENS).filter(|&i| screens[i].host == Some(host)).collect();
    let same = |s: &Screen, d: &Detected| (s.w, s.h, s.scale) == (d.w, d.h, d.scale);
    let mut slot_of: Vec<Option<usize>> = vec![None; now.screens.len()];
    let mut taken: Vec<usize> = Vec::new();
    for exact in [true, false] {
        for (j, d) in now.screens.iter().enumerate() {
            if slot_of[j].is_some() {
                continue;
            }
            let found = mine.iter().copied().find(|i| {
                !taken.contains(i) && same(&screens[*i], d) && (!exact || screens[*i].desk == d.desk.map(i16_of))
            });
            if let Some(i) = found {
                slot_of[j] = Some(i);
                taken.push(i);
            }
        }
    }
    if taken.is_empty() {
        return place(screens, host, now);
    }
    let mut out = *screens;
    for &i in mine.iter().filter(|i| !taken.contains(i)) {
        out[i] = Screen::NONE;
    }
    for (j, d) in now.screens.iter().enumerate() {
        if let Some(i) = slot_of[j] {
            out[i] = Screen { desk: d.desk.map(i16_of), fixed: true, ..out[i] };
        }
    }
    // The new ones, each beside one of the PC's screens it touches on the computer, or right of the others.
    let mut pending: Vec<Screen> =
        (0..now.screens.len()).filter(|&j| slot_of[j].is_none()).map(|j| screen_of(host, &now.screens[j])).collect();
    while !pending.is_empty() {
        let placed: Vec<Screen> = out.iter().copied().filter(|s| s.host == Some(host)).collect();
        let next = pending.iter().position(|s| placed.iter().any(|t| beside(s, t, now.units).is_some()));
        let (k, at) = match next {
            Some(k) => (k, placed.iter().find_map(|t| beside(&pending[k], t, now.units)).unwrap_or_default()),
            None => {
                let right = bbox(out.iter().filter(|s| s.host.is_some()).map(world))
                    .map_or([0.0; 2], |(min, max)| [max[0], min[1]]);
                (0, right.map(|v| v.round().clamp(-30_000.0, 30_000.0) as i16))
            }
        };
        let s = Screen { at, ..pending.remove(k) };
        let Some(slot) = out.iter().position(|x| x.host.is_none()) else { break };
        out[slot] = s;
    }
    out
}

/// Whether the KVM's copy of `host`'s screens (their resolution, scaling and place on the computer) or of its units
/// (`units`) differs from what this computer's system reports now.
pub fn differs(screens: &[Screen; MAX_SCREENS], host: Host, units: Units, now: &Detection) -> bool {
    let key = |w: u16, h: u16, scale: u16, desk: [i16; 2]| (w, h, scale, desk);
    let mut have: Vec<_> = screens
        .iter()
        .filter(|s| s.host == Some(host) && s.is_valid())
        .map(|s| key(s.w, s.h, s.scale, s.desk))
        .collect();
    let mut want: Vec<_> = now.screens.iter().map(|d| key(d.w, d.h, d.scale, d.desk.map(i16_of))).collect();
    have.sort();
    want.sort();
    have != want || units != now.units
}

/// One line of `xrandr --listmonitors`: ` 0: +*DP-1 2560/597x1440/336+0+0  DP-1`.
pub fn parse_xrandr_line(line: &str) -> Option<Detected> {
    let mut words = line.split_whitespace();
    words.next()?.strip_suffix(':')?.parse::<u32>().ok()?;
    let flags = words.next()?;
    let geometry = words.next()?;
    let name = words.next().unwrap_or(flags.trim_start_matches(['+', '*'])).to_string();
    // X11 positions are never negative: the root window starts at the top-left of all screens.
    let (size, rest) = geometry.split_once('+')?;
    let (x, y) = rest.split_once('+')?;
    let (w, h) = size.split_once('x')?;
    let px = |s: &str| s.split('/').next()?.parse::<u16>().ok();
    Some(Detected {
        name,
        w: px(w)?,
        h: px(h)?,
        scale: 100,
        desk: [x.parse().ok()?, y.parse().ok()?],
        main: flags.contains('*'),
    })
}

/// KDE's own report, `kscreen-doctor -j`: each enabled output's place (logical pixels), mode and scaling. None if it
/// isn't KScreen's JSON.
pub fn parse_kscreen(json: &str) -> Option<Vec<Detected>> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let mut out = Vec::new();
    for o in v.get("outputs")?.as_array()? {
        if o["enabled"].as_bool() != Some(true) || o["connected"].as_bool() == Some(false) {
            continue;
        }
        let mode = o["modes"].as_array().and_then(|m| m.iter().find(|m| m["id"] == o["currentModeId"]));
        let size = mode.map_or(&o["size"], |m| &m["size"]);
        let px = |v: &serde_json::Value| v.as_u64().map(|v| v.min(u16::MAX as u64) as u16);
        let (mut w, mut h) = (px(&size["width"])?, px(&size["height"])?);
        // Turned a quarter (KScreen's rotation Left = 2, Right = 8): the screen stands the other way round.
        if matches!(o["rotation"].as_u64(), Some(2 | 8)) {
            (w, h) = (h, w);
        }
        let pos = |a: &str| o["pos"][a].as_i64().map(|v| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32);
        out.push(Detected {
            name: o["name"].as_str().unwrap_or_default().to_string(),
            w,
            h,
            scale: (o["scale"].as_f64().unwrap_or(1.0) * 100.0).round().clamp(1.0, 1000.0) as u16,
            desk: [pos("x")?, pos("y")?],
            main: o["priority"].as_u64() == Some(1),
        });
    }
    Some(out)
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{Detected, Units, parse_kscreen, parse_xrandr_line};
    use crate::fl;

    /// The screens, and what their coordinates count.
    pub fn detect() -> Result<(Vec<Detected>, Units), String> {
        if kde_wayland()
            && let Some(v) = kscreen()
            && !v.is_empty()
        {
            return Ok((v, Units::Logical));
        }
        Ok((xrandr()?, Units::Physical))
    }

    fn kde_wayland() -> bool {
        std::env::var_os("WAYLAND_DISPLAY").is_some()
            && std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|p| p.eq_ignore_ascii_case("KDE")))
    }

    fn kscreen() -> Option<Vec<Detected>> {
        let out = std::process::Command::new("kscreen-doctor").arg("-j").output().ok()?;
        out.status.success().then(|| parse_kscreen(&String::from_utf8_lossy(&out.stdout))).flatten()
    }

    fn xrandr() -> Result<Vec<Detected>, String> {
        let out = std::process::Command::new("xrandr")
            .arg("--listmonitors")
            .output()
            .map_err(|e| fl!("screens-xrandr-missing", error = e.to_string()))?;
        if !out.status.success() {
            return Err(fl!("screens-xrandr-failed", error = String::from_utf8_lossy(&out.stderr).trim().to_string()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).lines().filter_map(parse_xrandr_line).collect())
    }

    pub fn cursor() -> Option<[i32; 2]> {
        None
    }

    pub use super::unsupported::{ask_to_place, can_place, place, prepare_to_place};
}

/// Placing the cursor where the system has no way to.
#[cfg(not(any(windows, target_os = "macos")))]
mod unsupported {
    use super::Unplaceable;

    pub fn can_place() -> Result<(), Unplaceable> {
        Err(Unplaceable::Unsupported)
    }

    pub fn place(_: [i32; 2]) -> Result<(), Unplaceable> {
        Err(Unplaceable::Unsupported)
    }

    pub fn prepare_to_place() {}

    pub fn ask_to_place() {}
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{LPARAM, POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    };
    use windows_sys::Win32::System::StationsAndDesktops::{
        CloseDesktop, DESKTOP_READOBJECTS, GetUserObjectInformationW, OpenInputDesktop, UOI_NAME,
    };
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetProcessDpiAwarenessContext,
        SetThreadDpiAwarenessContext,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, MONITORINFOF_PRIMARY, SetCursorPos};
    use windows_sys::core::BOOL;

    use super::{Detected, Units, Unplaceable};

    unsafe extern "system" fn collect(h: HMONITOR, _: HDC, _: *mut RECT, list: LPARAM) -> BOOL {
        // SAFETY: `list` is the Vec passed to EnumDisplayMonitors below, alive for the call.
        unsafe { (*(list as *mut Vec<HMONITOR>)).push(h) };
        1
    }

    /// Physical pixels: the window already is per-monitor aware (winit sets it); the command line becomes so here. A
    /// second call fails harmlessly.
    fn aware() {
        // SAFETY: a plain call with a documented constant.
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    }

    /// The screens, in physical pixels.
    pub fn detect() -> Result<(Vec<Detected>, Units), String> {
        aware();
        let mut handles: Vec<HMONITOR> = Vec::new();
        // SAFETY: the callback only pushes into `handles`.
        unsafe {
            EnumDisplayMonitors(
                core::ptr::null_mut(),
                core::ptr::null(),
                Some(collect),
                &mut handles as *mut Vec<HMONITOR> as LPARAM,
            )
        };
        let mut out = Vec::new();
        for h in handles {
            // SAFETY: plain calls with correctly sized and initialised structures.
            unsafe {
                let mut info: MONITORINFOEXW = core::mem::zeroed();
                info.monitorInfo.cbSize = core::mem::size_of::<MONITORINFOEXW>() as u32;
                if GetMonitorInfoW(h, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) == 0 {
                    continue;
                }
                let r = info.monitorInfo.rcMonitor;
                let (mut dx, mut dy) = (96u32, 96u32);
                if GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) != 0 {
                    dx = 96;
                }
                let name = info.szDevice;
                let name = String::from_utf16_lossy(&name[..name.iter().position(|c| *c == 0).unwrap_or(name.len())]);
                out.push(Detected {
                    name: name.trim_start_matches(r"\\.\").to_string(),
                    w: (r.right - r.left).clamp(0, u16::MAX as i32) as u16,
                    h: (r.bottom - r.top).clamp(0, u16::MAX as i32) as u16,
                    scale: (dx * 100 / 96) as u16,
                    desk: [r.left, r.top],
                    main: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                });
            }
        }
        Ok((out, Units::Physical))
    }

    pub fn cursor() -> Option<[i32; 2]> {
        let mut p = POINT { x: 0, y: 0 };
        // SAFETY: writes the one POINT passed.
        (unsafe { GetCursorPos(&mut p) } != 0).then_some([p.x, p.y])
    }

    /// The desktop with the input is this program's ("Default"), not the one of the sign-in, the lock screen or an
    /// administrator prompt ("Winlogon"), which a program of the user may not even open.
    pub fn can_place() -> Result<(), Unplaceable> {
        // SAFETY: plain calls with a buffer of the size passed; the handle is closed again.
        unsafe {
            let d = OpenInputDesktop(0, 0, DESKTOP_READOBJECTS);
            if d.is_null() {
                return Err(Unplaceable::OtherDesktop);
            }
            let mut name = [0u16; 32];
            let mut len = 0u32;
            let ok = GetUserObjectInformationW(d, UOI_NAME, name.as_mut_ptr().cast(), 64, &mut len) != 0;
            CloseDesktop(d);
            let name = String::from_utf16_lossy(&name[..name.iter().position(|c| *c == 0).unwrap_or(name.len())]);
            if ok && name.eq_ignore_ascii_case("Default") { Ok(()) } else { Err(Unplaceable::OtherDesktop) }
        }
    }

    pub fn place(at: [i32; 2]) -> Result<(), Unplaceable> {
        // SAFETY: a plain call.
        if unsafe { SetCursorPos(at[0], at[1]) } != 0 { Ok(()) } else { Err(Unplaceable::Refused) }
    }

    pub fn prepare_to_place() {
        // SAFETY: a plain call with a documented constant; it applies to this thread only.
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    }

    pub fn ask_to_place() {}
}

#[cfg(target_os = "macos")]
mod imp {
    //! Quartz (CoreGraphics) and Accessibility, declared here: plain C functions, no Objective-C.
    use core::ffi::c_void;
    use std::cell::Cell;

    use super::{Detected, Units, Unplaceable};
    use crate::fl;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGSize {
        width: f64,
        height: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    type CGDirectDisplayID = u32;
    type CFTypeRef = *const c_void;
    type CGEventRef = *mut c_void;
    type CGEventSourceRef = *mut c_void;
    type CGDisplayModeRef = *mut c_void;

    /// `CFDictionaryKeyCallBacks` / `CFDictionaryValueCallBacks`: only their address is taken.
    #[repr(C)]
    struct KeyCallBacks([usize; 6]);
    #[repr(C)]
    struct ValueCallBacks([usize; 5]);

    // CGEventSourceStateID
    const COMBINED_SESSION_STATE: i32 = 0;
    const HID_SYSTEM_STATE: i32 = 1;
    // CGEventTapLocation
    const HID_EVENT_TAP: u32 = 0;
    // CGEventType
    const MOUSE_MOVED: u32 = 5;
    const LEFT_MOUSE_DRAGGED: u32 = 6;
    const RIGHT_MOUSE_DRAGGED: u32 = 7;
    const OTHER_MOUSE_DRAGGED: u32 = 27;
    // CGMouseButton
    const LEFT: u32 = 0;
    const RIGHT: u32 = 1;
    const CENTER: u32 = 2;
    // CGEventField
    const MOUSE_DELTA_X: u32 = 4;
    const MOUSE_DELTA_Y: u32 = 5;
    // CGEventSuppressionState, CGEventFilterMask
    const SUPPRESSION_INTERVAL: u32 = 0;
    const REMOTE_MOUSE_DRAG: u32 = 1;
    const PERMIT_ALL_LOCAL: u32 = 0x1 | 0x2 | 0x4;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGGetActiveDisplayList(max: u32, displays: *mut CGDirectDisplayID, count: *mut u32) -> i32;
        fn CGDisplayBounds(display: CGDirectDisplayID) -> CGRect;
        fn CGDisplayIsMain(display: CGDirectDisplayID) -> u32;
        fn CGDisplayIsBuiltin(display: CGDirectDisplayID) -> u32;
        fn CGDisplayMirrorsDisplay(display: CGDirectDisplayID) -> CGDirectDisplayID;
        fn CGDisplayCopyDisplayMode(display: CGDirectDisplayID) -> CGDisplayModeRef;
        fn CGDisplayModeGetPixelWidth(mode: CGDisplayModeRef) -> usize;
        fn CGDisplayModeGetPixelHeight(mode: CGDisplayModeRef) -> usize;
        fn CGDisplayModeRelease(mode: CGDisplayModeRef);
        fn CGEventCreate(source: CGEventSourceRef) -> CGEventRef;
        fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
        fn CGEventSourceCreate(state: i32) -> CGEventSourceRef;
        fn CGEventSourceSetLocalEventsSuppressionInterval(source: CGEventSourceRef, seconds: f64);
        fn CGEventSourceSetLocalEventsFilterDuringSuppressionState(source: CGEventSourceRef, filter: u32, state: u32);
        fn CGEventSourceButtonState(state: i32, button: u32) -> u8;
        fn CGEventSourceFlagsState(state: i32) -> u64;
        fn CGEventCreateMouseEvent(source: CGEventSourceRef, kind: u32, at: CGPoint, button: u32) -> CGEventRef;
        fn CGEventSetFlags(event: CGEventRef, flags: u64);
        fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
        fn CGEventPost(tap: u32, event: CGEventRef);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: CFTypeRef);
        fn CFDictionaryCreate(
            allocator: CFTypeRef,
            keys: *const CFTypeRef,
            values: *const CFTypeRef,
            count: isize,
            key_callbacks: *const KeyCallBacks,
            value_callbacks: *const ValueCallBacks,
        ) -> CFTypeRef;
        static kCFBooleanTrue: CFTypeRef;
        static kCFTypeDictionaryKeyCallBacks: KeyCallBacks;
        static kCFTypeDictionaryValueCallBacks: ValueCallBacks;
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> u8;
        fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> u8;
        static kAXTrustedCheckOptionPrompt: CFTypeRef;
    }

    /// The displays, in points (global display coordinates: the main display's top-left at 0,0, y down), with their
    /// pixels (those of the display mode: a "looks like 1800 × 1169" mode on a 3024-pixel panel draws 3600 at 200 %).
    /// Displays that mirror another are left out.
    pub fn detect() -> Result<(Vec<Detected>, Units), String> {
        let mut ids = [0u32; 16];
        let mut n = 0u32;
        // SAFETY: the list holds `ids.len()` entries; `n` says how many were written.
        if unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) } != 0 {
            return Err(fl!("screens-macos-failed"));
        }
        let mut out = Vec::new();
        for &id in &ids[..(n as usize).min(ids.len())] {
            // SAFETY: plain calls on a display the system just listed; the mode is released again.
            unsafe {
                if CGDisplayMirrorsDisplay(id) != 0 {
                    continue;
                }
                let b = CGDisplayBounds(id);
                let mode = CGDisplayCopyDisplayMode(id);
                let (pw, ph) = if mode.is_null() {
                    (b.size.width as usize, b.size.height as usize)
                } else {
                    let px = (CGDisplayModeGetPixelWidth(mode), CGDisplayModeGetPixelHeight(mode));
                    CGDisplayModeRelease(mode);
                    px
                };
                let scale = (pw as f64 * 100.0 / b.size.width.max(1.0)).round() as u16;
                let name = if CGDisplayIsBuiltin(id) != 0 { "built-in".to_string() } else { format!("display-{id}") };
                out.push(Detected {
                    name,
                    w: pw.min(u16::MAX as usize) as u16,
                    h: ph.min(u16::MAX as usize) as u16,
                    scale,
                    desk: [b.origin.x.round() as i32, b.origin.y.round() as i32],
                    main: CGDisplayIsMain(id) != 0,
                });
            }
        }
        Ok((out, Units::Logical))
    }

    pub fn cursor() -> Option<[i32; 2]> {
        // SAFETY: an event without a source just carries the cursor's location; it is released again.
        unsafe {
            let e = CGEventCreate(core::ptr::null_mut());
            if e.is_null() {
                return None;
            }
            let p = CGEventGetLocation(e);
            CFRelease(e as CFTypeRef);
            Some([p.x.floor() as i32, p.y.floor() as i32])
        }
    }

    pub fn can_place() -> Result<(), Unplaceable> {
        // SAFETY: a plain call.
        if unsafe { AXIsProcessTrusted() } != 0 { Ok(()) } else { Err(Unplaceable::NotAllowed) }
    }

    thread_local! {
        /// The placing thread's event source (with local events let through, below) and where it last put the cursor.
        static SOURCE: Cell<CGEventSourceRef> = const { Cell::new(core::ptr::null_mut()) };
        static LAST: Cell<Option<[i32; 2]>> = const { Cell::new(None) };
    }

    /// An event source whose events don't hold back the real keyboard and mouse: by default the system suppresses
    /// local hardware events for 0.25 s after a synthetic one, and during a synthetic drag, which would swallow the
    /// KVM's own keys and buttons.
    pub fn prepare_to_place() {
        // SAFETY: plain calls; the source lives as long as the thread.
        unsafe {
            let s = CGEventSourceCreate(COMBINED_SESSION_STATE);
            if !s.is_null() {
                CGEventSourceSetLocalEventsSuppressionInterval(s, 0.0);
                CGEventSourceSetLocalEventsFilterDuringSuppressionState(s, PERMIT_ALL_LOCAL, SUPPRESSION_INTERVAL);
                CGEventSourceSetLocalEventsFilterDuringSuppressionState(s, PERMIT_ALL_LOCAL, REMOTE_MOUSE_DRAG);
            }
            SOURCE.with(|c| c.set(s));
        }
    }

    /// A mouse-moved event at `at` (dragged with the button that is down, so that a drag follows), with the modifiers
    /// held now and the motion since the last (for programs that read deltas), posted where the mouse's own go.
    pub fn place(at: [i32; 2]) -> Result<(), Unplaceable> {
        can_place()?;
        let pressed = |b| unsafe { CGEventSourceButtonState(HID_SYSTEM_STATE, b) } != 0;
        let (kind, button) = if pressed(LEFT) {
            (LEFT_MOUSE_DRAGGED, LEFT)
        } else if pressed(RIGHT) {
            (RIGHT_MOUSE_DRAGGED, RIGHT)
        } else if pressed(CENTER) {
            (OTHER_MOUSE_DRAGGED, CENTER)
        } else {
            (MOUSE_MOVED, LEFT)
        };
        let delta = LAST.with(|l| l.replace(Some(at))).map_or([0, 0], |p| [at[0] - p[0], at[1] - p[1]]);
        let source = SOURCE.with(Cell::get);
        // SAFETY: plain calls; the event is released after it was posted.
        unsafe {
            let e = CGEventCreateMouseEvent(source, kind, CGPoint { x: at[0] as f64, y: at[1] as f64 }, button);
            if e.is_null() {
                return Err(Unplaceable::Refused);
            }
            CGEventSetFlags(e, CGEventSourceFlagsState(HID_SYSTEM_STATE));
            CGEventSetIntegerValueField(e, MOUSE_DELTA_X, delta[0] as i64);
            CGEventSetIntegerValueField(e, MOUSE_DELTA_Y, delta[1] as i64);
            CGEventPost(HID_EVENT_TAP, e);
            CFRelease(e as CFTypeRef);
        }
        Ok(())
    }

    /// The system's own prompt to allow this program under Accessibility (once; after that it opens nothing).
    pub fn ask_to_place() {
        // SAFETY: a one-entry dictionary of the documented key, released after the call.
        unsafe {
            let keys = [kAXTrustedCheckOptionPrompt];
            let values = [kCFBooleanTrue];
            let d = CFDictionaryCreate(
                core::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            AXIsProcessTrustedWithOptions(d);
            if !d.is_null() {
                CFRelease(d);
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod imp {
    use super::{Detected, Units};
    use crate::fl;

    pub fn detect() -> Result<(Vec<Detected>, Units), String> {
        Err(fl!("screens-unsupported-os"))
    }

    pub fn cursor() -> Option<[i32; 2]> {
        None
    }

    pub use super::unsupported::{ask_to_place, can_place, place, prepare_to_place};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xrandr_monitors_parse() {
        let d = parse_xrandr_line(" 0: +*DP-1 2560/597x1440/336+0+0  DP-1").unwrap();
        assert_eq!(d, Detected { name: "DP-1".into(), w: 2560, h: 1440, scale: 100, desk: [0, 0], main: true });
        let d = parse_xrandr_line(" 1: +HDMI-A-1 1920/527x1080/296+2560+180  HDMI-A-1").unwrap();
        assert_eq!((d.w, d.h, d.desk, d.main), (1920, 1080, [2560, 180], false));
        assert_eq!(parse_xrandr_line("Monitors: 2"), None);
    }

    fn detection(screens: &[Detected], units: Units) -> Detection {
        Detection { screens: screens.to_vec(), units }
    }

    #[test]
    fn detected_screens_go_where_the_pcs_were_and_touch_as_they_do_there() {
        let mut l = [Screen::NONE; MAX_SCREENS];
        l[0] = Screen::new(Host::Pc1, 1920, 1080, [0, 0]);
        let found = detection(
            &[
                Detected { name: "a".into(), w: 3840, h: 2160, scale: 150, desk: [0, 0], main: true },
                Detected { name: "b".into(), w: 1920, h: 1080, scale: 100, desk: [-1920, 0], main: false },
            ],
            Units::Physical,
        );
        let placed = merge(&l, Host::Pc2, &found);
        // PC2's second screen right of PC1's, its main screen (2560 logical pixels wide at 150 %) touching it; both with
        // their places on PC2 fixed.
        assert_eq!((placed[2].at, placed[2].desk, placed[2].fixed), ([1920, 0], [-1920, 0], true));
        assert_eq!((placed[1].at, placed[1].desk, placed[1].scale), ([3840, 0], [0, 0], 150));
        assert_eq!(placed[0], l[0], "PC1's screen stays");
        // Detected again: where they are now.
        assert_eq!(merge(&placed, Host::Pc2, &found), placed);
        assert!(!differs(&placed, Host::Pc2, Units::Physical, &found));
    }

    #[test]
    fn a_screen_dragged_beside_its_pcs_main_screen_takes_that_side_on_its_computer() {
        // The first board's arrangement (2026-11-12): PC2's second screen dragged left of its main screen, but still
        // recorded where it was added, right of a 1920-wide main screen.
        let mut l = [Screen::NONE; MAX_SCREENS];
        l[0] = Screen::new(Host::Pc1, 1920, 1080, [0, 0]);
        l[1] = Screen::new(Host::Pc2, 3840, 2160, [-3840, -440]);
        l[2] = Screen { desk: [1920, 0], ..Screen::new(Host::Pc2, 2880, 1800, [-6720, -265]) };
        let d = desks(&l, Host::Pc2, Units::Physical);
        assert_eq!((d[1].desk, d[2].desk), ([0, 0], [-2880, 175]));
        assert_eq!(d[0], l[0], "PC1's screen stays");
        // Below the main screen, at 150 %: its x along the main screen's top edge, in the main screen's pixels.
        let mut l2 = l;
        l2[1].scale = 150;
        l2[2].at = [-3840 + 100, -440 + 1440];
        assert_eq!(desks(&l2, Host::Pc2, Units::Physical)[2].desk, [150, 2160]);
        // Away from every other screen of its PC: its offset from the main screen.
        l2[2].at = [5000, 0];
        assert_eq!(desks(&l2, Host::Pc2, Units::Physical)[2].desk, [13260, 660]);
        // A place fixed (detected or typed in) stays where it is, wherever the screen is dragged.
        l2[2].fixed = true;
        assert_eq!(desks(&l2, Host::Pc2, Units::Physical)[2].desk, [1920, 0]);
    }

    #[test]
    fn detected_screens_keep_their_places_on_their_computer() {
        let mut l = [Screen::NONE; MAX_SCREENS];
        l[0] = Screen::new(Host::Pc1, 1920, 1080, [0, 0]);
        let found = detection(
            &[
                Detected { name: "a".into(), w: 3840, h: 2160, scale: 200, desk: [0, 0], main: true },
                Detected { name: "b".into(), w: 2880, h: 1800, scale: 175, desk: [-2880, 360], main: false },
                Detected { name: "c".into(), w: 1920, h: 1080, scale: 100, desk: [3840, 0], main: false },
            ],
            Units::Physical,
        );
        let placed = merge(&l, Host::Pc2, &found);
        assert_eq!(desks(&placed, Host::Pc2, Units::Physical), placed);
        // Without the fixed places, the arrangement gives the same ones back.
        let mut loose = placed;
        loose.iter_mut().for_each(|s| s.fixed = false);
        assert_eq!(desks(&loose, Host::Pc2, Units::Physical), loose);
    }

    /// `kscreen-doctor -j` on the first board's PC1 (KDE Plasma 6.8 on Wayland, 2026-11-12), cut to what is read: the
    /// HD monitor (1920×1080) right of the 4K monitor (3840×2160 at 150 %), and a disabled output.
    const KSCREEN: &str = r#"{"outputs": [
        {"name": "HDMI-A-1", "enabled": true, "connected": true, "priority": 1, "pos": {"x": 2560, "y": 270},
         "scale": 1, "rotation": 1, "currentModeId": "2",
         "modes": [{"id": "1", "size": {"width": 1280, "height": 720}}, {"id": "2", "size": {"width": 1920, "height": 1080}}],
         "size": {"width": 1920, "height": 1080}},
        {"name": "DP-3", "enabled": true, "connected": true, "priority": 2, "pos": {"x": 0, "y": 0},
         "scale": 1.5, "rotation": 1, "currentModeId": "7",
         "modes": [{"id": "7", "size": {"width": 3840, "height": 2160}}], "size": {"width": 3840, "height": 2160}},
        {"name": "DP-11", "enabled": false, "connected": true, "priority": 0, "pos": {"x": 0, "y": 0},
         "scale": 1, "rotation": 1, "currentModeId": "", "modes": [], "size": {"width": 1024, "height": 768}}
    ]}"#;

    fn pc1_now() -> Detection {
        let mut v = parse_kscreen(KSCREEN).unwrap();
        v.sort_by_key(|d| (!d.main, d.desk[1], d.desk[0]));
        detection(&v, Units::Logical)
    }

    #[test]
    fn kde_reports_its_outputs_in_logical_pixels() {
        let now = pc1_now();
        let hd = Detected { name: "HDMI-A-1".into(), w: 1920, h: 1080, scale: 100, desk: [2560, 270], main: true };
        let shared = Detected { name: "DP-3".into(), w: 3840, h: 2160, scale: 150, desk: [0, 0], main: false };
        assert_eq!(now.screens, [hd, shared]);
        // A quarter turn stands the screen the other way round; anything else isn't KScreen's report.
        let turned = KSCREEN.replacen("\"rotation\": 1", "\"rotation\": 8", 1);
        assert_eq!((parse_kscreen(&turned).unwrap()[0].w, parse_kscreen(&turned).unwrap()[0].h), (1080, 1920));
        assert_eq!(parse_kscreen("[]"), None);
    }

    #[test]
    fn a_monitor_cabled_to_both_pcs_goes_in_beside_its_neighbour_and_keeps_its_place_once_laid_on_the_other() {
        // As on the first board's desk before (2026-11-12): PC1 had the HD monitor alone, at 0,0 on PC1; PC2 its 4K
        // monitor.
        let mut l = [Screen::NONE; MAX_SCREENS];
        l[0] = Screen::new(Host::Pc1, 1920, 1080, [0, 720]);
        l[1] = Screen { scale: 150, ..Screen::new(Host::Pc2, 3840, 2160, [-2560, 360]) };
        let now = pc1_now();
        assert!(differs(&l, Host::Pc1, Units::Physical, &now));
        // Detect: the HD monitor stays where it stands and takes KDE's place; the 4K monitor goes in left of it, as on
        // PC1 (2560 logical pixels wide, its top 270 higher), on PC2's screen.
        let next = merge(&l, Host::Pc1, &now);
        assert_eq!(next[0], Screen { desk: [2560, 270], fixed: true, ..l[0] });
        assert_eq!(next[1], l[1]);
        let g = next[2];
        assert_eq!(
            (g.host, g.w, g.scale, g.at, g.desk, g.fixed, g.off),
            (Some(Host::Pc1), 3840, 150, [-2560, 450], [0, 0], true, false)
        );
        assert!(!differs(&next, Host::Pc1, Units::Logical, &now));
        // Turned off for hop and laid exactly on PC2's: its place on PC1 stays, and so does the HD monitor's.
        let mut laid = next;
        laid[2] = Screen { off: true, at: l[1].at, ..g };
        assert_eq!(desks(&laid, Host::Pc1, Units::Logical), laid);
        // Detected again later: nothing moves, nothing comes back on.
        assert_eq!(merge(&laid, Host::Pc1, &now), laid);
        // The 4K monitor unplugged from PC1: it goes, the HD monitor takes its new place on PC1.
        let mut alone = now.clone();
        alone.screens = vec![Detected { desk: [0, 0], ..now.screens[0].clone() }];
        let after = merge(&laid, Host::Pc1, &alone);
        assert_eq!((after[0].desk, after[0].at, after[2].host), ([0, 0], [0, 720], None));
    }

    #[test]
    fn a_screen_added_by_hand_takes_its_place_on_its_computer_from_the_arrangement() {
        let mut l = [Screen::NONE; MAX_SCREENS];
        l[0] = Screen { desk: [2560, 270], fixed: true, ..Screen::new(Host::Pc1, 1920, 1080, [0, 720]) };
        // Right of the HD monitor here: right of it on PC1 too, in logical pixels.
        l[1] = Screen { desk: [4480, 0], ..Screen::new(Host::Pc1, 1280, 1024, [1920, 720]) };
        assert_eq!(desks(&l, Host::Pc1, Units::Logical)[1].desk, [4480, 270]);
    }
}
