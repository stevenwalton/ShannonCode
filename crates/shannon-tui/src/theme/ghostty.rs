//! **A terminal's theme, read as Shannon's** (Steven, 10-14: "There's a lot
//! of themes out there and I believe there is some convergence on how to
//! import them into different programs … draw inspiration from vim and
//! ghostty").
//!
//! The convergence is the terminal palette: ghostty's 463 bundled themes —
//! iTerm2-Color-Schemes', the same set every terminal imports — are each
//! sixteen colours, a background and a foreground, and nothing else. Shannon's
//! roles are drawn in terminal colours (`theme::TERMINAL`), so sixteen colours
//! ARE a theme here (`Theme::resolve`'s remap); this reads them and adds
//! what the shipped files add by hand (`themes/*.toml`).
//!
//! **Only colours are read.** A ghostty theme file is a ghostty config file
//! and may set any ghostty option ("do not use a theme file from an untrusted
//! source", ghostty's manual): every key but the palette, the background, the
//! foreground, the selection and the cursor's colour is ghostty's and is
//! skipped unread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ratatui::style::Color;

use super::{Ansi, Paint, TERMINAL_ANSI};

/// **What a terminal theme says**: its sixteen colours, its ground and text,
/// its selection and its cursor.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Palette {
    pub(super) ansi: Ansi,
    pub(super) background: Option<Color>,
    pub(super) foreground: Option<Color>,
    pub(super) selection: Option<Color>,
    pub(super) cursor: Option<Color>,
}

/// **Read a ghostty theme file**: `palette = N=#rrggbb`, `background`,
/// `foreground`, `selection-background`, `cursor-color` — every other key
/// unread. A colour it cannot read is said; the rest is kept.
pub(super) fn read(name: &str, text: &str) -> (Palette, Vec<String>) {
    let mut palette = Palette {
        ansi: TERMINAL_ANSI,
        ..Palette::default()
    };
    let mut notes = Vec::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        let mut said = |what: &str| {
            notes.push(format!(
                "theme `{name}`: `{key} = {value}` — {what} is not a colour this reads (#rrggbb)"
            ));
        };
        match key {
            "palette" => {
                let Some((slot, colour)) = value.split_once('=') else {
                    said("it");
                    continue;
                };
                match (slot.trim().parse::<usize>(), hex(colour.trim())) {
                    (Ok(slot), Some(colour)) if slot < 16 => palette.ansi[slot] = Some(colour),
                    // Colours past the sixteen are ghostty's 256; ours are named
                    // by the sixteen.
                    (Ok(slot), Some(_)) if slot < 256 => {}
                    _ => said("it"),
                }
            }
            "background" | "foreground" | "selection-background" | "cursor-color" => {
                match hex(value) {
                    Some(colour) => {
                        let slot = match key {
                            "background" => &mut palette.background,
                            "foreground" => &mut palette.foreground,
                            "cursor-color" => &mut palette.cursor,
                            _ => &mut palette.selection,
                        };
                        *slot = Some(colour);
                    }
                    None => said("it"),
                }
            }
            _ => {}
        }
    }
    (palette, notes)
}

/// `#rrggbb` or `rrggbb`.
fn hex(text: &str) -> Option<Color> {
    let digits = text.strip_prefix('#').unwrap_or(text);
    if digits.len() != 6 || !digits.is_ascii() {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some(Color::Rgb(byte(0)?, byte(2)?, byte(4)?))
}

/// **A palette made a theme** — the entries the shipped files write by
/// hand, from what the palette says: its ground and text; dark text on the
/// decision pane's fills and a screen's bar (the terminal's white on a
/// pastel red reads poorly); in a light theme, its text where the shipped
/// theme paints white; and where the palette repeats its magenta in the
/// bright slot — a withheld value's colour — the pane's expansions moved to
/// its cyan. Over `terminal`'s entries, as `inherits = "terminal"`.
pub(super) fn entries(palette: &Palette) -> HashMap<String, Paint> {
    let slot = |n: usize| palette.ansi[n];
    let paint = |fg: Option<Color>, bg: Option<Color>, bold: bool| Paint {
        fg,
        bg,
        bold,
        ..Paint::default()
    };
    let base = palette.background;
    let text = palette.foreground;
    let faint = slot(8);
    let mut out: HashMap<String, Paint> = HashMap::new();
    let mut put = |key: &str, p: Paint| {
        out.insert(key.to_string(), p);
    };
    if base.is_some() || text.is_some() {
        put("ui.background", paint(text, base, false));
    }
    // The palette's own cursor, over the ground's text the theme would
    // otherwise derive (`Theme::resolve`).
    if let Some(cursor) = palette.cursor {
        put("ui.cursor", paint(None, Some(cursor), false));
    }
    if faint.is_some() {
        put(
            "comment",
            Paint {
                italic: true,
                ..paint(faint, None, false)
            },
        );
        put(
            "ui.graph.shared",
            Paint {
                italic: true,
                ..paint(faint, None, false)
            },
        );
    }
    if let Some(base) = base {
        put("ui.menu.selected", paint(Some(base), text, false));
        put("ui.bar", paint(Some(base), slot(6), true));
        put("ui.bar.waiting", paint(Some(base), slot(3), true));
        put("ui.danger.sign", paint(Some(base), slot(1), true));
        put("ui.caution.fill", paint(Some(base), slot(3), false));
        put("ui.danger.fill", paint(Some(base), slot(1), true));
    }
    if let Some(selection) = palette.selection.or(slot(0)) {
        put("ui.graph.mark", paint(None, Some(selection), false));
    }
    if base.is_some_and(is_light) {
        for key in ["ui.text", "ui.statusline"] {
            put(key, paint(text, None, false));
        }
        for key in [
            "ui.text.quiet",
            "ui.muted",
            "ui.border",
            "ui.tabline",
            "decision.operator",
        ] {
            put(key, paint(faint, None, false));
        }
        put("decision.function.call", paint(text, None, true));
    }
    if slot(13).is_some() && slot(13) == slot(5) {
        for key in [
            "decision.variable.special",
            "decision.tag",
            "decision.number",
            "decision.constant",
        ] {
            put(key, paint(slot(6), None, false));
        }
    }
    out
}

/// Is `colour` a light ground — the luma of a 24-bit colour past half?
fn is_light(colour: Color) -> bool {
    match colour {
        Color::Rgb(r, g, b) => {
            2126 * u32::from(r) + 7152 * u32::from(g) + 722 * u32::from(b) > 128 * 10_000
        }
        _ => false,
    }
}

/// **Ghostty's theme directories**, wherever this machine keeps them — the
/// user's own (`$XDG_CONFIG_HOME/ghostty/themes`, else
/// `~/.config/ghostty/themes`), then the bundled set (`$GHOSTTY_RESOURCES_DIR`,
/// else where macOS and Linux install it): ghostty's own lookup order. `env`
/// is the environment, a parameter so a test reads none of this machine's.
pub(crate) fn dirs(env: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    match env("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(config) => out.push(Path::new(&config).join("ghostty/themes")),
        None => {
            if let Some(home) = env("HOME").filter(|v| !v.is_empty()) {
                out.push(Path::new(&home).join(".config/ghostty/themes"));
            }
        }
    }
    match env("GHOSTTY_RESOURCES_DIR").filter(|v| !v.is_empty()) {
        Some(resources) => out.push(Path::new(&resources).join("themes")),
        None => {
            if cfg!(target_os = "macos") {
                out.push(PathBuf::from(
                    "/Applications/Ghostty.app/Contents/Resources/ghostty/themes",
                ));
            } else {
                out.push(PathBuf::from("/usr/share/ghostty/themes"));
                out.push(PathBuf::from("/usr/local/share/ghostty/themes"));
            }
        }
    }
    out
}

/// **The theme files in `dir`**, by name — a regular file's name, never a
/// hidden one, never a directory. The names a lookup may match exactly: a
/// name is found in this list, never joined onto `dir` as a path, so no name
/// reaches outside it. A directory that cannot be read lists nothing.
pub(super) fn listed(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A ghostty file reads as its colours** — the sixteen, the ground,
    /// the text, the selection and the cursor; every other key unread; a
    /// colour it cannot read said, the rest kept.
    #[test]
    fn a_ghostty_file_reads_as_its_colours() {
        let text = "palette = 0=#45475a\npalette = 5=f5c2e7\npalette = 200=#000000\n\
                    background = #1e1e2e\nforeground = #cdd6f4\nselection-background = #585b70\n\
                    cursor-color = #f5e0dc\nfont-family = Comic Sans\ncommand = rm /tmp/a\n\
                    palette = 3=mauve\n";
        let (palette, notes) = read("t", text);
        assert_eq!(palette.ansi[0], Some(Color::Rgb(0x45, 0x47, 0x5a)));
        assert_eq!(
            palette.ansi[5],
            Some(Color::Rgb(0xf5, 0xc2, 0xe7)),
            "bare hex"
        );
        assert_eq!(palette.ansi[3], None, "unread");
        assert_eq!(palette.background, Some(Color::Rgb(0x1e, 0x1e, 0x2e)));
        assert_eq!(palette.foreground, Some(Color::Rgb(0xcd, 0xd6, 0xf4)));
        assert_eq!(palette.selection, Some(Color::Rgb(0x58, 0x5b, 0x70)));
        assert_eq!(palette.cursor, Some(Color::Rgb(0xf5, 0xe0, 0xdc)));
        assert_eq!(
            entries(&palette)["ui.cursor"].bg,
            palette.cursor,
            "the palette's cursor, the block's colour"
        );
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("mauve"), "{notes:?}");
    }

    /// **A light ground takes its text where `terminal` paints white**, and
    /// a dark one does not; a repeated magenta moves the pane's expansions.
    #[test]
    fn a_palette_is_made_a_theme() {
        let rgb = |v: u32| Some(Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8));
        let mut palette = Palette {
            background: rgb(0xfdf6e3),
            foreground: rgb(0x657b83),
            ..Palette::default()
        };
        palette.ansi[5] = rgb(0xd33682);
        palette.ansi[13] = rgb(0xd33682);
        palette.ansi[6] = rgb(0x2aa198);
        let light = entries(&palette);
        assert_eq!(light["ui.text"].fg, rgb(0x657b83));
        assert_eq!(light["decision.variable.special"].fg, rgb(0x2aa198));
        palette.background = rgb(0x002b36);
        let dark = entries(&palette);
        assert!(
            !dark.contains_key("ui.text"),
            "dark: the terminal's white stays"
        );
        assert_eq!(dark["ui.background"].bg, rgb(0x002b36));
    }

    /// **Ghostty's own lookup order**, from the environment given — never
    /// this machine's.
    #[test]
    fn ghostty_dirs_follow_its_lookup() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                vars.iter()
                    .find(|(name, _)| *name == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        let xdg = dirs(&env(&[
            ("XDG_CONFIG_HOME", "/x"),
            ("GHOSTTY_RESOURCES_DIR", "/r"),
        ]));
        assert_eq!(
            xdg,
            [
                PathBuf::from("/x/ghostty/themes"),
                PathBuf::from("/r/themes")
            ]
        );
        let home = dirs(&env(&[("HOME", "/h")]));
        assert_eq!(home[0], PathBuf::from("/h/.config/ghostty/themes"));
        assert!(home.len() > 1, "the bundled set too: {home:?}");
    }
}
