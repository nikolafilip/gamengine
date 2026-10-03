//! What the content bundle holds (`manifest.gmc`, CONTENT.md 5.2): every entry by its key,
//! with the files, the icon keys and the hashes a client needs to draw the content it meets.
//! Written by `gm-tools content build`, read by the client in one call; `bitcode` because it
//! is the wire's codec and already in the client. A client looks everything up by key and
//! never by position, so a bundle older or newer than a zone's content resolves what it can.

use bitcode::{Decode, Encode};

/// The format's own version: a client refuses a manifest of another.
pub const MANIFEST_VERSION: u32 = 1;
/// The most entries any list may have: a bound before anything is allocated.
pub const MAX_ENTRIES: usize = 4096;

#[derive(Clone, Debug, Default, PartialEq, Encode, Decode)]
pub struct Manifest {
    pub format: u32,
    /// The content's own version (`assets/content/VERSION`).
    pub content_version: u32,
    pub templates: Vec<TemplateEntry>,
    pub materials: Vec<MaterialEntry>,
    pub abilities: Vec<AbilityEntry>,
    pub creatures: Vec<CreatureEntry>,
    pub builds: Vec<BuildEntry>,
    pub props: Vec<PropEntry>,
    /// Icon keys of the statuses, by status name (`burn`, `chill`, ...).
    pub statuses: Vec<IconEntry>,
    /// Portraits by `<frame>_<armour>` (`striker_mail`).
    pub portraits: Vec<IconEntry>,
    /// SHA-256 of `ui.gma`.
    pub atlas_sha256: [u8; 32],
    pub atlas_bytes: u32,
}

/// Which hand (CONTENT.md 3): `right`, `left`, `back`.
pub const HELD_RIGHT: u8 = 0;
pub const HELD_LEFT: u8 = 1;
pub const HELD_BACK: u8 = 2;

#[derive(Clone, Debug, Default, PartialEq, Encode, Decode)]
pub struct TemplateEntry {
    pub key: String,
    /// `weapon` or `armour`.
    pub kind: String,
    /// The prop key of its model, when it has one.
    pub model: Option<String>,
    /// The icon's key in the atlas, when it has one (given or baked).
    pub icon: Option<String>,
    pub held: u8,
    /// Where the view model sits (LOOK.md 6.4): move (metres), turn (degrees), scale.
    pub fit_view: Option<[f32; 7]>,
    pub retired: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Encode, Decode)]
pub struct MaterialEntry {
    pub key: String,
    pub layer: String,
    pub icon: Option<String>,
    pub tint: Option<[f32; 3]>,
}

#[derive(Clone, Debug, Default, PartialEq, Encode, Decode)]
pub struct AbilityEntry {
    pub key: String,
    pub icon: Option<String>,
    pub prop: Option<String>,
    pub sound: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Encode, Decode)]
pub struct CreatureEntry {
    pub key: String,
    pub model: Option<String>,
    pub icon: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Encode, Decode)]
pub struct BuildEntry {
    pub name: String,
    pub icon: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Encode, Decode)]
pub struct PropEntry {
    pub key: String,
    /// The file under the bundle: `props/<key>.gmm`.
    pub file: String,
    pub sha256: [u8; 32],
    pub bytes: u32,
    pub triangles: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Encode, Decode)]
pub struct IconEntry {
    pub key: String,
    pub icon: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("not a manifest")]
    Decode,
    #[error("manifest format {0}; this client reads {MANIFEST_VERSION}")]
    Version(u32),
    #[error("a list is too long")]
    TooMany,
}

impl Manifest {
    pub fn encode(&self) -> Vec<u8> {
        bitcode::encode(self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Manifest, ManifestError> {
        let m: Manifest = bitcode::decode(bytes).map_err(|_| ManifestError::Decode)?;
        if m.format != MANIFEST_VERSION {
            return Err(ManifestError::Version(m.format));
        }
        if [
            m.templates.len(),
            m.materials.len(),
            m.abilities.len(),
            m.creatures.len(),
            m.builds.len(),
            m.props.len(),
            m.statuses.len(),
            m.portraits.len(),
        ]
        .iter()
        .any(|n| *n > MAX_ENTRIES)
        {
            return Err(ManifestError::TooMany);
        }
        Ok(m)
    }

    pub fn template(&self, key: &str) -> Option<&TemplateEntry> {
        self.templates.iter().find(|t| t.key == key)
    }

    pub fn ability(&self, key: &str) -> Option<&AbilityEntry> {
        self.abilities.iter().find(|a| a.key == key)
    }

    pub fn material(&self, key: &str) -> Option<&MaterialEntry> {
        self.materials.iter().find(|m| m.key == key)
    }

    pub fn prop(&self, key: &str) -> Option<&PropEntry> {
        self.props.iter().find(|p| p.key == key)
    }

    pub fn creature(&self, key: &str) -> Option<&CreatureEntry> {
        self.creatures.iter().find(|c| c.key == key)
    }

    pub fn portrait(&self, key: &str) -> Option<&str> {
        self.portraits
            .iter()
            .find(|p| p.key == key)
            .map(|p| p.icon.as_str())
    }

    pub fn status_icon(&self, key: &str) -> Option<&str> {
        self.statuses
            .iter()
            .find(|p| p.key == key)
            .map(|p| p.icon.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_the_version_is_checked() {
        let m = Manifest {
            format: MANIFEST_VERSION,
            content_version: 3,
            templates: vec![TemplateEntry {
                key: "sword".into(),
                kind: "weapon".into(),
                model: Some("sword_iron".into()),
                icon: Some("sword".into()),
                held: HELD_RIGHT,
                fit_view: None,
                retired: false,
            }],
            props: vec![PropEntry {
                key: "sword_iron".into(),
                file: "props/sword_iron.gmm".into(),
                sha256: [7; 32],
                bytes: 1234,
                triangles: 48,
            }],
            ..Default::default()
        };
        let bytes = m.encode();
        assert_eq!(Manifest::decode(&bytes).unwrap(), m);
        assert_eq!(
            m.template("sword").unwrap().model.as_deref(),
            Some("sword_iron")
        );
        assert!(m.template("axe").is_none());
        let old = Manifest {
            format: 0,
            ..m.clone()
        };
        assert_eq!(
            Manifest::decode(&old.encode()),
            Err(ManifestError::Version(0))
        );
        assert_eq!(Manifest::decode(b"junk"), Err(ManifestError::Decode));
    }
}
