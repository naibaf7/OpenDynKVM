//! This computer's screens, for screen hop's arrangement (software.md §4.14, §9): each screen's resolution, display
//! scaling and place in this computer's own arrangement, and where its cursor is.
//!
//! - **Windows:** the monitor API, in physical pixels (the process is per-monitor DPI aware), scaling from each
//!   monitor's DPI. The main screen is at 0,0, and Windows maps an absolute pointer onto it alone
//!   ([`Mapping::MainScreen`]).
//! - **Linux:** `xrandr --listmonitors` (X11, and XWayland on Wayland desktops), at 100 %. An absolute pointer spans
//!   every screen ([`Mapping::Desktop`]). The cursor position isn't available to programs on Wayland, so none is
//!   reported.

use kvm_proto::ctrl::{MAX_SCREENS, Mapping, Pc, SCREEN_PX, SCREEN_SCALE, Screen};

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

/// How this computer's system maps an absolute pointer onto its screens.
pub const MAPPING: Mapping = if cfg!(windows) { Mapping::MainScreen } else { Mapping::Desktop };

/// This computer's screens, the main one first.
pub fn detect() -> Result<Vec<Detected>, String> {
    let mut v = imp::detect()?;
    if v.is_empty() {
        return Err(fl!("screens-none-found"));
    }
    v.sort_by_key(|d| (!d.main, d.desk[1], d.desk[0]));
    Ok(v)
}

/// `pc`'s screens with their place on their own computer (`desk`) taken from the arrangement, the inverse of
/// [`place`]: its main screen stays at 0,0, a screen touching one already done sits against the same edge there, and
/// one touching none keeps its offset from the main screen. The arrangement page calls it whenever a screen is dropped,
/// resized, rescaled or added. Before, a screen added by hand kept the place it was given ("right of the PC's others"),
/// whichever side it was dragged to: the KVM then moved that PC's cursor towards the wrong side (2026-11-12).
pub fn desks(screens: &[Screen; MAX_SCREENS], pc: Pc) -> [Screen; MAX_SCREENS] {
    let mut out = *screens;
    let Some(main) = kvm_core::hop::main_screen(screens, pc) else { return out };
    let mine: Vec<usize> = (0..MAX_SCREENS).filter(|i| screens[*i].pc == Some(pc) && screens[*i].is_valid()).collect();
    out[main].desk = [0, 0];
    let mut done = vec![main];
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
            let found = done.iter().find_map(|&j| {
                let t = out[j];
                let (tmin, tmax) = world(&t);
                let k = t.scale.max(1) as f32 / 100.0;
                // The other axis must share some length, so that a corner alone doesn't count as touching.
                let shares = |a: usize| smin[a] < tmax[a] && tmin[a] < smax[a];
                let along = |a: usize| t.desk[a] as f32 + (smin[a] - tmin[a]) * k;
                if (smin[0] - tmax[0]).abs() <= TOUCH && shares(1) {
                    Some([t.desk[0] as f32 + t.w as f32, along(1)])
                } else if (smax[0] - tmin[0]).abs() <= TOUCH && shares(1) {
                    Some([t.desk[0] as f32 - s.w as f32, along(1)])
                } else if (smin[1] - tmax[1]).abs() <= TOUCH && shares(0) {
                    Some([along(0), t.desk[1] as f32 + t.h as f32])
                } else if (smax[1] - tmin[1]).abs() <= TOUCH && shares(0) {
                    Some([along(0), t.desk[1] as f32 - s.h as f32])
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
    // Touching none: its offset from the main screen, in the main screen's pixels.
    let m = out[main];
    let k = m.scale.max(1) as f32 / 100.0;
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

/// A screen's corners in the arrangement (top-left, bottom-right exclusive).
fn world(s: &Screen) -> ([f32; 2], [f32; 2]) {
    let [w, h] = s.size();
    let min = [s.at[0] as f32, s.at[1] as f32];
    (min, [min[0] + w as f32, min[1] + h as f32])
}

fn bbox(it: impl Iterator<Item = ([f32; 2], [f32; 2])>) -> Option<([f32; 2], [f32; 2])> {
    it.reduce(|(a0, a1), (b0, b1)| ([a0[0].min(b0[0]), a0[1].min(b0[1])], [a1[0].max(b1[0]), a1[1].max(b1[1])]))
}

/// `found` (this computer's screens, the main one first) in place of the screens `pc` had: where those were, or right
/// of the others. Screens that touch in the PC's own arrangement touch here too: Windows' pixels are physical and each
/// screen has its own scaling, so a screen's place follows from the neighbour it touches, not from its position alone.
pub fn place(screens: &[Screen; MAX_SCREENS], pc: Pc, found: &[Detected]) -> [Screen; MAX_SCREENS] {
    let mut out = *screens;
    let origin = match bbox(screens.iter().filter(|s| s.pc == Some(pc)).map(world)) {
        Some((min, _)) => min,
        None => {
            bbox(screens.iter().filter(|s| s.pc.is_some()).map(world)).map_or([0.0; 2], |(min, max)| [max[0], min[1]])
        }
    };
    for s in out.iter_mut().filter(|s| s.pc == Some(pc)) {
        *s = Screen::NONE;
    }
    let i16_of = |v: i32| v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    let mut placed: Vec<Screen> = found
        .iter()
        .map(|d| Screen {
            pc: Some(pc),
            w: d.w.clamp(*SCREEN_PX.start(), *SCREEN_PX.end()),
            h: d.h.clamp(*SCREEN_PX.start(), *SCREEN_PX.end()),
            scale: d.scale.clamp(*SCREEN_SCALE.start(), *SCREEN_SCALE.end()),
            desk: [i16_of(d.desk[0]), i16_of(d.desk[1])],
            at: [0, 0],
        })
        .collect();
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
            let s = placed[i];
            let along = |t: &Screen, axis: usize| {
                t.at[axis] as f32 + (s.desk[axis] - t.desk[axis]) as f32 * 100.0 / t.scale.max(1) as f32
            };
            let touch = (0..placed.len()).filter(|j| done[*j]).find_map(|j| {
                let t = placed[j];
                let ([sw, sh], [tw, th]) = (s.size(), t.size());
                let edge = |x: i16, len: u16| x as i32 + len as i32;
                if s.desk[0] as i32 == edge(t.desk[0], t.w) {
                    Some([t.at[0] as f32 + tw as f32, along(&t, 1)])
                } else if edge(s.desk[0], s.w) == t.desk[0] as i32 {
                    Some([t.at[0] as f32 - sw as f32, along(&t, 1)])
                } else if s.desk[1] as i32 == edge(t.desk[1], t.h) {
                    Some([along(&t, 0), t.at[1] as f32 + th as f32])
                } else if edge(s.desk[1], s.h) == t.desk[1] as i32 {
                    Some([along(&t, 0), t.at[1] as f32 - sh as f32])
                } else {
                    None
                }
            });
            if let Some(at) = touch {
                placed[i].at = at.map(|v| v.round() as i16);
                done[i] = true;
            }
        }
    }
    // Touching none: at its own position, scaled as the main screen is.
    let k = placed.first().map_or(1.0, |m| 100.0 / m.scale.max(1) as f32);
    for (s, d) in placed.iter_mut().zip(&done) {
        if !d {
            s.at = s.desk.map(|v| (v as f32 * k).round() as i16);
        }
    }
    let min = placed.iter().fold([i16::MAX; 2], |m, s| [m[0].min(s.at[0]), m[1].min(s.at[1])]);
    let free: Vec<usize> = (0..MAX_SCREENS).filter(|i| out[*i].pc.is_none()).collect();
    for (mut s, slot) in placed.into_iter().zip(free) {
        let at = |a: usize| (origin[a] + (s.at[a] as i32 - min[a] as i32) as f32).round().clamp(-30_000.0, 30_000.0);
        s.at = [at(0) as i16, at(1) as i16];
        out[slot] = s;
    }
    out
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

#[cfg(target_os = "linux")]
mod imp {
    use super::{Detected, parse_xrandr_line};
    use crate::fl;

    pub fn detect() -> Result<Vec<Detected>, String> {
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
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{LPARAM, POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    };
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetProcessDpiAwarenessContext,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, MONITORINFOF_PRIMARY};
    use windows_sys::core::BOOL;

    use super::Detected;

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

    pub fn detect() -> Result<Vec<Detected>, String> {
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
        Ok(out)
    }

    pub fn cursor() -> Option<[i32; 2]> {
        let mut p = POINT { x: 0, y: 0 };
        // SAFETY: writes the one POINT passed.
        (unsafe { GetCursorPos(&mut p) } != 0).then_some([p.x, p.y])
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use super::Detected;
    use crate::fl;

    pub fn detect() -> Result<Vec<Detected>, String> {
        Err(fl!("screens-unsupported-os"))
    }

    pub fn cursor() -> Option<[i32; 2]> {
        None
    }
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

    #[test]
    fn detected_screens_go_where_the_pcs_were_and_touch_as_they_do_there() {
        let mut l = [Screen::NONE; MAX_SCREENS];
        l[0] = Screen::new(Pc::Pc1, 1920, 1080, [0, 0]);
        let found = [
            Detected { name: "a".into(), w: 3840, h: 2160, scale: 150, desk: [0, 0], main: true },
            Detected { name: "b".into(), w: 1920, h: 1080, scale: 100, desk: [-1920, 0], main: false },
        ];
        let placed = place(&l, Pc::Pc2, &found);
        // PC2's second screen right of PC1's, its main screen (2560 logical pixels wide at 150 %) touching it.
        assert_eq!((placed[2].at, placed[2].desk), ([1920, 0], [-1920, 0]));
        assert_eq!((placed[1].at, placed[1].desk, placed[1].scale), ([3840, 0], [0, 0], 150));
        assert_eq!(placed[0], l[0], "PC1's screen stays");
        // Detected again: where they are now.
        assert_eq!(place(&placed, Pc::Pc2, &found), placed);
    }

    #[test]
    fn a_screen_dragged_beside_its_pcs_main_screen_takes_that_side_on_its_computer() {
        // The first board's arrangement (2026-11-12): PC2's second screen dragged left of its main screen, but still
        // recorded where it was added, right of a 1920-wide main screen.
        let mut l = [Screen::NONE; MAX_SCREENS];
        l[0] = Screen::new(Pc::Pc1, 1920, 1080, [0, 0]);
        l[1] = Screen::new(Pc::Pc2, 3840, 2160, [-3840, -440]);
        l[2] = Screen { desk: [1920, 0], ..Screen::new(Pc::Pc2, 2880, 1800, [-6720, -265]) };
        let d = desks(&l, Pc::Pc2);
        assert_eq!((d[1].desk, d[2].desk), ([0, 0], [-2880, 175]));
        assert_eq!(d[0], l[0], "PC1's screen stays");
        // Below the main screen, at 150 %: its x along the main screen's top edge, in the main screen's pixels.
        let mut l2 = l;
        l2[1].scale = 150;
        l2[2].at = [-3840 + 100, -440 + 1440];
        assert_eq!(desks(&l2, Pc::Pc2)[2].desk, [150, 2160]);
        // Away from every other screen of its PC: its offset from the main screen.
        l2[2].at = [5000, 0];
        assert_eq!(desks(&l2, Pc::Pc2)[2].desk, [13260, 660]);
    }

    #[test]
    fn detected_screens_keep_their_places_on_their_computer() {
        let mut l = [Screen::NONE; MAX_SCREENS];
        l[0] = Screen::new(Pc::Pc1, 1920, 1080, [0, 0]);
        let found = [
            Detected { name: "a".into(), w: 3840, h: 2160, scale: 200, desk: [0, 0], main: true },
            Detected { name: "b".into(), w: 2880, h: 1800, scale: 175, desk: [-2880, 360], main: false },
            Detected { name: "c".into(), w: 1920, h: 1080, scale: 100, desk: [3840, 0], main: false },
        ];
        let placed = place(&l, Pc::Pc2, &found);
        assert_eq!(desks(&placed, Pc::Pc2), placed);
    }
}
