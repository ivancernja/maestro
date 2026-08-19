use ratatui::style::Color;
use std::{collections::HashMap, path::PathBuf, process::Command, time::SystemTime};

/// The palette comes from `omarchy-theme-color --all`, the same resolver every
/// other Omarchy consumer uses, so maestro matches the active theme exactly
/// rather than approximating it. Off Omarchy it falls back to ANSI names, which
/// inherit whatever the terminal defines.
#[derive(Clone, Copy)]
pub struct Theme {
    pub accent: Color,
    pub fg: Color,
    pub muted: Color,
    pub selection: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
    stamp: Option<SystemTime>,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            accent: Color::Cyan,
            fg: Color::Reset,
            muted: Color::DarkGray,
            selection: Color::DarkGray,
            green: Color::Green,
            yellow: Color::Yellow,
            red: Color::Red,
            stamp: None,
        }
    }
}

fn colors_file() -> PathBuf {
    // MAESTRO_THEME_FILE points at another colors.toml, for tests and previews.
    if let Ok(path) = std::env::var("MAESTRO_THEME_FILE") {
        return PathBuf::from(path);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".local/state/omarchy/current/theme/colors.toml")
}

/// omarchy-theme-set replaces the whole theme directory, so the colors file's
/// mtime is enough to notice a theme change while the app is running.
fn stamp() -> Option<SystemTime> {
    std::fs::metadata(colors_file())
        .and_then(|m| m.modified())
        .ok()
}

fn hex(value: &str) -> Option<Color> {
    let digits = value.trim().trim_start_matches('#');
    if digits.len() != 6 {
        return None;
    }
    Some(Color::Rgb(
        u8::from_str_radix(&digits[0..2], 16).ok()?,
        u8::from_str_radix(&digits[2..4], 16).ok()?,
        u8::from_str_radix(&digits[4..6], 16).ok()?,
    ))
}

impl Theme {
    pub fn load() -> Self {
        let mut theme = Self {
            stamp: stamp(),
            ..Self::default()
        };

        let mut command = Command::new("omarchy-theme-color");
        if let Ok(path) = std::env::var("MAESTRO_THEME_FILE") {
            command.args(["--file", &path]);
        }
        let Ok(out) = command.arg("--all").output() else {
            return theme;
        };
        if !out.status.success() {
            return theme;
        }

        let mut palette: HashMap<&str, &str> = HashMap::new();
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        for line in text.lines() {
            if let Some((key, value)) = line.split_once('\t') {
                palette.insert(key.trim(), value.trim());
            }
        }

        let pick = |keys: &[&str], fallback: Color| -> Color {
            keys.iter()
                .filter_map(|k| palette.get(*k))
                .filter_map(|v| hex(v))
                .next()
                .unwrap_or(fallback)
        };

        theme.accent = pick(&["accent"], theme.accent);
        theme.fg = pick(&["foreground", "fg"], theme.fg);
        theme.muted = pick(&["muted", "dark_foreground"], theme.muted);
        theme.selection = pick(&["selection", "lighter_background"], theme.selection);
        theme.green = pick(&["green"], theme.green);
        theme.yellow = pick(&["yellow"], theme.yellow);
        theme.red = pick(&["red"], theme.red);
        theme
    }

    /// True when the theme on disk has changed since this one was loaded.
    pub fn is_stale(&self) -> bool {
        stamp() != self.stamp
    }
}
