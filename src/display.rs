//! Resolution and refresh rate. These belong to the desktop, not the monitor, so each desktop has
//! its own backend: KDE (`kscreen-doctor`), GNOME (Mutter's DisplayConfig over `busctl`), Hyprland
//! (`hyprctl`), Sway (`swaymsg`), other wlroots compositors (`wlr-randr`), X11 (`xrandr`) and
//! Windows (`ChangeDisplaySettingsEx`). Without one, `modes` returns `None` and the setting is hidden.

use crate::{Monitor, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct Mode {
    /// Position in `Modes::list`; the choice value.
    pub id: u16,
    pub width: u32,
    pub height: u32,
    pub refresh: f64,
    /// How the backend names this mode when setting it.
    key: String,
}

impl Mode {
    fn new(width: u32, height: u32, refresh: f64, key: String) -> Mode {
        Mode { id: 0, width, height, refresh, key }
    }

    /// `3840x2160@240`, or `@59.94` when the rate isn't a whole number.
    pub fn name(&self) -> String {
        format!("{}x{}@{}", self.width, self.height, self.rate())
    }

    pub fn label(&self) -> String {
        format!("{} \u{d7} {} @ {} Hz", self.width, self.height, self.rate())
    }

    fn rate(&self) -> String {
        if (self.refresh - self.refresh.round()).abs() < 0.05 { format!("{:.0}", self.refresh) } else { format!("{:.2}", self.refresh) }
    }
}

#[derive(Debug)]
pub struct Modes {
    pub current: Option<u16>,
    /// Largest first, then fastest; one entry per name.
    pub list: Vec<Mode>,
}

impl Modes {
    /// Sorts, keeps one mode per name (the current one if it's among them) and numbers them.
    fn new(mut list: Vec<Mode>, current: impl Fn(&Mode) -> bool) -> Modes {
        list.sort_by(|a, b| {
            (b.width * b.height, b.width).cmp(&(a.width * a.height, a.width)).then(b.refresh.total_cmp(&a.refresh))
        });
        // Same name (144.00 and 143.97 Hz are both "@144"): keep the current one, else the first.
        let mut kept: Vec<Mode> = Vec::with_capacity(list.len());
        for mode in list {
            match kept.last_mut() {
                Some(last) if last.name() == mode.name() => {
                    if current(&mode) && !current(last) {
                        *last = mode;
                    }
                }
                _ => kept.push(mode),
            }
        }
        let mut list = kept;
        let current = list.iter().position(&current).and_then(|i| u16::try_from(i).ok());
        for (i, m) in list.iter_mut().enumerate() {
            m.id = u16::try_from(i).unwrap_or(u16::MAX);
        }
        Modes { current, list }
    }
}

pub fn modes(m: &Monitor) -> Option<Modes> {
    imp::modes(m)
}

pub fn set(m: &Monitor, mode: &Mode) -> Result<()> {
    imp::set(m, mode)
}

/// The desktop's adaptive sync (VRR) policy for this monitor: its options as (name, label), and the
/// current one. KDE has Never / Automatic / Always (only for VRR-capable outputs); the wlroots
/// desktops have Off / On.
#[derive(Debug)]
pub struct Sync {
    pub options: &'static [(&'static str, &'static str)],
    pub current: Option<&'static str>,
}

pub fn sync(m: &Monitor) -> Option<Sync> {
    imp::sync(m)
}

pub fn set_sync(m: &Monitor, name: &str) -> Result<()> {
    imp::set_sync(m, name)
}

/// Whether the desktop sends this monitor an HDR signal; `None` when it can't (not HDR-capable, or
/// a desktop without a backend here: GNOME, Hyprland, X11).
pub fn hdr(m: &Monitor) -> Option<bool> {
    imp::hdr(m)
}

pub fn set_hdr(m: &Monitor, on: bool) -> Result<()> {
    imp::set_hdr(m, on)
}

#[cfg(target_os = "linux")]
mod imp {
    use std::process::Command;

    use serde_json::Value;

    use super::{Mode, Modes, Sync};
    use crate::{Edid, Monitor, Result};

    const KDE_POLICIES: &[(&str, &str)] = &[("never", "Never"), ("automatic", "Automatic"), ("always", "Always")];
    const ON_OFF: &[(&str, &str)] = &[("off", "Off"), ("on", "On")];

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Backend {
        Kde,
        Gnome,
        Hyprland,
        Sway,
        Wlroots,
        X11,
    }

    fn backend() -> Option<Backend> {
        let env = |key| std::env::var(key).unwrap_or_default();
        let desktop = env("XDG_CURRENT_DESKTOP").to_ascii_lowercase();
        if desktop.contains("kde") {
            Some(Backend::Kde)
        } else if !env("WAYLAND_DISPLAY").is_empty() {
            Some(if !env("HYPRLAND_INSTANCE_SIGNATURE").is_empty() {
                Backend::Hyprland
            } else if !env("SWAYSOCK").is_empty() {
                Backend::Sway
            } else if desktop.contains("gnome") {
                Backend::Gnome
            } else {
                Backend::Wlroots
            })
        } else if !env("DISPLAY").is_empty() {
            Some(Backend::X11)
        } else {
            None
        }
    }

    pub fn modes(m: &Monitor) -> Option<Modes> {
        match backend()? {
            Backend::Kde => kde(m.connector.as_deref()?),
            Backend::Gnome => gnome::modes(m.connector.as_deref()?),
            Backend::Hyprland => hyprland(m.connector.as_deref()?).map(|(modes, ..)| modes),
            Backend::Sway => sway(m.connector.as_deref()?),
            Backend::Wlroots => wlr_randr(m.connector.as_deref()?),
            Backend::X11 => xrandr::modes(m).map(|(_, modes)| modes),
        }
    }

    pub fn set(m: &Monitor, mode: &Mode) -> Result<()> {
        let connector = m.connector.as_deref().ok_or("unknown connector");
        match backend().ok_or("no supported desktop for changing the resolution")? {
            Backend::Kde => run("kscreen-doctor", &[&format!("output.{}.mode.{}", connector?, mode.key)]),
            Backend::Gnome => gnome::set(connector?, &mode.key),
            Backend::Hyprland => {
                let connector = connector?;
                let (_, place, vrr) = hyprland(connector).ok_or("hyprctl: monitor not found")?;
                let rule = format!("{connector},{},{place},vrr,{}", mode.key, u8::from(vrr));
                let out = output("hyprctl", &["keyword", "monitor", &rule])?;
                if out.trim() == "ok" { Ok(()) } else { Err(format!("hyprctl: {}", out.trim())) }
            }
            Backend::Sway => run("swaymsg", &["output", connector?, "mode", &mode.key]),
            Backend::Wlroots => run("wlr-randr", &["--output", connector?, "--mode", &mode.key]),
            Backend::X11 => {
                let (name, _) = xrandr::modes(m).ok_or("xrandr: output not found")?;
                let size = format!("{}x{}", mode.width, mode.height);
                run("xrandr", &["--output", &name, "--mode", &size, "--rate", &mode.key])
            }
        }
    }

    pub fn sync(m: &Monitor) -> Option<Sync> {
        let connector = m.connector.as_deref()?;
        let on_off = |on: bool| Some(Sync { options: ON_OFF, current: Some(if on { "on" } else { "off" }) });
        match backend()? {
            Backend::Kde => {
                let json = json("kscreen-doctor", &["--json"])?;
                let output = json["outputs"].as_array()?.iter().find(|o| o["name"] == connector && o["enabled"] == true)?;
                // Only VRR-capable outputs have a policy: 0 never, 1 always, 2 automatic.
                let current = match output["vrrPolicy"].as_u64()? {
                    0 => "never",
                    1 => "always",
                    _ => "automatic",
                };
                Some(Sync { options: KDE_POLICIES, current: Some(current) })
            }
            Backend::Hyprland => {
                let json = json("hyprctl", &["monitors", "all", "-j"])?;
                let monitor = json.as_array()?.iter().find(|o| o["name"] == connector)?;
                on_off(monitor["vrr"].as_bool()?)
            }
            Backend::Sway => {
                let json = json("swaymsg", &["-t", "get_outputs", "-r"])?;
                let output = json.as_array()?.iter().find(|o| o["name"] == connector)?;
                on_off(output["adaptive_sync_status"].as_str()? == "enabled")
            }
            Backend::Wlroots => {
                let json = json("wlr-randr", &["--json"])?;
                let output = json.as_array()?.iter().find(|o| o["name"] == connector)?;
                on_off(output["adaptive_sync"].as_bool()?)
            }
            // GNOME offers VRR as separate "variable" modes (experimental); X11 and Windows leave
            // it to the GPU vendor's tools.
            Backend::Gnome | Backend::X11 => None,
        }
    }

    pub fn hdr(m: &Monitor) -> Option<bool> {
        let connector = m.connector.as_deref()?;
        match backend()? {
            // "hdr" is null on outputs that can't do HDR.
            Backend::Kde => {
                let json = json("kscreen-doctor", &["--json"])?;
                let output = json["outputs"].as_array()?.iter().find(|o| o["name"] == connector && o["enabled"] == true)?;
                output["hdr"].as_bool()
            }
            // Sway 1.11+.
            Backend::Sway => {
                let json = json("swaymsg", &["-t", "get_outputs", "-r"])?;
                json.as_array()?.iter().find(|o| o["name"] == connector)?["hdr"].as_bool()
            }
            Backend::Gnome | Backend::Hyprland | Backend::Wlroots | Backend::X11 => None,
        }
    }

    pub fn set_hdr(m: &Monitor, on: bool) -> Result<()> {
        let connector = m.connector.as_deref().ok_or("unknown connector")?;
        match backend().ok_or("no supported desktop for HDR")? {
            Backend::Kde => run("kscreen-doctor", &[&format!("output.{connector}.hdr.{}", if on { "enable" } else { "disable" })]),
            Backend::Sway => run("swaymsg", &["output", connector, "hdr", if on { "on" } else { "off" }]),
            _ => Err("switching HDR isn't supported on this desktop".into()),
        }
    }

    pub fn set_sync(m: &Monitor, name: &str) -> Result<()> {
        let connector = m.connector.as_deref().ok_or("unknown connector")?;
        let on = name == "on";
        match backend().ok_or("no supported desktop for adaptive sync")? {
            Backend::Kde => run("kscreen-doctor", &[&format!("output.{connector}.vrrpolicy.{name}")]),
            Backend::Hyprland => {
                // The monitor rule needs the whole line: current mode, position, scale and transform.
                let (modes, place, _) = hyprland(connector).ok_or("hyprctl: monitor not found")?;
                let mode = modes.current.and_then(|id| modes.list.iter().find(|m| m.id == id)).ok_or("hyprctl: unknown mode")?;
                let rule = format!("{connector},{},{place},vrr,{}", mode.key, u8::from(on));
                let out = output("hyprctl", &["keyword", "monitor", &rule])?;
                if out.trim() == "ok" { Ok(()) } else { Err(format!("hyprctl: {}", out.trim())) }
            }
            Backend::Sway => run("swaymsg", &["output", connector, "adaptive_sync", if on { "on" } else { "off" }]),
            Backend::Wlroots => run("wlr-randr", &["--output", connector, "--adaptive-sync", if on { "enabled" } else { "disabled" }]),
            Backend::Gnome | Backend::X11 => Err("adaptive sync isn't supported on this desktop".into()),
        }
    }

    fn output(program: &str, args: &[&str]) -> Result<String> {
        let out = Command::new(program).args(args).output().map_err(|e| format!("{program}: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let message = if stderr.trim().is_empty() { String::from_utf8_lossy(&out.stdout) } else { stderr };
            Err(format!("{program}: {}", message.trim()))
        }
    }

    fn run(program: &str, args: &[&str]) -> Result<()> {
        output(program, args).map(drop)
    }

    fn json(program: &str, args: &[&str]) -> Option<Value> {
        serde_json::from_str(&output(program, args).ok()?).ok()
    }

    fn uint(v: &Value) -> Option<u32> {
        u32::try_from(v.as_u64()?).ok()
    }

    fn kde(connector: &str) -> Option<Modes> {
        let json = json("kscreen-doctor", &["--json"])?;
        let output = json["outputs"].as_array()?.iter().find(|o| o["name"] == connector && o["enabled"] == true)?;
        let current = output["currentModeId"].as_str().unwrap_or_default().to_string();
        let list = output["modes"].as_array()?.iter().filter_map(|m| {
            let size = &m["size"];
            Some(Mode::new(uint(&size["width"])?, uint(&size["height"])?, m["refreshRate"].as_f64()?, m["id"].as_str()?.into()))
        });
        Some(Modes::new(list.collect(), |m| m.key == current))
    }

    /// Modes, the monitor's `position,scale,transform,N` and its vrr state. `hyprctl keyword monitor`
    /// replaces the whole rule, so a change has to repeat what it isn't changing.
    fn hyprland(connector: &str) -> Option<(Modes, String, bool)> {
        let json = json("hyprctl", &["monitors", "all", "-j"])?;
        let monitor = json.as_array()?.iter().find(|o| o["name"] == connector && o["disabled"] != true)?;
        let current = (uint(&monitor["width"])?, uint(&monitor["height"])?, monitor["refreshRate"].as_f64()?);
        // "3840x2160@240.02Hz"
        let list = monitor["availableModes"].as_array()?.iter().filter_map(|m| {
            let key = m.as_str()?.trim_end_matches("Hz");
            let (size, rate) = key.split_once('@')?;
            let (w, h) = size.split_once('x')?;
            Some(Mode::new(w.parse().ok()?, h.parse().ok()?, rate.parse().ok()?, key.into()))
        });
        let modes = Modes::new(list.collect(), |m| {
            (m.width, m.height) == (current.0, current.1) && (m.refresh - current.2).abs() < 0.01
        });
        let place = format!(
            "{}x{},{},transform,{}",
            monitor["x"].as_i64()?,
            monitor["y"].as_i64()?,
            monitor["scale"].as_f64()?,
            monitor["transform"].as_i64().unwrap_or(0)
        );
        Some((modes, place, monitor["vrr"].as_bool().unwrap_or(false)))
    }

    fn sway(connector: &str) -> Option<Modes> {
        let json = json("swaymsg", &["-t", "get_outputs", "-r"])?;
        let output = json.as_array()?.iter().find(|o| o["name"] == connector && o["active"] == true)?;
        // Refresh in mHz.
        let mode = |m: &Value| {
            let (w, h, mhz) = (uint(&m["width"])?, uint(&m["height"])?, m["refresh"].as_u64()?);
            Some(Mode::new(w, h, mhz as f64 / 1000.0, format!("{w}x{h}@{}.{:03}Hz", mhz / 1000, mhz % 1000)))
        };
        let current = mode(&output["current_mode"])?.key;
        let list = output["modes"].as_array()?.iter().filter_map(mode);
        Some(Modes::new(list.collect(), |m| m.key == current))
    }

    fn wlr_randr(connector: &str) -> Option<Modes> {
        let json = json("wlr-randr", &["--json"])?;
        let output = json.as_array()?.iter().find(|o| o["name"] == connector && o["enabled"] == true)?;
        let mut current = String::new();
        let list = output["modes"].as_array()?.iter().filter_map(|m| {
            let (w, h, hz) = (uint(&m["width"])?, uint(&m["height"])?, m["refresh"].as_f64()?);
            let mode = Mode::new(w, h, hz, format!("{w}x{h}@{hz:.3}Hz"));
            if m["current"] == true {
                current = mode.key.clone();
            }
            Some(mode)
        });
        let list: Vec<_> = list.collect();
        Some(Modes::new(list, |m| m.key == current))
    }

    /// X11 output names differ from the kernel's on NVIDIA, so outputs are matched by EDID first.
    mod xrandr {
        use super::*;

        pub fn modes(m: &Monitor) -> Option<(String, Modes)> {
            let text = output("xrandr", &["--props"]).ok()?;
            parse(&text).into_iter().find(|o| o.edid.is_some() && o.edid == m.edid).or_else(|| {
                let connector = m.connector.as_deref()?;
                parse(&text).into_iter().find(|o| o.name == connector)
            }).map(|o| (o.name, o.modes))
        }

        pub struct Output {
            pub name: String,
            edid: Option<Edid>,
            pub modes: Modes,
        }

        /// `xrandr --props`: an output line, indented properties (the EDID as hex lines), then
        /// `   1920x1080     60.00*+  59.94` mode lines (`*` current). Only active outputs count.
        pub fn parse(text: &str) -> Vec<Output> {
            let mut outputs = Vec::new();
            let mut lines = text.lines().peekable();
            while let Some(line) = lines.next() {
                let mut words = line.split_whitespace();
                let (Some(name), Some("connected")) = (words.next(), words.next()) else { continue };
                // Disconnected or disabled outputs have no geometry ("1920x1080+0+0").
                if !line.split_whitespace().any(|w| w.contains('+') && w.contains('x')) {
                    continue;
                }

                let (mut hex, mut in_edid, mut list, mut current) = (String::new(), false, Vec::new(), None);
                while let Some(line) = lines.next_if(|l| l.starts_with(char::is_whitespace)) {
                    let trimmed = line.trim();
                    if trimmed.starts_with("EDID:") {
                        in_edid = true;
                        continue;
                    }
                    if in_edid && !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
                        hex.push_str(trimmed);
                        continue;
                    }
                    in_edid = false;

                    let mut words = trimmed.split_whitespace();
                    let Some((w, h)) = words.next().and_then(|s| s.split_once('x')) else { continue };
                    let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.parse::<u32>()) else { continue }; // also skips "1920x1080i"
                    for rate in words {
                        let key = rate.trim_end_matches(['*', '+']);
                        let Ok(hz) = key.parse::<f64>() else { continue };
                        if rate.contains('*') {
                            current = Some((w, h, key.to_string()));
                        }
                        list.push(Mode::new(w, h, hz, key.into()));
                    }
                }

                let bytes: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()).collect();
                let modes = Modes::new(list, |m| current.as_ref().is_some_and(|(w, h, k)| (m.width, m.height, &m.key) == (*w, *h, k)));
                outputs.push(Output { name: name.into(), edid: Edid::parse(&bytes), modes });
            }
            outputs
        }
    }

    /// Mutter's org.gnome.Mutter.DisplayConfig, through `busctl` (JSON output, typed arguments).
    mod gnome {
        use super::*;

        const DEST: [&str; 3] = ["org.gnome.Mutter.DisplayConfig", "/org/gnome/Mutter/DisplayConfig", "org.gnome.Mutter.DisplayConfig"];
        /// ApplyMonitorsConfig method: 2 = persistent.
        const PERSISTENT: &str = "2";

        fn state() -> Option<Value> {
            let mut args = vec!["--user", "--json=short", "call"];
            args.extend(DEST);
            args.push("GetCurrentState");
            json("busctl", &args).map(|mut v| v["data"].take())
        }

        /// GetCurrentState monitor: ((connector, vendor, product, serial), modes, props).
        fn monitor<'a>(state: &'a Value, connector: &str) -> Option<&'a Value> {
            state[1].as_array()?.iter().find(|m| m[0][0] == connector)
        }

        fn is_current(mode: &Value) -> bool {
            mode[6]["is-current"]["data"] == true
        }

        /// Mode: (id, width, height, refresh, preferred scale, supported scales, props).
        fn mode(m: &Value) -> Option<Mode> {
            Some(Mode::new(uint(&m[1])?, uint(&m[2])?, m[3].as_f64()?, m[0].as_str()?.into()))
        }

        pub fn modes(connector: &str) -> Option<Modes> {
            let state = state()?;
            let modes = monitor(&state, connector)?[1].as_array()?;
            let current = modes.iter().find(|m| is_current(m)).and_then(|m| m[0].as_str()).unwrap_or_default().to_string();
            Some(Modes::new(modes.iter().filter_map(mode).collect(), |m| m.key == current))
        }

        /// Re-applies the current layout with one monitor's mode changed. Monitors right of or
        /// below it move by the change in its size, so the layout stays connected.
        pub fn set(connector: &str, key: &str) -> Result<()> {
            let state = state().ok_or("Mutter DisplayConfig: no reply")?;
            let serial = state[0].as_u64().ok_or("Mutter DisplayConfig: bad reply")?;
            // 1 = logical (sizes divided by scale), 2 = physical.
            let logical = state[3]["layout-mode"]["data"].as_u64().unwrap_or(1) == 1;
            let modes = monitor(&state, connector).and_then(|m| m[1].as_array()).ok_or("monitor not found")?;
            let old = modes.iter().find(|m| is_current(m)).ok_or("no current mode")?;
            let new = modes.iter().find(|m| m[0] == key).ok_or("unknown mode")?;
            let logicals = state[2].as_array().ok_or("Mutter DisplayConfig: bad reply")?;

            // Logical monitor: (x, y, scale, transform, primary, monitors, props).
            let target = logicals.iter().find(|l| l[5].as_array().is_some_and(|ms| ms.iter().any(|m| m[0] == connector)));
            let target = target.ok_or("monitor isn't in the layout")?;
            let scale = target[2].as_f64().unwrap_or(1.0);
            // Keep the scale if the new mode supports it, else take the mode's preferred one.
            let new_scale = if new[5].as_array().is_some_and(|s| s.iter().any(|v| v.as_f64().is_some_and(|v| (v - scale).abs() < 1e-6))) {
                scale
            } else {
                new[4].as_f64().unwrap_or(1.0)
            };
            let rotated = target[3].as_u64().is_some_and(|t| t % 2 == 1);
            let size = |m: &Value, scale: f64| {
                let (w, h) = (m[1].as_f64().unwrap_or(0.0), m[2].as_f64().unwrap_or(0.0));
                let (w, h) = if rotated { (h, w) } else { (w, h) };
                if logical { ((w / scale).round(), (h / scale).round()) } else { (w, h) }
            };
            let (old_w, old_h) = size(old, scale);
            let (new_w, new_h) = size(new, new_scale);
            let (tx, ty) = (target[0].as_f64().unwrap_or(0.0), target[1].as_f64().unwrap_or(0.0));

            let mut args: Vec<String> = ["--user", "call"].iter().map(|s| s.to_string()).collect();
            args.extend(DEST.iter().map(|s| s.to_string()));
            args.extend(["ApplyMonitorsConfig".into(), "uua(iiduba(ssa{sv}))a{sv}".into(), serial.to_string(), PERSISTENT.into()]);
            args.push(logicals.len().to_string());
            for l in logicals {
                let is_target = std::ptr::eq(l, target);
                let (mut x, mut y) = (l[0].as_f64().unwrap_or(0.0), l[1].as_f64().unwrap_or(0.0));
                if !is_target {
                    if x >= tx + old_w {
                        x += new_w - old_w;
                    }
                    if y >= ty + old_h {
                        y += new_h - old_h;
                    }
                }
                let scale = if is_target { new_scale } else { l[2].as_f64().unwrap_or(1.0) };
                args.extend([x.to_string(), y.to_string(), scale.to_string(), l[3].to_string(), l[4].to_string()]);
                let members = l[5].as_array().map(Vec::as_slice).unwrap_or_default();
                args.push(members.len().to_string());
                for member in members {
                    let name = member[0].as_str().unwrap_or_default();
                    let mode = if name == connector {
                        key.to_string()
                    } else {
                        let modes = monitor(&state, name).and_then(|m| m[1].as_array());
                        let current = modes.and_then(|ms| ms.iter().find(|m| is_current(m)));
                        current.and_then(|m| m[0].as_str()).ok_or(format!("{name}: no current mode"))?.to_string()
                    };
                    args.extend([name.to_string(), mode, "0".into()]);
                }
            }
            args.push("0".into());
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            run("busctl", &args)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::xrandr;

        #[test]
        fn xrandr_props() {
            let text = "Screen 0: minimum 16 x 16, current 3840 x 2160, maximum 32767 x 32767
DP-0 connected primary 3840x2160+0+0 (normal left inverted right x axis y axis) 697mm x 392mm
\tEDID:
\t\t00ffffffffffff000000000000000000
\t\t00000000000000000000000000000000
\tnon-desktop: 0
\t\tsupported: 0, 1
   3840x2160     60.00 +  240.02*  144.00    59.94
   1920x1080     60.00    59.94    50.00
   1920x1080i    60.00    50.00
DP-1 disconnected (normal left inverted right x axis y axis)
";
            let outputs = xrandr::parse(text);
            assert_eq!(outputs.len(), 1);
            let modes = &outputs[0].modes;
            let names: Vec<_> = modes.list.iter().map(|m| m.name()).collect();
            assert_eq!(names, ["3840x2160@240", "3840x2160@144", "3840x2160@60", "3840x2160@59.94", "1920x1080@60", "1920x1080@59.94", "1920x1080@50"]);
            assert_eq!(modes.current, Some(0));
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Devices::Display::{
        DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE,
        DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE, DisplayConfigGetDeviceInfo,
        DisplayConfigSetDeviceInfo,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        CDS_UPDATEREGISTRY, ChangeDisplaySettingsExW, DEVMODEW, DISP_CHANGE_SUCCESSFUL, DM_DISPLAYFREQUENCY, DM_PELSHEIGHT,
        DM_PELSWIDTH, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW,
    };

    use super::{Mode, Modes, Sync};
    use crate::{Monitor, Result};

    /// Windows' "Use HDR" (advanced color) for the display path behind this monitor.
    pub fn hdr(m: &Monitor) -> Option<bool> {
        let (adapter, id) = crate::windows::target_of(m.id.split('#').next()?)?;
        // SAFETY: a zeroed request with its header filled in, as DisplayConfigGetDeviceInfo expects.
        unsafe {
            let mut info: DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO = zeroed();
            info.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO;
            info.header.size = size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32;
            info.header.adapterId = adapter;
            info.header.id = id;
            if DisplayConfigGetDeviceInfo(&mut info.header) != 0 {
                return None;
            }
            // Bit 0: advancedColorSupported, bit 1: advancedColorEnabled.
            let bits = info.Anonymous.value;
            (bits & 1 != 0).then_some(bits & 2 != 0)
        }
    }

    pub fn set_hdr(m: &Monitor, on: bool) -> Result<()> {
        let gdi = m.id.split('#').next().unwrap_or_default();
        let (adapter, id) = crate::windows::target_of(gdi).ok_or("display path not found")?;
        // SAFETY: as above, for DisplayConfigSetDeviceInfo.
        let status = unsafe {
            let mut state: DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE = zeroed();
            state.header.r#type = DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE;
            state.header.size = size_of::<DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE>() as u32;
            state.header.adapterId = adapter;
            state.header.id = id;
            state.Anonymous.value = u32::from(on);
            DisplayConfigSetDeviceInfo(&state.header)
        };
        if status == 0 { Ok(()) } else { Err(format!("DisplayConfigSetDeviceInfo failed ({status})")) }
    }

    /// G-SYNC lives in the NVIDIA Control Panel; Windows has no public API for it.
    pub fn sync(_: &Monitor) -> Option<Sync> {
        None
    }

    pub fn set_sync(_: &Monitor, _: &str) -> Result<()> {
        Err("adaptive sync isn't supported here".into())
    }

    /// `\\.\DISPLAY1` (the id drops a `#n` suffix for several physical monitors on one output).
    fn device(m: &Monitor) -> Vec<u16> {
        let name = m.id.split('#').next().unwrap_or_default();
        name.encode_utf16().chain([0]).collect()
    }

    fn setting(device: &[u16], index: u32) -> Option<DEVMODEW> {
        // SAFETY: a zeroed DEVMODEW with dmSize set is what EnumDisplaySettingsW expects.
        unsafe {
            let mut dm: DEVMODEW = zeroed();
            dm.dmSize = size_of::<DEVMODEW>() as u16;
            (EnumDisplaySettingsW(device.as_ptr(), index, &mut dm) != 0).then_some(dm)
        }
    }

    pub fn modes(m: &Monitor) -> Option<Modes> {
        let device = device(m);
        let current = setting(&device, ENUM_CURRENT_SETTINGS)?;
        let current = (current.dmPelsWidth, current.dmPelsHeight, current.dmDisplayFrequency);
        let list = (0..)
            .map_while(|i| setting(&device, i))
            .filter(|dm| dm.dmBitsPerPel == 32)
            .map(|dm| Mode::new(dm.dmPelsWidth, dm.dmPelsHeight, f64::from(dm.dmDisplayFrequency), dm.dmDisplayFrequency.to_string()));
        Some(Modes::new(list.collect(), |m| (m.width, m.height, m.key.as_str()) == (current.0, current.1, &current.2.to_string())))
    }

    pub fn set(m: &Monitor, mode: &Mode) -> Result<()> {
        let device = device(m);
        let mut dm = setting(&device, ENUM_CURRENT_SETTINGS).ok_or("display not found")?;
        dm.dmPelsWidth = mode.width;
        dm.dmPelsHeight = mode.height;
        dm.dmDisplayFrequency = mode.key.parse().map_err(|_| "bad mode")?;
        dm.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY;
        // SAFETY: device is NUL-terminated and dm a valid DEVMODEW.
        let result = unsafe { ChangeDisplaySettingsExW(device.as_ptr(), &dm, null_mut(), CDS_UPDATEREGISTRY, null()) };
        if result == DISP_CHANGE_SUCCESSFUL { Ok(()) } else { Err(format!("ChangeDisplaySettingsEx failed ({result})")) }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod imp {
    use super::{Mode, Modes, Sync};
    use crate::{Monitor, Result};

    pub fn hdr(_: &Monitor) -> Option<bool> {
        None
    }

    pub fn set_hdr(_: &Monitor, _: bool) -> Result<()> {
        Err("switching HDR isn't supported here".into())
    }

    pub fn sync(_: &Monitor) -> Option<Sync> {
        None
    }

    pub fn set_sync(_: &Monitor, _: &str) -> Result<()> {
        Err("adaptive sync isn't supported here".into())
    }

    pub fn modes(_: &Monitor) -> Option<Modes> {
        None
    }

    pub fn set(_: &Monitor, _: &Mode) -> Result<()> {
        Err("changing the resolution isn't supported here".into())
    }
}

#[cfg(test)]
mod tests {
    use super::Mode;

    #[test]
    fn names() {
        let mode = |refresh| Mode::new(3840, 2160, refresh, String::new());
        assert_eq!(mode(240.016).name(), "3840x2160@240");
        assert_eq!(mode(59.997).label(), "3840 \u{d7} 2160 @ 60 Hz");
        assert_eq!(mode(59.939).name(), "3840x2160@59.94");
    }

    #[test]
    fn keeps_the_current_of_same_named_modes() {
        let mode = |refresh, key: &str| Mode::new(3840, 2160, refresh, key.into());
        let modes = super::Modes::new(vec![mode(144.0, "a"), mode(143.97, "b"), mode(60.0, "c")], |m| m.key == "b");
        assert_eq!(modes.list.len(), 2);
        assert_eq!(modes.current, Some(0));
        assert_eq!(modes.list[0].key, "b");
    }
}
