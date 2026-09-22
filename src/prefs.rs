use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Mutex;

const APP_DIR: &str = "foxmail-rs";
const PREFS_FILE: &str = "prefs.json";
const WCAG_AA: f32 = 4.5;

static CACHED: Mutex<Option<Prefs>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorScheme {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Palette {
    #[default]
    Default,
    Teal,
    Green,
    Purple,
    Custom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMode {
    #[default]
    Default,
    Minimal,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TokenId {
    #[default]
    Primary,
    Canvas,
    Surface,
    Text,
    Border,
}

impl TokenId {
    pub const ALL: [TokenId; 5] = [
        TokenId::Primary,
        TokenId::Canvas,
        TokenId::Surface,
        TokenId::Text,
        TokenId::Border,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Primary => "Primary",
            Self::Canvas => "Canvas",
            Self::Surface => "Surface",
            Self::Text => "Text",
            Self::Border => "Border",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSet {
    pub primary: u32,
    pub canvas: u32,
    pub surface: u32,
    pub text: u32,
    pub border: u32,
}

impl TokenSet {
    pub fn get(self, id: TokenId) -> u32 {
        match id {
            TokenId::Primary => self.primary,
            TokenId::Canvas => self.canvas,
            TokenId::Surface => self.surface,
            TokenId::Text => self.text,
            TokenId::Border => self.border,
        }
    }

    pub fn set(&mut self, id: TokenId, value: u32) {
        match id {
            TokenId::Primary => self.primary = value,
            TokenId::Canvas => self.canvas = value,
            TokenId::Surface => self.surface = value,
            TokenId::Text => self.text = value,
            TokenId::Border => self.border = value,
        }
    }

    pub fn contrast_failures(self) -> Vec<(&'static str, f32)> {
        let pairs = [
            ("Text / Canvas", self.text, self.canvas),
            ("Text / Surface", self.text, self.surface),
        ];
        pairs
            .into_iter()
            .filter_map(|(label, a, b)| {
                let ratio = contrast_ratio(a, b);
                (ratio + f32::EPSILON < WCAG_AA).then_some((label, ratio))
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prefs {
    #[serde(default)]
    pub scheme: ColorScheme,
    #[serde(default)]
    pub palette: Palette,
    #[serde(default = "default_custom_dark")]
    pub custom_dark: TokenSet,
    #[serde(default = "default_custom_light")]
    pub custom_light: TokenSet,
    #[serde(default)]
    pub sidebar_mono_icons: bool,
    #[serde(default)]
    pub hide_sender_avatar: bool,
    #[serde(default)]
    pub close_to_tray: bool,
    #[serde(default)]
    pub workspace: WorkspaceMode,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            scheme: ColorScheme::System,
            palette: Palette::Default,
            custom_dark: default_custom_dark(),
            custom_light: default_custom_light(),
            sidebar_mono_icons: false,
            hide_sender_avatar: false,
            close_to_tray: false,
            workspace: WorkspaceMode::Default,
        }
    }
}

impl Prefs {
    pub fn is_dark(&self) -> bool {
        match self.scheme {
            ColorScheme::Dark => true,
            ColorScheme::Light => false,
            ColorScheme::System => system_is_dark(),
        }
    }

    pub fn tokens(&self) -> TokenSet {
        let dark = self.is_dark();
        match self.palette {
            Palette::Custom => {
                if dark {
                    self.custom_dark
                } else {
                    self.custom_light
                }
            }
            palette => builtin_tokens(palette, dark),
        }
    }

    pub fn can_save_custom(&self) -> bool {
        if self.palette != Palette::Custom {
            return true;
        }
        self.custom_dark.contrast_failures().is_empty()
            && self.custom_light.contrast_failures().is_empty()
    }
}

pub fn cached() -> Prefs {
    let mut slot = CACHED.lock().expect("prefs lock");
    if slot.is_none() {
        *slot = Some(load().unwrap_or_else(|_| Prefs::default()));
    }
    slot.clone().unwrap_or_default()
}

pub fn apply(prefs: Prefs) -> io::Result<()> {
    save(&prefs)?;
    *CACHED.lock().expect("prefs lock") = Some(prefs);
    Ok(())
}

pub fn load() -> io::Result<Prefs> {
    load_from(&prefs_path()?)
}

pub fn save(prefs: &Prefs) -> io::Result<()> {
    save_to(&prefs_path()?, prefs)
}

fn prefs_path() -> io::Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA/APPDATA 未设置"))?;
    Ok(PathBuf::from(base).join(APP_DIR).join(PREFS_FILE))
}

fn load_from(path: &std::path::Path) -> io::Result<Prefs> {
    if !path.is_file() {
        return Ok(Prefs::default());
    }
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn save_to(path: &std::path::Path, prefs: &Prefs) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents = serde_json::to_vec_pretty(prefs)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = File::create(&temporary)?;
    file.write_all(&contents)?;
    file.sync_all()?;
    drop(file);
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)
}

pub fn default_custom_dark() -> TokenSet {
    builtin_tokens(Palette::Default, true)
}

pub fn default_custom_light() -> TokenSet {
    builtin_tokens(Palette::Default, false)
}

pub fn builtin_tokens(palette: Palette, dark: bool) -> TokenSet {
    match (palette, dark) {
        (Palette::Default | Palette::Custom, true) => TokenSet {
            primary: 0xE2795B,
            canvas: 0x191919,
            surface: 0x1C1C1C,
            text: 0xE6E6E6,
            border: 0x2A2A2A,
        },
        (Palette::Default | Palette::Custom, false) => TokenSet {
            primary: 0xC45C3E,
            canvas: 0xF4F4F4,
            surface: 0xFFFFFF,
            text: 0x1A1A1A,
            border: 0xD6D6D6,
        },
        (Palette::Teal, true) => TokenSet {
            primary: 0x3DB8A8,
            canvas: 0x121A1A,
            surface: 0x172020,
            text: 0xE6F0EE,
            border: 0x2A3A38,
        },
        (Palette::Teal, false) => TokenSet {
            primary: 0x0F766E,
            canvas: 0xF2F7F6,
            surface: 0xFFFFFF,
            text: 0x14201E,
            border: 0xD5E2DF,
        },
        (Palette::Green, true) => TokenSet {
            primary: 0x5BA86A,
            canvas: 0x141814,
            surface: 0x1A1F1A,
            text: 0xE6EDE6,
            border: 0x2A322A,
        },
        (Palette::Green, false) => TokenSet {
            primary: 0x2F7D45,
            canvas: 0xF3F6F3,
            surface: 0xFFFFFF,
            text: 0x152015,
            border: 0xD4DDD4,
        },
        (Palette::Purple, true) => TokenSet {
            primary: 0x9B7ED9,
            canvas: 0x16141C,
            surface: 0x1C1A24,
            text: 0xE8E4F0,
            border: 0x2E2A38,
        },
        (Palette::Purple, false) => TokenSet {
            primary: 0x6D4AB3,
            canvas: 0xF5F3F8,
            surface: 0xFFFFFF,
            text: 0x1A1524,
            border: 0xDDD7E6,
        },
    }
}

pub fn format_hex(color: u32) -> String {
    format!("#{:06X}", color & 0xFFFFFF)
}

pub fn parse_hex(value: &str) -> Option<u32> {
    let value = value.trim();
    let value = value.strip_prefix('#').unwrap_or(value);
    match value.len() {
        3 => {
            let mut chars = value.chars();
            let r = chars.next()?;
            let g = chars.next()?;
            let b = chars.next()?;
            let expanded = format!("{r}{r}{g}{g}{b}{b}");
            u32::from_str_radix(&expanded, 16).ok()
        }
        6 => u32::from_str_radix(value, 16).ok(),
        _ => None,
    }
}

pub fn rgb_to_hsv(color: u32) -> (f32, f32, f32) {
    let (r, g, b) = channels(color);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta < f32::EPSILON {
        0.0
    } else if (max - r).abs() < f32::EPSILON {
        60.0 * (((g - b) / delta) % 6.0)
    } else if (max - g).abs() < f32::EPSILON {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };
    let hue = if hue < 0.0 { hue + 360.0 } else { hue };
    let sat = if max < f32::EPSILON { 0.0 } else { delta / max };
    (hue, sat, max)
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> u32 {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let s = s.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0).floor() as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    pack(r + m, g + m, b + m)
}

pub fn contrast_ratio(a: u32, b: u32) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

fn relative_luminance(color: u32) -> f32 {
    let (r, g, b) = channels(color);
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

fn linear(channel: f32) -> f32 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

fn channels(color: u32) -> (f32, f32, f32) {
    let color = color & 0xFFFFFF;
    (
        ((color >> 16) & 0xFF) as f32 / 255.0,
        ((color >> 8) & 0xFF) as f32 / 255.0,
        (color & 0xFF) as f32 / 255.0,
    )
}

fn pack(r: f32, g: f32, b: f32) -> u32 {
    let byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
    (byte(r) << 16) | (byte(g) << 8) | byte(b)
}

pub fn mix_u32(a: u32, b: u32, t: f32) -> u32 {
    let (ar, ag, ab) = channels(a);
    let (br, bg, bb) = channels(b);
    pack(ar + (br - ar) * t, ag + (bg - ag) * t, ab + (bb - ab) * t)
}

fn system_is_dark() -> bool {
    #[cfg(windows)]
    {
        system_is_dark_windows().unwrap_or(true)
    }
    #[cfg(not(windows))]
    {
        true
    }
}

#[cfg(windows)]
fn system_is_dark_windows() -> io::Result<bool> {
    use std::ptr::null_mut;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_CURRENT_USER, KEY_READ, REG_DWORD,
    };

    let sub = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let name = wide("AppsUseLightTheme");
    unsafe {
        let mut hkey = null_mut();
        let status = RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_READ, &mut hkey);
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let mut data: u32 = 1;
        let mut len = 4u32;
        let mut ty = 0u32;
        let status = RegQueryValueExW(
            hkey,
            name.as_ptr(),
            null_mut(),
            &mut ty,
            (&mut data as *mut u32).cast(),
            &mut len,
        );
        RegCloseKey(hkey);
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        if ty != REG_DWORD {
            return Ok(true);
        }
        Ok(data == 0)
    }
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_on_white_meets_wcag() {
        let ratio = contrast_ratio(0x000000, 0xFFFFFF);
        assert!((ratio - 21.0).abs() < 0.05, "{ratio}");
    }

    #[test]
    fn builtin_palettes_meet_text_contrast() {
        for palette in [
            Palette::Default,
            Palette::Teal,
            Palette::Green,
            Palette::Purple,
        ] {
            for dark in [true, false] {
                let tokens = builtin_tokens(palette, dark);
                let failures = tokens.contrast_failures();
                assert!(failures.is_empty(), "{palette:?} dark={dark} {failures:?}");
            }
        }
    }

    #[test]
    fn custom_same_text_and_canvas_cannot_save() {
        let mut prefs = Prefs {
            palette: Palette::Custom,
            ..Prefs::default()
        };
        prefs.custom_dark.text = prefs.custom_dark.canvas;
        assert!(!prefs.can_save_custom());
    }

    #[test]
    fn parses_hex_and_roundtrips_hsv() {
        assert_eq!(parse_hex("#abc"), Some(0xAABBCC));
        assert_eq!(parse_hex("E2795B"), Some(0xE2795B));
        assert_eq!(format_hex(0xE2795B), "#E2795B");
        let (h, s, v) = rgb_to_hsv(0xFF0000);
        assert!((h - 0.0).abs() < 1.0);
        assert!((s - 1.0).abs() < 0.02);
        assert!((v - 1.0).abs() < 0.02);
        assert_eq!(hsv_to_rgb(h, s, v), 0xFF0000);
    }

    #[test]
    fn prefs_json_roundtrips_in_temp_dir() {
        let root = std::env::temp_dir().join(format!(
            "foxmail-rs-prefs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("prefs.json");
        let prefs = Prefs {
            scheme: ColorScheme::Light,
            palette: Palette::Purple,
            hide_sender_avatar: true,
            workspace: WorkspaceMode::Minimal,
            ..Default::default()
        };
        save_to(&path, &prefs).unwrap();
        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded, prefs);
        let _ = fs::remove_dir_all(root);
    }
}
