//! Icons from the logo (`branding/`, written by `branding/make_logo.py`): the application icon for the window, menus,
//! shortcuts and installers (`cargo xtask package`), the tray icon (the logo on a tile in the active target's colour: the
//! active PC's, violet while a Bluetooth host has the input, grey when disconnected) and the wordmark for the window's
//! sidebar.

use kvm_proto::ctrl::{Pc, Rgb};
use resvg::{tiny_skia, usvg};

/// The default PC colours (the config's defaults), used where no KVM is asked.
pub const PC_COLORS: [Rgb; 2] = [Rgb::new(0, 90, 255), Rgb::new(255, 110, 0)];

pub const SIZE: usize = 32;

/// The application icon: the mark in white on a dark rounded tile.
pub const ICON_SVG: &str = include_str!("../../../branding/opendynkvm-icon.svg");
/// The same with the bolder mark drawn for small sizes, up to `SMALL_MAX` pixels.
const ICON_SMALL_SVG: &str = include_str!("../../../branding/opendynkvm-icon-small.svg");
const SMALL_MAX: usize = 48;
/// The wordmark in white, to be tinted.
const WORDMARK_SVG: &str = include_str!("../../../branding/opendynkvm-wordmark-on-dark.svg");
/// The icons' tile and ink colours, as the files write them.
const TILE: &str = "#0f1316";
const INK: &str = "#ffffff";

/// The tray icon's tile without a KVM.
pub const DISCONNECTED: Rgb = Rgb::new(110, 110, 110);

/// The tile colour for a PC: its configured colour.
pub fn pc_tile(pc: Pc, colors: [Rgb; 2]) -> Rgb {
    colors[pc.index()]
}

/// The tray icon on a tile of colour `tile`: RGBA, row-major, `SIZE` × `SIZE`.
pub fn rgba(tile: Rgb) -> Vec<u8> {
    // A light colour gets the dark ink.
    let luma = 0.2126 * f32::from(tile.r) + 0.7152 * f32::from(tile.g) + 0.0722 * f32::from(tile.b);
    let ink = if luma > 170.0 { TILE } else { INK };
    let svg = ICON_SMALL_SVG.replace(INK, "#ink").replace(TILE, &hex(tile)).replace("#ink", ink);
    render(&svg, SIZE, SIZE)
}

/// The application icon, `size` × `size` RGBA; up to `SMALL_MAX` pixels with the bolder mark.
pub fn app_rgba(size: usize) -> Vec<u8> {
    icon_rgba(size, size <= SMALL_MAX)
}

/// The application icon drawn at `size` pixels; `small`: with the bolder mark, for an icon that shows small (on a
/// high-resolution display too).
pub fn icon_rgba(size: usize, small: bool) -> Vec<u8> {
    render(if small { ICON_SMALL_SVG } else { ICON_SVG }, size, size)
}

/// The wordmark in white, `height` pixels high: (width, height, RGBA).
pub fn wordmark_rgba(height: usize) -> (usize, usize, Vec<u8>) {
    let size = tree(WORDMARK_SVG).size();
    let width = (height as f32 * size.width() / size.height()).round() as usize;
    (width, height, render(WORDMARK_SVG, width, height))
}

fn tree(svg: &str) -> usvg::Tree {
    usvg::Tree::from_str(svg, &usvg::Options::default()).expect("the logo's files are valid SVG")
}

/// `svg` stretched to `w` × `h` pixels, RGBA (not premultiplied).
fn render(svg: &str, w: usize, h: usize) -> Vec<u8> {
    let tree = tree(svg);
    let mut pixmap = tiny_skia::Pixmap::new(w as u32, h as u32).expect("an icon is at least a pixel");
    let size = tree.size();
    let scale = tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height());
    resvg::render(&tree, scale, &mut pixmap.as_mut());
    pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}

fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

/// ARGB32 in network byte order, as StatusNotifierItem wants it.
#[cfg(target_os = "linux")]
pub fn argb(tile: Rgb) -> Vec<u8> {
    rgba(tile).as_chunks::<4>().0.iter().flat_map(|p| [p[3], p[0], p[1], p[2]]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(rgba: &[u8], size: usize, x: usize, y: usize) -> [u8; 4] {
        rgba[(y * size + x) * 4..][..4].try_into().unwrap()
    }

    #[test]
    fn the_tray_icon_is_the_logo_on_the_active_pcs_colour() {
        let pc2 = rgba(pc_tile(Pc::Pc2, PC_COLORS));
        assert_eq!(pc2.len(), SIZE * SIZE * 4);
        // Outside the tile's rounded corner, on the tile, and the white mark.
        assert_eq!(px(&pc2, SIZE, 0, 0)[3], 0);
        assert_eq!(px(&pc2, SIZE, 3, 3), [255, 110, 0, 255]);
        let white = pc2.as_chunks::<4>().0.iter().filter(|p| **p == [255, 255, 255, 255]).count();
        assert!(white > SIZE * SIZE / 8, "the mark is drawn: {white} white pixels");
        // Disconnected: grey; a light colour gets dark ink.
        assert_eq!(px(&rgba(DISCONNECTED), SIZE, 3, 3), [110, 110, 110, 255]);
        let light = rgba(pc_tile(Pc::Pc1, [Rgb::new(255, 230, 0), PC_COLORS[1]]));
        assert!(light.as_chunks::<4>().0.contains(&[0x0f, 0x13, 0x16, 255]));
    }

    #[test]
    fn the_app_icon_and_the_wordmark_render() {
        for size in [16, 48, 64, 256] {
            let icon = app_rgba(size);
            assert_eq!(px(&icon, size, size / 2, 0), [0x0f, 0x13, 0x16, 255], "{size} px: the tile");
        }
        let (w, h, word) = wordmark_rgba(20);
        assert!(w > 7 * h && w < 9 * h, "{w} × {h}");
        assert!(word.as_chunks::<4>().0.iter().any(|p| p[3] == 255));
    }
}
