# OpenDynKVM logo

The mark is a keycap seen from the side, on its stem, with two ribbons crossing in front of it: one PC, then the other.
The wordmark is Montserrat, "OpenDyn" in Bold and "KVM" in Regular, converted to outlines. Every file here is written
by [`make_logo.py`](make_logo.py). They hold filled paths only, with no strokes, masks or fonts, so any SVG renderer
shows them the same.

| File | What |
|---|---|
| `opendynkvm-logo-on-dark.svg`, `opendynkvm-logo-on-light.svg` | The mark above the wordmark, in white or in dark ink, on a transparent background |
| `opendynkvm-mark-on-dark.svg`, `opendynkvm-mark-on-light.svg` | The mark alone |
| `opendynkvm-wordmark-on-dark.svg`, `opendynkvm-wordmark-on-light.svg` | The wordmark alone; the app's sidebar tints the white one in the theme's text colour |
| `opendynkvm-icon.svg` | The application icon: the white mark on a dark tile with rounded corners, 512 units square |
| `opendynkvm-icon-small.svg` | The same for icons of 48 px and smaller: solid ribbons, a bolder keycap and no slots in the stem |

The colours are white `#ffffff` and dark `#0f1316`. The dark is the ink on light backgrounds and the icons' tile.

## Where the logo is used

- The [README](../README.md) header shows `opendynkvm-logo-on-light.svg`, or `opendynkvm-logo-on-dark.svg` in a dark
  theme.
- `odkvmctl::icon` (`software/odkvmctl/src/icon.rs`) draws these files with `resvg` for:
  - the sidebar of `opendyn-kvm`: the small icon and the wordmark;
  - the window icon: the small icon. Wayland desktops ignore it and show the icon of the installed menu entry (the
    packages', or `cargo xtask desktop`'s for a development build);
  - the tray icon: the small icon with its tile in the active PC's colour, or violet while a Bluetooth host has the
    input. It is grey without a KVM, and a light colour gets the dark ink;
  - the installers (`cargo xtask package`): PNGs from 16 to 256 px (the small icon up to 48 px), the Windows `.ico`,
    and `opendynkvm-icon.svg` as the Linux scalable icon.

## Changing it

Edit `make_logo.py` and run it:

```sh
python3 branding/make_logo.py          # or: --fonts DIR, a directory holding Montserrat
```

Then rebuild the software, which embeds the files.

- **Geometry:** the constants at the top of the script are centre lines and widths in pixels of the drawing the logo
  was traced from: a stroke of 9, and a gap of 9 wherever a line passes behind another.
- **Requirements:** the script needs `shapely`, `fontTools` and Montserrat Bold and Regular (`.otf` or `.ttf`): the
  Fedora package `julietaula-montserrat-fonts`, the Debian package `fonts-montserrat`, or
  [Google Fonts](https://fonts.google.com/specimen/Montserrat).
- **Font license:** Montserrat is under the SIL Open Font License 1.1, which allows its use in logos. No font file is
  embedded or needed to show the files.
