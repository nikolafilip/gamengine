//! What the client remembers between runs (CLIENT.md 8): where the hub is, the last email,
//! how the mouse behaves. One small file of `key = value` lines (the flat part of TOML,
//! read and written here: a parser library would cost the browser build more than the
//! screens do); in a browser, one `localStorage` entry. Never a password, never a session.

/// Degrees per mouse count: Quake's `sensitivity 3` times `m_yaw 0.022`.
pub const DEFAULT_SENSITIVITY: f32 = 0.066;
/// What the settings screen's slider spans.
pub const SENSITIVITY_RANGE: (f32, f32) = (0.01, 0.20);
/// The sound's volume to begin with (SOUND.md 3.2).
pub const DEFAULT_VOLUME: u8 = 70;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// The hub's address (`host:port`) and the file its certificate is in; the desktop only.
    pub hub: String,
    pub hub_cert: String,
    /// The email of the last successful login, and the character played last.
    pub email: String,
    pub character: String,
    pub sensitivity: f32,
    /// Pushing the mouse forward looks down.
    pub invert: bool,
    pub third_person: bool,
    pub fullscreen: bool,
    /// The scale HUD and screens are drawn at: 1 to 4, or 0 for what the window's size
    /// gives (CLIENT.md 3).
    pub ui_scale: u8,
    /// Sound (SOUND.md 3.2): 0 to 100, and whether it is off altogether.
    pub volume: u8,
    pub mute: bool,
    /// Players whose chat lines are not shown (CLIENT.md 5), by name.
    pub ignored: Vec<String>,
    /// Lines with keys this build does not know (a newer build wrote them): kept as they
    /// stand and written back.
    pub unknown: Vec<String>,
    /// The file could not be read as settings: it is left alone, nothing is saved over it.
    pub read_only: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            hub: String::new(),
            hub_cert: String::new(),
            email: String::new(),
            character: String::new(),
            sensitivity: DEFAULT_SENSITIVITY,
            invert: false,
            third_person: false,
            fullscreen: false,
            ui_scale: 0,
            volume: DEFAULT_VOLUME,
            mute: false,
            ignored: Vec::new(),
            unknown: Vec::new(),
            read_only: false,
        }
    }
}

fn quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A value as it is written after the `=`: a text in double quotes (`\\"` and `\\\\` for a
/// quote and a backslash inside), a text in single quotes as it stands, or a bare word;
/// then nothing, or a comment.
fn value(written: &str) -> Option<String> {
    let written = written.trim();
    let (text, rest) = if let Some(inner) = written.strip_prefix('"') {
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.char_indices();
        let end = loop {
            match chars.next()? {
                (_, '\\') => out.push(chars.next()?.1),
                (i, '"') => break i + 1,
                (_, c) => out.push(c),
            }
        };
        (out, &inner[end..])
    } else if let Some(inner) = written.strip_prefix('\'') {
        let end = inner.find('\'')?;
        (inner[..end].to_string(), &inner[end + 1..])
    } else {
        let end = written.find('#').unwrap_or(written.len());
        (written[..end].trim().to_string(), "")
    };
    let rest = rest.trim();
    (rest.is_empty() || rest.starts_with('#')).then_some(text)
}

impl Settings {
    /// Read settings; a line with a key this build does not know is kept as it stands (a
    /// newer build wrote it).
    pub fn parse(text: &str) -> Result<Settings, String> {
        let mut s = Settings::default();
        // (Some editors begin a file with a byte-order mark.)
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let bad = || format!("line {}: {line:?}", n + 1);
            let (key, written) = line.split_once('=').ok_or_else(bad)?;
            let key = key.trim();
            let text = value(written).ok_or_else(bad)?;
            let flag = || text.parse::<bool>().map_err(|_| bad());
            match key {
                "hub" => s.hub = text,
                "hub_cert" => s.hub_cert = text,
                "email" => s.email = text,
                "character" => s.character = text,
                "sensitivity" => {
                    let v: f32 = text.parse().map_err(|_| bad())?;
                    if !v.is_finite() {
                        return Err(bad());
                    }
                    s.sensitivity = v.clamp(SENSITIVITY_RANGE.0, SENSITIVITY_RANGE.1);
                }
                "invert" => s.invert = flag()?,
                "third_person" => s.third_person = flag()?,
                "fullscreen" => s.fullscreen = flag()?,
                "ui_scale" => s.ui_scale = text.parse::<u8>().map_err(|_| bad())?.min(4),
                "volume" => s.volume = text.parse::<u8>().map_err(|_| bad())?.min(100),
                "mute" => s.mute = flag()?,
                // Names hold no comma.
                "ignored" => {
                    s.ignored = text
                        .split(',')
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .map(str::to_string)
                        .collect()
                }
                _ => s.unknown.push(line.to_string()),
            }
        }
        Ok(s)
    }

    pub fn to_text(&self) -> String {
        let mut text = format!(
            "# gamengine client settings (docs/CLIENT.md 8)\n\
             hub = {}\nhub_cert = {}\nemail = {}\ncharacter = {}\nsensitivity = {}\n\
             invert = {}\nthird_person = {}\nfullscreen = {}\nui_scale = {}\nvolume = {}\nmute = {}\nignored = {}\n",
            quoted(&self.hub),
            quoted(&self.hub_cert),
            quoted(&self.email),
            quoted(&self.character),
            self.sensitivity,
            self.invert,
            self.third_person,
            self.fullscreen,
            self.ui_scale,
            self.volume,
            self.mute,
            quoted(&self.ignored.join(","))
        );
        for line in &self.unknown {
            text.push_str(line);
            text.push('\n');
        }
        text
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod place {
    use std::path::{Path, PathBuf};

    use super::Settings;

    /// `$XDG_CONFIG_HOME/gamengine/settings.toml`, `~/.config/...`, `%APPDATA%\...`.
    pub fn default_path() -> Option<PathBuf> {
        let dir = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(dir.join("gamengine").join("settings.toml"))
    }

    /// What a build ships beside its program (`client.toml`, the same format): where its
    /// hub is and the file its certificate is in (looked for beside `client.toml` when the
    /// path is relative). Used where neither the command line nor the person's own
    /// settings name a hub, and never written into them.
    pub fn site_hub(root: &Path) -> Option<(String, PathBuf)> {
        let text = std::fs::read_to_string(root.join("client.toml")).ok()?;
        match Settings::parse(&text) {
            Ok(site) if !site.hub.is_empty() => {
                let cert = PathBuf::from(&site.hub_cert);
                let cert = if cert.is_relative() {
                    root.join(cert)
                } else {
                    cert
                };
                Some((site.hub, cert))
            }
            Ok(_) => None,
            Err(e) => {
                log::warn!("{}: {e}", root.join("client.toml").display());
                None
            }
        }
    }

    impl Settings {
        /// The settings in `path`: the defaults when there is no such file, and the
        /// defaults marked `read_only` when the file is not settings.
        pub fn load(path: &Path) -> Settings {
            match std::fs::read_to_string(path) {
                Ok(text) => Settings::parse(&text).unwrap_or_else(|e| {
                    log::warn!(
                        "{} is not a settings file ({e}); using the defaults and leaving it alone",
                        path.display()
                    );
                    Settings {
                        read_only: true,
                        ..Settings::default()
                    }
                }),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
                Err(e) => {
                    log::warn!("reading {}: {e}; using the defaults", path.display());
                    Settings {
                        read_only: true,
                        ..Settings::default()
                    }
                }
            }
        }

        /// Written beside itself and renamed: a crash leaves the old file or the new one.
        pub fn save(&self, path: &Path) {
            if self.read_only {
                return;
            }
            let write = || -> std::io::Result<()> {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                // (A name of this process's own: two clients may be saving at once. The
                // one that saves last is the one whose settings stay.)
                let partial = path.with_extension(format!("part{}", std::process::id()));
                std::fs::write(&partial, self.to_text())?;
                std::fs::rename(&partial, path)
            };
            if let Err(e) = write() {
                log::warn!("saving {}: {e}", path.display());
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use place::{default_path, site_hub};

#[cfg(target_arch = "wasm32")]
mod place {
    use super::Settings;

    const KEY: &str = "gamengine.settings";

    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok()?
    }

    impl Settings {
        /// The page's stored settings; the defaults when there are none or the browser
        /// keeps no storage for this page.
        pub fn load() -> Settings {
            storage()
                .and_then(|s| s.get_item(KEY).ok()?)
                .and_then(|text| Settings::parse(&text).ok())
                .unwrap_or_default()
        }

        pub fn save(&self) {
            if let Some(s) = storage() {
                let _ = s.set_item(KEY, &self.to_text());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_a_round_trip() {
        let s = Settings {
            hub: "127.0.0.1:4400".into(),
            hub_cert: "C:\\Users\\a \"b\"\\hub.der".into(),
            email: "someone@example.com".into(),
            character: "Željko".into(),
            sensitivity: 0.09,
            invert: true,
            third_person: true,
            volume: 35,
            mute: true,
            fullscreen: false,
            ui_scale: 3,
            ignored: vec!["Mallory".into(), "Ana-Marija".into()],
            unknown: vec!["music = 0.5".into()],
            read_only: false,
        };
        assert_eq!(Settings::parse(&s.to_text()).unwrap(), s);
        assert_eq!(Settings::parse("").unwrap(), Settings::default());
    }

    #[test]
    fn a_file_written_by_hand_is_read_as_it_was_meant() {
        // A byte-order mark, a comment after a value, single quotes, a bare word.
        let s = Settings::parse(
            "\u{feff}hub = \"play.example.com:4400\"   # the hub\n\
             hub_cert = 'C:\\Games\\hub.der'\nemail = someone@example.com\n\
             invert = true # upside down\nui_scale = 3\n",
        )
        .unwrap();
        assert_eq!(s.hub, "play.example.com:4400");
        assert_eq!(s.hub_cert, "C:\\Games\\hub.der");
        assert_eq!(s.email, "someone@example.com");
        assert!(s.invert);
        assert_eq!(s.ui_scale, 3);
        // A `#` inside quotes is the text's own.
        assert_eq!(
            Settings::parse("email = \"a#b@example.com\"")
                .unwrap()
                .email,
            "a#b@example.com"
        );
    }

    #[test]
    fn a_key_from_a_newer_build_is_kept_and_nonsense_is_refused() {
        let s =
            Settings::parse("# a comment\n\nmusic = 0.5\ninvert = true\nvolume = 250\n").unwrap();
        assert!(s.invert);
        assert_eq!(s.volume, 100, "a volume past the top is the top");
        // What this build does not know is written back as it stood.
        assert!(s.to_text().ends_with("music = 0.5\n"), "{}", s.to_text());
        assert!(Settings::parse("invert = yes").is_err());
        assert!(Settings::parse("email = \"a\"b\"").is_err());
        assert!(Settings::parse("email = \"unfinished").is_err());
        assert!(Settings::parse("sensitivity = fast").is_err());
        assert!(Settings::parse("sensitivity = NaN").is_err());
        assert!(Settings::parse("just a line").is_err());
        // Out of the slider's range: brought back into it.
        let s = Settings::parse("sensitivity = 40").unwrap();
        assert_eq!(s.sensitivity, SENSITIVITY_RANGE.1);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_build_names_its_hub() {
        let dir = std::env::temp_dir().join(format!("gm-site-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("client.toml"),
            "hub = \"203.0.113.7:4400\"\nhub_cert = \"hub-cert.der\"\n",
        )
        .unwrap();
        assert_eq!(
            site_hub(&dir),
            Some(("203.0.113.7:4400".to_string(), dir.join("hub-cert.der")))
        );
        // Nothing shipped, or a file that names no hub: nothing.
        assert_eq!(site_hub(&std::env::temp_dir().join("gm-site-none")), None);
        std::fs::write(dir.join("client.toml"), "invert = true\n").unwrap();
        assert_eq!(site_hub(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_file_that_is_not_settings_is_left_alone() {
        let dir = std::env::temp_dir().join(format!("gm-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.toml");
        assert_eq!(
            Settings::load(&path),
            Settings::default(),
            "no file: the defaults"
        );
        std::fs::write(&path, "this is somebody's notes\n").unwrap();
        let s = Settings::load(&path);
        assert!(s.read_only);
        s.save(&path);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "this is somebody's notes\n"
        );
        // A good one is saved and read back.
        let good = dir.join("good.toml");
        let mut s = Settings::load(&good);
        s.email = "someone@example.com".into();
        s.save(&good);
        assert_eq!(Settings::load(&good).email, "someone@example.com");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
