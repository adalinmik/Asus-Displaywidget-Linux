use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use asusdisplay::lightbar::{self, Effect};
use asusdisplay::{Feature, Kind, Monitor, Reading, Support};
use serde::Serialize;
use tauri::{Manager, State};

/// Behind one lock because a monitor answers one DDC/CI request at a time.
#[derive(Default)]
struct Monitors(Mutex<Vec<Monitor>>);

impl Monitors {
    fn lock(&self) -> MutexGuard<'_, Vec<Monitor>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Serialize)]
struct Setting {
    #[serde(flatten)]
    feature: Feature,
    #[serde(flatten)]
    reading: Option<Reading>,
    error: Option<String>,
}

fn setting(m: &mut Monitor, feature: Feature) -> Setting {
    let (reading, error) = match feature.kind {
        Kind::Action => (None, None),
        _ => match m.read(&feature) {
            Ok(r) => (Some(r), None),
            Err(e) => (None, Some(e)),
        },
    };
    Setting { feature, reading, error }
}

#[tauri::command(async)]
fn monitors(state: State<'_, Monitors>) -> Result<Vec<String>, String> {
    let mut list = state.lock();
    *list = asusdisplay::enumerate(&Default::default());
    Ok(list.iter().map(|m| m.connector.as_ref().map_or_else(|| m.model.clone(), |c| format!("{} ({c})", m.model))).collect())
}

#[tauri::command(async)]
fn status(state: State<'_, Monitors>, index: usize) -> Result<Vec<Setting>, String> {
    let mut list = state.lock();
    let m = list.get_mut(index).ok_or("monitor not found")?;
    Ok(m.features(false).into_iter().map(|f| setting(m, f)).collect())
}

#[tauri::command(async)]
fn set(state: State<'_, Monitors>, index: usize, key: String, value: String, force: bool) -> Result<Setting, String> {
    let mut list = state.lock();
    let m = list.get_mut(index).ok_or("monitor not found")?;
    let feature = m.feature(&key).ok_or("unknown setting")?;
    m.write(&feature, &value, force)?;
    Ok(setting(m, feature))
}

/// Settings this monitor doesn't expose, with the reason (shown in the dev build only).
#[tauri::command(async)]
fn unsupported(state: State<'_, Monitors>, index: usize) -> Result<Vec<Support>, String> {
    let mut list = state.lock();
    let m = list.get_mut(index).ok_or("monitor not found")?;
    Ok(m.unsupported())
}

#[tauri::command]
fn lightbars() -> Vec<String> {
    lightbar::find().iter().map(|b| b.path().to_string()).collect()
}

#[tauri::command]
fn lightbar_set(white: Option<(u8, u16)>, rgb: Option<(String, u8, u8, u8)>) -> Result<(), String> {
    let mut bars = lightbar::find();
    let bar = bars.first_mut().ok_or("no light bar found")?;
    match (white, rgb) {
        (Some((brightness, kelvin)), _) => bar.set_white(brightness, kelvin),
        (_, Some((effect, r, g, b))) => {
            let effect = match effect.as_str() {
                "breath" => Effect::Breath,
                "strobe" => Effect::Strobe,
                "cycle" => Effect::Cycle,
                "rainbow" => Effect::Rainbow,
                _ => Effect::Static,
            };
            bar.set_rgb(effect, r, g, b)
        }
        _ => bar.off(),
    }
}

/// An AppImage runs from a temporary mount; APPIMAGE holds the file that stays put.
fn executable() -> Result<PathBuf, String> {
    std::env::var_os("APPIMAGE").map_or_else(std::env::current_exe, |p| Ok(p.into())).map_err(|e| e.to_string())
}

/// Wayland compositors take the window icon from the desktop entry matching the app id
/// (`asusdisplay-gui`), which only the .deb/.rpm/pacman packages install. For the AppImage and
/// dev builds a hidden entry and the icon go in ~/.local/share instead.
#[cfg(target_os = "linux")]
fn install_icon() {
    const ICON: &[u8] = include_bytes!("../icons/128x128@2x.png");
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    let Some(data) = data else { return };
    let user_entry = data.join("applications/asusdisplay-gui.desktop");
    // .deb/.rpm name it after productName, the PKGBUILD after the app id.
    if ["AsusDisplay.desktop", "asusdisplay-gui.desktop"]
        .iter()
        .any(|name| PathBuf::from("/usr/share/applications").join(name).exists())
    {
        // A hidden entry left by an earlier dev/AppImage run shadows the system one and hides
        // the app from the launcher, so drop it (only if it's ours).
        if fs::read_to_string(&user_entry).is_ok_and(|s| s.contains("NoDisplay=true") && s.contains("StartupWMClass=asusdisplay-gui")) {
            let _ = fs::remove_file(&user_entry);
        }
        return;
    }
    let Ok(exe) = executable() else { return };
    let icon = data.join("icons/hicolor/256x256/apps/asusdisplay-gui.png");
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=AsusDisplay\nExec=\"{}\"\nIcon={}\nStartupWMClass=asusdisplay-gui\nNoDisplay=true\n",
        exe.display(),
        icon.display()
    );
    // Only write what changed, so launches don't keep poking the desktop's file watchers.
    for (path, content) in [(icon.clone(), ICON), (user_entry, entry.as_bytes())] {
        if fs::read(&path).ok().as_deref() != Some(content) {
            let _ = path.parent().map(fs::create_dir_all).transpose().and_then(|_| fs::write(&path, content));
        }
    }
}

/// XDG autostart entry, picked up by every desktop environment at login.
fn autostart_file() -> Result<PathBuf, String> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or("HOME is not set")?;
    Ok(config.join("autostart/asusdisplay.desktop"))
}

#[tauri::command]
fn autostart() -> bool {
    autostart_file().is_ok_and(|f| f.exists())
}

#[tauri::command]
fn set_autostart(enabled: bool) -> Result<(), String> {
    let file = autostart_file()?;
    if !enabled {
        return match fs::remove_file(&file) {
            Err(e) if e.kind() != ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }

    let exe = executable()?;
    let entry = format!("[Desktop Entry]\nType=Application\nName=AsusDisplay\nExec=\"{}\"\n", exe.display());
    fs::create_dir_all(file.parent().expect("has a parent")).and_then(|()| fs::write(&file, entry)).map_err(|e| e.to_string())
}

fn main() {
    // On NVIDIA under Wayland, WebKitGTK's DMA-BUF renderer dies with "Error 71 (Protocol error)"
    // through the driver's explicit sync. Turning that off keeps GPU rendering (and the monitor's
    // full frame rate); WEBKIT_DISABLE_DMABUF_RENDERER=1 also works but falls back to a slower path.
    #[cfg(target_os = "linux")]
    if std::path::Path::new("/proc/driver/nvidia").exists()
        && std::env::var_os("__NV_DISABLE_EXPLICIT_SYNC").is_none()
        && std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none()
    {
        // SAFETY: no other threads exist yet.
        unsafe { std::env::set_var("__NV_DISABLE_EXPLICIT_SYNC", "1") };
    }
    #[cfg(target_os = "linux")]
    install_icon();

    tauri::Builder::default()
        .setup(|app| {
            // The window's background (tauri.conf.json) is the dark theme's; match a light system.
            if let Some(window) = app.get_webview_window("main")
                && window.theme().is_ok_and(|theme| theme == tauri::Theme::Light)
            {
                let _ = window.set_background_color(Some(tauri::window::Color(0xf2, 0xf4, 0xf9, 0xff)));
            }
            Ok(())
        })
        .manage(Monitors::default())
        .invoke_handler(tauri::generate_handler![monitors, status, set, unsupported, lightbars, lightbar_set, autostart, set_autostart])
        .run(tauri::generate_context!())
        .expect("failed to start the app");
}
