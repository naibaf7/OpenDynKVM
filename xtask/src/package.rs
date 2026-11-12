//! `cargo xtask package [--out DIR] [--native]`: installers for the host software, odkvmctl and opendyn-kvm (software.md
//! §9). Written to `dist/` unless `--out` says otherwise:
//!
//! | File | For |
//! |---|---|
//! | `opendyn-kvm_<version>_amd64.deb` | Debian, Ubuntu |
//! | `opendyn-kvm-<version>-1.x86_64.rpm` | Fedora, openSUSE (needs `rpmbuild`) |
//! | `OpenDynKVM-Setup-<version>.exe` | Windows 10 and 11, per user, no administrator rights (needs `makensis`) |
//!
//! - The Linux packages hold both programs, the menu entry, the icons, the udev rule (reloaded on install and
//!   removal) and the `i2c-dev` module setting for DDC/CI (loaded on install and at boot). Starting at login is each
//!   user's choice, in the GUI.
//! - The Linux binaries are built in a Debian 12 container (podman or docker; the image is made on first use from
//!   `software/odkvmctl/packaging/build-env.sh`) with the host's rustup toolchain mounted, so they need no newer glibc
//!   than Debian 12 and the current Ubuntu releases have. `--native` builds them on this machine; the packages then
//!   need its glibc.
//! - Every package carries the licenses of all it was compiled from (`THIRD-PARTY-LICENSES.txt`, from `cargo metadata`),
//!   RMK's notice for the parts of `kvm-vial` adapted from it, and the license of the window's text font (Adwaita Sans,
//!   SIL OFL 1.1, embedded in opendyn-kvm).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use anyhow::{Context, Result, bail, ensure};

const PACKAGE: &str = "opendyn-kvm";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const WINDOWS: &str = "x86_64-pc-windows-gnu";
const LINUX: &str = "x86_64-unknown-linux-gnu";
const BASE_IMAGE: &str = "docker.io/library/debian:bookworm";
const IMAGE: &str = "opendyn-kvm-build:bookworm";
const ICON_SIZES: [usize; 6] = [16, 32, 48, 64, 128, 256];
const ICO_SIZES: [usize; 4] = [16, 32, 48, 256];
const SUMMARY: &str = "Control software for the OpenDynKVM USB KVM switch";
const DESCRIPTION: &str = "odkvmctl (command line) and opendyn-kvm (window and tray icon) switch PCs, set the port modes \
and settings and update the firmware of the OpenDynKVM, an open-source USB KVM switch for two PCs, and can switch the \
monitors' inputs along (DDC/CI). Includes the udev rule that gives the logged-in user access to the KVM and to the \
monitors.";

fn cargo() -> OsString {
    env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

/// A program on `PATH`.
fn tool(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?).map(|d| d.join(name)).find(|p| p.is_file())
}

fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd.status().with_context(|| format!("running {cmd:?}"))?;
    ensure!(status.success(), "{cmd:?} failed ({status})");
    Ok(())
}

fn output(cmd: &mut Command) -> Result<String> {
    let out = cmd.output().with_context(|| format!("running {cmd:?}"))?;
    ensure!(out.status.success(), "{cmd:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub struct Options {
    pub out: PathBuf,
    pub native: bool,
}

/// The firmware files every package carries (`cargo xtask dist` makes them; `odkvmctl::firmware` finds them): the
/// image for updates over a PC port, and the two UF2s for the boot ROM's drive.
const FIRMWARE: [&str; 3] =
    [odkvmctl::firmware::UPDATE_BIN, odkvmctl::firmware::UPDATE_UF2, odkvmctl::firmware::FIRST_UF2];

/// The release's firmware files from `dist`, by name.
fn firmware_files(dist: &Path) -> Result<Vec<(&'static str, Vec<u8>)>> {
    FIRMWARE
        .iter()
        .map(|name| Ok((*name, fs::read(dist.join(name)).with_context(|| format!("{name}: run cargo xtask dist"))?)))
        .collect()
}

pub fn package(root: &Path, opt: &Options) -> Result<()> {
    let stage = opt.out.join("stage");
    if stage.exists() {
        fs::remove_dir_all(&stage)?;
    }
    fs::create_dir_all(&stage)?;
    let icons: Vec<(usize, Vec<u8>)> = ICON_SIZES.iter().map(|&s| Ok((s, png(s)?))).collect::<Result<_>>()?;

    let firmware = firmware_files(&opt.out)?;
    let linux_bin = build_linux(root, opt.native)?;
    let tree = stage.join("linux");
    let installed = linux_tree(root, &linux_bin, &icons, &firmware, &tree)?;
    let deb = opt.out.join(format!("{PACKAGE}_{VERSION}_amd64.deb"));
    fs::write(&deb, deb_package(&tree, installed, glibc_needed(&tree.join("usr/bin/opendyn-kvm")))?)?;
    println!("{}", deb.display());
    match tool("rpmbuild") {
        Some(rpmbuild) => println!("{}", rpm_package(&rpmbuild, &tree, &stage, &opt.out)?.display()),
        None => println!("rpmbuild not found: no .rpm"),
    }

    match tool("makensis") {
        Some(makensis) => {
            let win_bin = build(root, Some(WINDOWS))?;
            let installer = windows_installer(root, &makensis, &win_bin, &icons, &firmware, &stage, &opt.out)?;
            println!("{}", installer.display());
        }
        None => println!("makensis not found (Fedora: mingw32-nsis, Debian: nsis): no Windows installer"),
    }
    Ok(())
}

/// `cargo build --release -p odkvmctl` on this machine; returns the directory with the binaries.
fn build(root: &Path, target: Option<&str>) -> Result<PathBuf> {
    let mut cmd = Command::new(cargo());
    cmd.current_dir(root).args(["build", "--release", "--locked", "-p", "odkvmctl"]);
    if let Some(t) = target {
        cmd.args(["--target", t]);
    }
    run(&mut cmd)?;
    let dir = root.join("target");
    Ok(match target {
        Some(t) => dir.join(t).join("release"),
        None => dir.join("release"),
    })
}

/// The Linux binaries, from the Debian 12 container unless `native` (or no container engine is installed).
fn build_linux(root: &Path, native: bool) -> Result<PathBuf> {
    let engine = ["podman", "docker"].into_iter().find(|e| tool(e).is_some());
    let Some(engine) = engine.filter(|_| !native) else {
        if !native {
            println!("neither podman nor docker found: building natively, the packages need this machine's glibc");
        }
        return build(root, None);
    };
    let exists = Command::new(engine).args(["image", "inspect", IMAGE]).output().is_ok_and(|o| o.status.success());
    if !exists {
        // `run` + `commit` rather than `build`: rootless `podman build` steps get no DNS on some hosts.
        let setup = fs::read_to_string(root.join("software/odkvmctl/packaging/build-env.sh"))?;
        let name = format!("opendyn-kvm-build-setup-{}", std::process::id());
        run(Command::new(engine)
            .args(["run", "--name", &name, "--network=host", "--security-opt", "label=disable", BASE_IMAGE])
            .args(["sh", "-c", &setup]))?;
        let committed = run(Command::new(engine).args(["commit", &name, IMAGE]));
        let _ = Command::new(engine).args(["rm", &name]).output();
        committed?;
    }
    let home = PathBuf::from(env::var_os("HOME").context("HOME is not set")?);
    let rustup = env::var_os("RUSTUP_HOME").map_or_else(|| home.join(".rustup"), PathBuf::from);
    let cargo_home = env::var_os("CARGO_HOME").map_or_else(|| home.join(".cargo"), PathBuf::from);
    let mount = |host: &Path, at: &str, ro: bool| {
        let mut v = OsString::from(host);
        v.push(format!(":{at}{}", if ro { ":ro" } else { "" }));
        v
    };
    let mut cmd = Command::new(engine);
    // No SELinux relabelling of the mounted trees (Fedora): the container is just a build environment.
    cmd.args(["run", "--rm", "--network=host", "--security-opt", "label=disable"]);
    if engine == "podman" {
        cmd.arg("--userns=keep-id");
    } else {
        let id = |flag| output(Command::new("id").arg(flag)).map(|s| s.trim().to_string());
        cmd.args(["--user", &format!("{}:{}", id("-u")?, id("-g")?)]);
    }
    cmd.arg("-v").arg(mount(root, "/src", false));
    cmd.arg("-v").arg(mount(&rustup, "/rustup", true));
    cmd.arg("-v").arg(mount(&cargo_home, "/cargo", false));
    for (k, v) in [
        ("RUSTUP_HOME", "/rustup"),
        ("CARGO_HOME", "/cargo"),
        ("CARGO_TARGET_DIR", "/src/target/bookworm"),
        ("PATH", "/cargo/bin:/usr/bin:/bin"),
        ("HOME", "/tmp"),
    ] {
        cmd.args(["-e", &format!("{k}={v}")]);
    }
    if let Ok(t) = env::var("RUSTUP_TOOLCHAIN") {
        cmd.args(["-e", &format!("RUSTUP_TOOLCHAIN={t}")]);
    }
    cmd.args(["-w", "/src", IMAGE, "cargo", "build", "--release", "--locked", "-p", "odkvmctl", "--target", LINUX]);
    run(&mut cmd)?;
    Ok(root.join("target/bookworm").join(LINUX).join("release"))
}

/// The newest `GLIBC_x.y` symbol version an ELF file needs (`objdump -T`).
fn glibc_needed(elf: &Path) -> Option<String> {
    let text = output(Command::new("objdump").arg("-T").arg(elf)).ok()?;
    let mut best: Option<(u32, u32)> = None;
    for v in text.split("GLIBC_").skip(1) {
        let v: String = v.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        let mut parts = v.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
        let ver = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
        best = best.max(Some(ver));
    }
    best.map(|(a, b)| format!("{a}.{b}"))
}

/// The application icon as PNG.
fn png(size: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, size as u32, size as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header()?;
    w.write_image_data(&odkvmctl::icon::app_rgba(size))?;
    w.finish()?;
    Ok(out)
}

/// A Windows icon file holding PNG images (Windows Vista and later).
fn ico(images: &[(usize, Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![0, 0, 1, 0];
    out.extend((images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (size, data) in images {
        let s = if *size >= 256 { 0 } else { *size as u8 };
        out.extend([s, s, 0, 0]);
        out.extend(1u16.to_le_bytes()); // colour planes
        out.extend(32u16.to_le_bytes()); // bits per pixel
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((offset as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, data) in images {
        out.extend(data);
    }
    out
}

fn put(dir: &Path, rel: &str, data: &[u8], mode: u32) -> Result<u64> {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(&path, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    Ok(data.len() as u64)
}

/// A binary, without its symbols when a `strip` is at hand.
fn stripped(bin: &Path, strip: &str) -> Result<Vec<u8>> {
    let tmp = env::temp_dir().join(format!("xtask-strip-{}", std::process::id()));
    fs::copy(bin, &tmp).with_context(|| format!("{} (was it built?)", bin.display()))?;
    if let Some(s) = tool(strip) {
        run(Command::new(s).arg(&tmp))?;
    }
    let data = fs::read(&tmp)?;
    fs::remove_file(&tmp)?;
    Ok(data)
}

/// The licenses of everything compiled into odkvmctl for `target`, our own crates aside.
fn third_party(root: &Path, target: &str) -> Result<String> {
    let text = output(Command::new(cargo()).current_dir(root).args([
        "metadata",
        "--format-version",
        "1",
        "--locked",
        "--filter-platform",
        target,
    ]))?;
    let meta: serde_json::Value = serde_json::from_str(&text)?;
    let members: BTreeSet<&str> = meta["workspace_members"]
        .as_array()
        .context("no workspace_members")?
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    let packages: BTreeMap<&str, &serde_json::Value> = meta["packages"]
        .as_array()
        .context("no packages")?
        .iter()
        .filter_map(|p| Some((p["id"].as_str()?, p)))
        .collect();
    let nodes: BTreeMap<&str, &serde_json::Value> = meta["resolve"]["nodes"]
        .as_array()
        .context("no resolve graph")?
        .iter()
        .filter_map(|n| Some((n["id"].as_str()?, n)))
        .collect();
    let start =
        packages.values().find(|p| p["name"] == "odkvmctl").and_then(|p| p["id"].as_str()).context("odkvmctl")?;
    // Normal (not build or dev) dependencies only: what ends up in the binaries.
    let mut seen = BTreeSet::from([start]);
    let mut todo = vec![start];
    while let Some(id) = todo.pop() {
        for dep in nodes.get(id).and_then(|n| n["deps"].as_array()).into_iter().flatten() {
            let normal = dep["dep_kinds"].as_array().is_some_and(|k| k.iter().any(|k| k["kind"].is_null()));
            if let (true, Some(d)) = (normal, dep["pkg"].as_str())
                && seen.insert(d)
            {
                todo.push(d);
            }
        }
    }
    let mut list: Vec<&serde_json::Value> =
        seen.iter().filter(|id| !members.contains(**id)).filter_map(|id| packages.get(id).copied()).collect();
    list.sort_by_key(|p| (p["name"].as_str().unwrap_or(""), p["version"].as_str().unwrap_or("")));
    // Most crates carry the same Apache or MIT text: each distinct text is printed once, numbered.
    let mut texts: Vec<(String, Vec<String>)> = Vec::new();
    let mut crates = String::new();
    for p in list {
        let name = format!("{} {}", p["name"].as_str().unwrap_or("?"), p["version"].as_str().unwrap_or("?"));
        let license = p["license"].as_str().unwrap_or("see its license file");
        let dir = Path::new(p["manifest_path"].as_str().unwrap_or("")).parent().map(Path::to_path_buf);
        let mut files: Vec<PathBuf> = dir
            .and_then(|d| fs::read_dir(d).ok())
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|f| {
                let n = f.file_name().unwrap_or_default().to_string_lossy().to_ascii_uppercase();
                f.is_file() && ["LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE"].iter().any(|s| n.starts_with(s))
            })
            .collect();
        files.sort();
        let mut refs = Vec::new();
        for f in files {
            let text = fs::read_to_string(&f)?.trim().to_string();
            let n = match texts.iter().position(|(t, _)| *t == text) {
                Some(n) => n,
                None => {
                    texts.push((text, Vec::new()));
                    texts.len() - 1
                }
            };
            texts[n].1.push(name.clone());
            refs.push(format!("[{}]", n + 1));
        }
        let refs = if refs.is_empty() { "no license file in the package".to_string() } else { refs.join(" ") };
        crates += &format!("{name} — {license} — {refs}\n");
    }
    let mut out = format!(
        "Third-party software compiled into odkvmctl and opendyn-kvm ({target}), with its licenses.\n\
         Generated by `cargo xtask package` from Cargo.lock. Each crate lists the license texts it ships, by number.\n\n\
         {crates}"
    );
    for (n, (text, users)) in texts.iter().enumerate() {
        out += &format!("\n{}\n[{}] used by {}\n\n{text}\n", "=".repeat(100), n + 1, users.join(", "));
    }
    Ok(out)
}

/// Our own license files, RMK's notice, the text font's license and the third-party list, for `target`.
fn licenses(root: &Path, target: &str) -> Result<Vec<(&'static str, Vec<u8>)>> {
    Ok(vec![
        ("MIT.txt", fs::read(root.join("LICENSES/MIT.txt"))?),
        ("Apache-2.0.txt", fs::read(root.join("LICENSES/Apache-2.0.txt"))?),
        ("LICENSE-RMK-MIT.txt", fs::read(root.join("crates/kvm-vial/LICENSE-RMK-MIT"))?),
        ("OFL-1.1-AdwaitaSans.txt", fs::read(root.join("software/odkvmctl/fonts/OFL.txt"))?),
        ("THIRD-PARTY-LICENSES.txt", third_party(root, target)?.into_bytes()),
    ])
}

/// The icons of the Linux packages, as paths under `share/` with their contents: a PNG per size, then the SVG.
fn icon_files(pngs: &[(usize, Vec<u8>)]) -> Vec<(String, &[u8])> {
    pngs.iter()
        .map(|(s, data)| (format!("icons/hicolor/{s}x{s}/apps/{PACKAGE}.png"), data.as_slice()))
        .chain([(format!("icons/hicolor/scalable/apps/{PACKAGE}.svg"), odkvmctl::icon::ICON_SVG.as_bytes())])
        .collect()
}

/// Marks the menu entry `cargo xtask desktop` writes, so that `--remove` only removes its own.
const DEV_MARK: &str = "X-OpenDynKVM-Development=true";

/// The packages' menu entry, for the development build at `bin`.
fn dev_entry(template: &str, bin: &Path) -> String {
    let mut out = String::new();
    for line in template.lines() {
        match line.split_once('=') {
            Some(("Name", _)) => out.push_str("Name=OpenDynKVM (development build)"),
            Some(("Exec", _)) => {
                out.push_str(&format!("Exec={}", odkvmctl::autostart::exec_quote(&bin.to_string_lossy())))
            }
            _ => out.push_str(line),
        }
        out.push('\n');
    }
    out + DEV_MARK + "\n"
}

/// `cargo xtask desktop [--remove]`: the menu entry and the icons, as the Linux packages install them, for the
/// development build (`target/debug/opendyn-kvm`), in `$XDG_DATA_HOME` (`~/.local/share`). On Wayland the desktop takes
/// a window's icon from the menu entry named after its app id (winit doesn't send one), so a build that isn't installed
/// shows a generic icon without this. `--remove` takes them out again.
pub fn desktop(root: &Path, remove: bool) -> Result<()> {
    let share = env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .context("neither XDG_DATA_HOME nor HOME is set")?;
    let entry = share.join(format!("applications/{PACKAGE}.desktop"));
    let ours = |text: &str| text.lines().any(|l| l == DEV_MARK);
    if let Ok(text) = fs::read_to_string(&entry) {
        ensure!(ours(&text), "{} is not the development build's: left alone", entry.display());
    }
    if remove {
        let names: Vec<(usize, Vec<u8>)> = ICON_SIZES.iter().map(|&s| (s, Vec::new())).collect();
        for path in [entry].into_iter().chain(icon_files(&names).into_iter().map(|(rel, _)| share.join(rel))) {
            match fs::remove_file(&path) {
                Ok(()) => println!("removed {}", path.display()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| path.display().to_string()),
            }
        }
    } else {
        let target = env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
        let bin = target.join("debug").join(PACKAGE);
        if !bin.is_file() {
            println!("note: {} isn't built yet (cargo build -p odkvmctl)", bin.display());
        }
        let template = fs::read_to_string(root.join("software/odkvmctl/packaging/opendyn-kvm.desktop"))?;
        put(&share, &format!("applications/{PACKAGE}.desktop"), dev_entry(&template, &bin).as_bytes(), 0o644)?;
        let pngs: Vec<(usize, Vec<u8>)> = ICON_SIZES.iter().map(|&s| Ok((s, png(s)?))).collect::<Result<_>>()?;
        for (rel, data) in icon_files(&pngs) {
            put(&share, &rel, data, 0o644)?;
        }
        println!("{} and the icons in {}", entry.display(), share.join("icons/hicolor").display());
        println!("start opendyn-kvm again to see the icon");
    }
    // Let the desktop notice now rather than at the next login (KDE's service cache, the MIME cache elsewhere).
    for (tool_name, args) in [
        ("kbuildsycoca6", vec![]),
        ("kbuildsycoca5", vec![]),
        ("update-desktop-database", vec![share.join("applications").into_os_string()]),
    ] {
        if let Some(t) = tool(tool_name) {
            let _ = Command::new(t)
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
    Ok(())
}

/// The installed files of the Linux packages under `tree`; returns their total size in bytes.
fn linux_tree(
    root: &Path,
    bin: &Path,
    icons: &[(usize, Vec<u8>)],
    firmware: &[(&str, Vec<u8>)],
    tree: &Path,
) -> Result<u64> {
    let mut size = 0;
    for b in ["odkvmctl", "opendyn-kvm"] {
        size += put(tree, &format!("usr/bin/{b}"), &stripped(&bin.join(b), "strip")?, 0o755)?;
    }
    for (name, data) in firmware {
        size += put(tree, &format!("usr/share/{PACKAGE}/firmware/{name}"), data, 0o644)?;
    }
    let pkg = root.join("software/odkvmctl");
    size += put(
        tree,
        "usr/share/applications/opendyn-kvm.desktop",
        &fs::read(pkg.join("packaging/opendyn-kvm.desktop"))?,
        0o644,
    )?;
    for (rel, data) in icon_files(icons) {
        size += put(tree, &format!("usr/share/{rel}"), data, 0o644)?;
    }
    size += put(
        tree,
        "usr/lib/udev/rules.d/70-opendyn-kvm.rules",
        &fs::read(pkg.join("udev/70-opendyn-kvm.rules"))?,
        0o644,
    )?;
    size += put(
        tree,
        "usr/lib/modules-load.d/opendyn-kvm.conf",
        b"# DDC/CI to the monitors (opendyn-kvm)\ni2c-dev\n",
        0o644,
    )?;
    for (name, data) in licenses(root, LINUX)? {
        size += put(tree, &format!("usr/share/licenses/{PACKAGE}/{name}"), &data, 0o644)?;
    }
    let copyright = format!(
        "OpenDynKVM host software (odkvmctl, opendyn-kvm) {VERSION}\nCopyright 2026 Fabian Tschopp\n\
         License: MIT OR Apache-2.0 (/usr/share/licenses/{PACKAGE}/MIT.txt, Apache-2.0.txt)\n\n\
         Parts of kvm-vial are adapted from RMK, Copyright 2024 HaoboGu, MIT (LICENSE-RMK-MIT.txt).\n\
         The window's text font is Adwaita Sans, Copyright The Inter Project Authors and others, SIL OFL 1.1\n\
         (OFL-1.1-AdwaitaSans.txt).\n\
         Third-party crates and their licenses: THIRD-PARTY-LICENSES.txt.\n"
    );
    size += put(tree, &format!("usr/share/doc/{PACKAGE}/copyright"), copyright.as_bytes(), 0o644)?;
    Ok(size)
}

/// Reloads udev so that the rule applies to a KVM plugged in already and to the monitors' I2C buses.
const UDEV_RELOAD: &str = "if command -v udevadm >/dev/null 2>&1; then\n\
    udevadm control --reload-rules || true\n\
    udevadm trigger --subsystem-match=hidraw --subsystem-match=i2c-dev || true\nfi\n";

/// Loads i2c-dev now (modules-load.d loads it from the next boot on), before the rule is applied to its devices.
const MODPROBE: &str = "modprobe i2c-dev 2>/dev/null || true\n";

/// `DEBFULLNAME` / `DEBEMAIL` (the Debian tools' variables), else git's user.
fn maintainer() -> String {
    let get = |var: &str, git: &str| {
        env::var(var).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| {
            output(Command::new("git").args(["config", git])).map(|s| s.trim().to_string()).unwrap_or_default()
        })
    };
    match (get("DEBFULLNAME", "user.name"), get("DEBEMAIL", "user.email")) {
        (n, e) if !n.is_empty() && !e.is_empty() => format!("{n} <{e}>"),
        _ => "OpenDynKVM <opendyn-kvm@localhost>".into(),
    }
}

fn mtime() -> u64 {
    env::var("SOURCE_DATE_EPOCH").ok().and_then(|s| s.parse().ok()).unwrap_or_else(|| {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
    })
}

/// A gzip-compressed tar of `entries` (path, data, mode), owned by root, directories first.
fn tar_gz(entries: &[(String, Vec<u8>, u32)]) -> Result<Vec<u8>> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    let mut tar = tar::Builder::new(gz);
    let t = mtime();
    let header = |kind, size, mode| {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(kind);
        h.set_size(size);
        h.set_mode(mode);
        h.set_uid(0);
        h.set_gid(0);
        h.set_mtime(t);
        let _ = h.set_username("root");
        let _ = h.set_groupname("root");
        h
    };
    let mut dirs = BTreeSet::new();
    for (path, ..) in entries {
        let mut p = Path::new(path).parent();
        while let Some(d) = p.filter(|d| !d.as_os_str().is_empty() && d != &Path::new(".")) {
            dirs.insert(d.to_path_buf());
            p = d.parent();
        }
    }
    for d in &dirs {
        let mut h = header(tar::EntryType::Directory, 0, 0o755);
        tar.append_data(&mut h, d, std::io::empty())?;
    }
    for (path, data, mode) in entries {
        let mut h = header(tar::EntryType::Regular, data.len() as u64, *mode);
        tar.append_data(&mut h, path, data.as_slice())?;
    }
    Ok(tar.into_inner()?.finish()?)
}

/// An `ar` archive as `.deb` files use it: plain member names, mode 100644, owned by root.
fn ar(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = b"!<arch>\n".to_vec();
    let t = mtime();
    for (name, data) in members {
        let header = format!("{name:<16}{t:<12}{:<6}{:<6}{:<8}{:<10}`\n", 0, 0, 100644, data.len());
        debug_assert_eq!(header.len(), 60);
        out.extend(header.as_bytes());
        out.extend(*data);
        if data.len() % 2 == 1 {
            out.push(b'\n');
        }
    }
    out
}

/// Every file under `dir`, as (path relative to it with a leading "./", data, mode).
fn files_under(dir: &Path) -> Result<Vec<(String, Vec<u8>, u32)>> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in fs::read_dir(&d)? {
            let p = e?.path();
            if p.is_dir() {
                todo.push(p);
                continue;
            }
            #[cfg(unix)]
            let mode = std::os::unix::fs::PermissionsExt::mode(&fs::metadata(&p)?.permissions()) & 0o7777;
            #[cfg(not(unix))]
            let mode = 0o644;
            let rel = p.strip_prefix(dir)?.to_string_lossy().replace('\\', "/");
            out.push((format!("./{rel}"), fs::read(&p)?, mode));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn deb_package(tree: &Path, installed: u64, glibc: Option<String>) -> Result<Vec<u8>> {
    let depends = glibc.map_or_else(|| "libc6".to_string(), |v| format!("libc6 (>= {v})"));
    let description: String = textwrap(DESCRIPTION, 78).iter().map(|l| format!(" {l}\n")).collect();
    let control = format!(
        "Package: {PACKAGE}\nVersion: {VERSION}\nSection: utils\nPriority: optional\nArchitecture: amd64\n\
         Maintainer: {}\nInstalled-Size: {}\nDepends: {depends}\nRecommends: libegl1, libxkbcommon0\n\
         Description: {SUMMARY}\n{description}",
        maintainer(),
        installed.div_ceil(1024),
    );
    let postinst = format!("#!/bin/sh\nset -e\n{MODPROBE}{UDEV_RELOAD}");
    let postrm = format!("#!/bin/sh\nset -e\n{UDEV_RELOAD}");
    let control_tar = tar_gz(&[
        ("./control".into(), control.into_bytes(), 0o644),
        ("./postinst".into(), postinst.into_bytes(), 0o755),
        ("./postrm".into(), postrm.into_bytes(), 0o755),
    ])?;
    let data_tar = tar_gz(&files_under(tree)?)?;
    Ok(ar(&[("debian-binary", b"2.0\n"), ("control.tar.gz", &control_tar), ("data.tar.gz", &data_tar)]))
}

fn textwrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split_whitespace() {
        let line = lines.last_mut().unwrap();
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            lines.push(word.to_string());
        } else {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    lines
}

fn rpm_package(rpmbuild: &Path, tree: &Path, stage: &Path, out: &Path) -> Result<PathBuf> {
    let top = stage.join("rpmbuild");
    fs::create_dir_all(top.join("SPECS"))?;
    let spec = format!(
        "%global debug_package %{{nil}}\n%global __os_install_post %{{nil}}\n%define _build_id_links none\n\
         %global source_date_epoch_from_changelog 0\n\
         Name: {PACKAGE}\nVersion: {VERSION}\nRelease: 1\nSummary: {SUMMARY}\nLicense: MIT OR Apache-2.0\n\
         BuildArch: x86_64\n\n%description\n{}\n\n%install\nmkdir -p %{{buildroot}}\ncp -a {}/. %{{buildroot}}/\n\n\
         %post\n{MODPROBE}{UDEV_RELOAD}\n%postun\n{UDEV_RELOAD}\n%files\n%defattr(-,root,root,-)\n/usr/bin/odkvmctl\n\
         /usr/bin/opendyn-kvm\n/usr/share/applications/opendyn-kvm.desktop\n\
         /usr/share/icons/hicolor/*/apps/opendyn-kvm.png\n/usr/share/icons/hicolor/scalable/apps/opendyn-kvm.svg\n\
         /usr/share/{PACKAGE}/firmware\n\
         /usr/lib/udev/rules.d/70-opendyn-kvm.rules\n\
         /usr/lib/modules-load.d/opendyn-kvm.conf\n\
         /usr/share/licenses/{PACKAGE}\n/usr/share/doc/{PACKAGE}\n",
        textwrap(DESCRIPTION, 78).join("\n"),
        tree.display(),
    );
    let spec_path = top.join("SPECS").join(format!("{PACKAGE}.spec"));
    fs::write(&spec_path, spec)?;
    let def = |k: &str, v: &dyn std::fmt::Display| format!("{k} {v}");
    run(Command::new(rpmbuild)
        .arg("-bb")
        .arg("--quiet")
        .args(["--define", &def("_topdir", &top.display())])
        .args(["--define", &def("_rpmdir", &out.display())])
        .args(["--define", "_build_name_fmt %%{NAME}-%%{VERSION}-%%{RELEASE}.%%{ARCH}.rpm"])
        .arg(&spec_path))?;
    let rpm = out.join(format!("{PACKAGE}-{VERSION}-1.x86_64.rpm"));
    ensure!(rpm.exists(), "rpmbuild did not write {}", rpm.display());
    Ok(rpm)
}

fn windows_installer(
    root: &Path,
    makensis: &Path,
    bin: &Path,
    icons: &[(usize, Vec<u8>)],
    firmware: &[(&str, Vec<u8>)],
    stage: &Path,
    out: &Path,
) -> Result<PathBuf> {
    let dir = stage.join("windows");
    for b in ["odkvmctl.exe", "opendyn-kvm.exe"] {
        put(&dir, b, &stripped(&bin.join(b), "x86_64-w64-mingw32-strip")?, 0o755)?;
    }
    for (name, data) in firmware {
        put(&dir, &format!("firmware/{name}"), data, 0o644)?;
    }
    let ico_images: Vec<(usize, Vec<u8>)> = icons.iter().filter(|(s, _)| ICO_SIZES.contains(s)).cloned().collect();
    put(&dir, "opendyn-kvm.ico", &ico(&ico_images), 0o644)?;
    for (name, data) in licenses(root, WINDOWS)? {
        // CRLF, so that Notepad shows the lines.
        let text = String::from_utf8_lossy(&data).replace("\r\n", "\n").replace('\n', "\r\n");
        put(&dir, &format!("licenses/{name}"), text.as_bytes(), 0o644)?;
    }
    let exe = out.join(format!("OpenDynKVM-Setup-{VERSION}.exe"));
    let define = |k: &str, v: &dyn std::fmt::Display| format!("-D{k}={v}");
    run(Command::new(makensis)
        .arg("-V2")
        .arg(define("VERSION", &VERSION))
        .arg(define("SRC", &dir.display()))
        .arg(define("OUTFILE", &exe.display()))
        .arg(root.join("software/odkvmctl/packaging/installer.nsi")))?;
    if !exe.exists() {
        bail!("makensis did not write {}", exe.display());
    }
    let _ = std::io::stdout().flush();
    Ok(exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ar_members_are_even_aligned_with_60_byte_headers() {
        let a = ar(&[("debian-binary", b"2.0\n"), ("x", b"abc")]);
        assert!(a.starts_with(b"!<arch>\ndebian-binary   "));
        assert_eq!(&a[8 + 58..8 + 60], b"`\n");
        assert_eq!(&a[8 + 60..8 + 64], b"2.0\n");
        // "abc" is padded to an even length.
        assert_eq!(a.len(), 8 + 60 + 4 + 60 + 4);
    }

    #[test]
    fn the_development_entry_runs_the_debug_build_under_the_packages_icon() {
        let template = include_str!("../../software/odkvmctl/packaging/opendyn-kvm.desktop");
        let e = dev_entry(template, Path::new("/home/a b/kvm/target/debug/opendyn-kvm"));
        assert!(e.contains("\nExec=\"/home/a b/kvm/target/debug/opendyn-kvm\"\n"), "{e}");
        assert!(e.contains("\nName=OpenDynKVM (development build)\n"), "{e}");
        // The icon name and the window class stay the app id, which is how a Wayland desktop finds the icon.
        assert!(e.contains("\nIcon=opendyn-kvm\n") && e.contains("\nStartupWMClass=opendyn-kvm\n"), "{e}");
        assert!(e.ends_with(&format!("\n{DEV_MARK}\n")));
        let pngs = [(16, vec![1]), (256, vec![2])];
        let files: Vec<String> = icon_files(&pngs).into_iter().map(|(p, _)| p).collect();
        assert_eq!(
            files,
            [
                "icons/hicolor/16x16/apps/opendyn-kvm.png",
                "icons/hicolor/256x256/apps/opendyn-kvm.png",
                "icons/hicolor/scalable/apps/opendyn-kvm.svg"
            ]
        );
    }

    #[test]
    fn ico_directory_points_at_each_png() {
        let images = vec![(16, png(16).unwrap()), (256, png(256).unwrap())];
        let f = ico(&images);
        assert_eq!(&f[..6], &[0, 0, 1, 0, 2, 0]);
        assert_eq!((f[6], f[6 + 16]), (16, 0), "256 px is written as 0");
        let off = u32::from_le_bytes(f[6 + 12..6 + 16].try_into().unwrap()) as usize;
        assert_eq!(&f[off..off + 8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn description_wraps() {
        assert!(textwrap(DESCRIPTION, 78).iter().all(|l| l.len() <= 78));
    }
}
