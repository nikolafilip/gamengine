//! A zone's tuning on disk (GM.md 3): what a game master set by hand, as TOML, read when
//! the zone starts and written after every change, so a restart keeps the numbers and the
//! file can be read into the content when they are decided on.
//!
//! ```toml
//! tempo = 1.5
//!
//! [[ability]]
//! key = "sword"
//! windup_ms = 180
//! ```

use std::path::Path;

use gm_core::tuning::{AbilityTuning, Tuning};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize, Serialize)]
struct File {
    #[serde(default = "one")]
    tempo: f32,
    #[serde(default, rename = "ability")]
    abilities: Vec<Row>,
}

fn one() -> f32 {
    1.0
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct Row {
    key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cast_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    windup_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cooldown_ms: Option<u32>,
}

/// The tuning in `text`.
pub fn parse(text: &str) -> Result<Tuning, String> {
    let file: File = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut tuning = Tuning {
        tempo: file.tempo,
        abilities: Vec::new(),
    };
    for r in file.abilities {
        tuning.set(AbilityTuning {
            key: r.key,
            cast_ms: r.cast_ms,
            windup_ms: r.windup_ms,
            active_ms: r.active_ms,
            recovery_ms: r.recovery_ms,
            cooldown_ms: r.cooldown_ms,
        });
    }
    Ok(tuning)
}

/// The tuning as a file's text.
pub fn render(tuning: &Tuning) -> String {
    let file = File {
        tempo: tuning.tempo,
        abilities: tuning
            .abilities
            .iter()
            .filter(|a| !a.is_empty())
            .map(|a| Row {
                key: a.key.clone(),
                cast_ms: a.cast_ms,
                windup_ms: a.windup_ms,
                active_ms: a.active_ms,
                recovery_ms: a.recovery_ms,
                cooldown_ms: a.cooldown_ms,
            })
            .collect(),
    };
    let mut text =
        String::from("# Set by a game master in play (GM.md 3); read when a zone starts.\n");
    text.push_str(&toml::to_string(&file).unwrap_or_default());
    text
}

/// The tuning in the file at `path`; the default when there is no file.
pub fn read(path: &Path) -> Result<Tuning, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Tuning::default()),
        Err(e) => Err(e.to_string()),
    }
}

/// Write the tuning to `path` (whole, through a temporary file beside it).
pub fn write(path: &Path, tuning: &Tuning) -> Result<(), String> {
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, render(tuning)).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tuning_survives_the_file() {
        let mut t = Tuning {
            tempo: 1.5,
            abilities: Vec::new(),
        };
        t.set(AbilityTuning {
            key: "sword".into(),
            windup_ms: Some(180),
            cooldown_ms: Some(400),
            ..Default::default()
        });
        let text = render(&t);
        assert!(text.contains("tempo = 1.5"), "{text}");
        assert!(text.contains("windup_ms = 180"), "{text}");
        assert!(!text.contains("active_ms"), "{text}");
        assert_eq!(parse(&text).unwrap(), t);
        assert_eq!(parse("").unwrap(), Tuning::default());
        assert!(parse("tempo = \"fast\"").is_err());
    }
}
