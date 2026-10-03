//! The looks of content (CONTENT.md 3): what the rows say about pictures, props, models and
//! sounds, by key. Zones load this beside the pack to name every prop key in the pack's
//! `props` list and to choose what a body holds (LOOK.md 6.1); the tool loads it to check
//! every link and build the bundle; clients never see these files, only the bundle's
//! manifest. Nothing here touches what a thing does.

use std::path::Path;

use crate::items::{self, Fit};
use crate::{AbilitiesFile, BuildsFile, ContentError, CreaturesFile};

/// `Look.held` for a hand with nothing in it (LOOK.md 6.2).
pub const NONE: u16 = u16::MAX;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AbilityLook {
    pub key: String,
    pub icon: Option<String>,
    pub prop: Option<String>,
    pub sound: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TemplateLook {
    pub key: String,
    pub kind: String,
    pub model: Option<String>,
    pub icon: Option<String>,
    /// `right` (the default), `left` or `back`.
    pub held: String,
    pub fit: Fit,
    pub fit_view: Option<Fit>,
    pub retired: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MaterialLook {
    pub key: String,
    pub layer: String,
    pub icon: Option<String>,
    pub tint: Option<[f32; 3]>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CreatureLook {
    pub key: String,
    pub model: Option<String>,
    pub icon: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildLook {
    pub name: String,
    pub icon: Option<String>,
}

/// Every look field of the content, by key, and the props list a zone sends.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Looks {
    pub abilities: Vec<AbilityLook>,
    pub templates: Vec<TemplateLook>,
    pub materials: Vec<MaterialLook>,
    pub creatures: Vec<CreatureLook>,
    pub builds: Vec<BuildLook>,
    /// The keys of every `ability.prop` and `template.model`, in order of first appearance
    /// (LOOK.md 6.2): a `Look.held` on the wire is an index into this list.
    pub props: Vec<String>,
}

/// Keys are lowercase ASCII letters, digits, `_` and at most one `/` (CONTENT.md 2).
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 48
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '/')
        && key.matches('/').count() <= 1
        && !key.starts_with('/')
        && !key.ends_with('/')
}

impl Looks {
    /// Read the look fields of the five tables in `dir`; the tables that do not exist are
    /// empty. Keys of look fields are checked for shape here; that their files exist is
    /// the tool's check (CONTENT.md 5.1), because zones have no bundle.
    pub fn load_dir(dir: &Path) -> Result<Looks, ContentError> {
        let read = |name: &str, optional: bool| {
            let path = dir.join(name);
            match std::fs::read_to_string(&path) {
                Ok(text) => Ok(text),
                Err(e) if optional && e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
                Err(e) => Err(ContentError::Io {
                    path: path.display().to_string(),
                    source: e,
                }),
            }
        };
        Looks::load_str(
            &read("abilities.toml", false)?,
            &read("builds.toml", true)?,
            &read("creatures.toml", true)?,
            &read("items.toml", true)?,
        )
    }

    pub fn load_str(
        abilities: &str,
        builds: &str,
        creatures: &str,
        items: &str,
    ) -> Result<Looks, ContentError> {
        fn parse<T: serde::de::DeserializeOwned>(
            text: &str,
            path: &str,
        ) -> Result<T, ContentError> {
            toml::from_str(text).map_err(|e| ContentError::Toml {
                path: path.into(),
                source: e,
            })
        }
        let af: AbilitiesFile = parse(abilities, "abilities.toml")?;
        let bf: BuildsFile = if builds.is_empty() {
            BuildsFile { build: Vec::new() }
        } else {
            parse(builds, "builds.toml")?
        };
        let cf: CreaturesFile = parse(creatures, "creatures.toml")?;
        let it: items::ItemContent = parse(items, "items.toml")?;
        let mut looks = Looks::default();
        let mut props: Vec<String> = Vec::new();
        let mut name_prop = |key: &Option<String>, owner: &str| -> Result<(), ContentError> {
            if let Some(k) = key {
                if !valid_key(k) {
                    return Err(ContentError::Invalid(format!(
                        "{owner}: `{k}` is not a key (lowercase letters, digits, `_`)"
                    )));
                }
                if !props.contains(k) {
                    props.push(k.clone());
                }
            }
            Ok(())
        };
        for a in &af.ability {
            name_prop(&a.prop, &format!("ability {}", a.key))?;
            for (field, value) in [("icon", &a.icon), ("sound", &a.sound)] {
                if let Some(v) = value
                    && !valid_key(v)
                {
                    return Err(ContentError::Invalid(format!(
                        "ability {}: {field} `{v}` is not a key",
                        a.key
                    )));
                }
            }
            looks.abilities.push(AbilityLook {
                key: a.key.clone(),
                icon: a.icon.clone(),
                prop: a.prop.clone(),
                sound: a.sound.clone(),
            });
        }
        for t in &it.templates {
            name_prop(&t.model, &format!("template {}", t.id))?;
            let held = t.held.clone().unwrap_or_else(|| "right".into());
            if !["right", "left", "back"].contains(&held.as_str()) {
                return Err(ContentError::Invalid(format!(
                    "template {}: held must be right, left or back, not `{held}`",
                    t.id
                )));
            }
            if let Some(v) = &t.icon
                && !valid_key(v)
            {
                return Err(ContentError::Invalid(format!(
                    "template {}: icon `{v}` is not a key",
                    t.id
                )));
            }
            for fit in [t.fit.as_ref(), t.fit_view.as_ref()].into_iter().flatten() {
                let finite = fit.mov.iter().chain(&fit.turn).all(|v| v.is_finite())
                    && fit.scale.is_finite()
                    && fit.scale > 0.0
                    && fit.scale <= 100.0;
                if !finite {
                    return Err(ContentError::Invalid(format!(
                        "template {}: a fit must be finite with a scale in (0, 100]",
                        t.id
                    )));
                }
            }
            looks.templates.push(TemplateLook {
                key: t.id.clone(),
                kind: t.kind.clone(),
                model: t.model.clone(),
                icon: t.icon.clone(),
                held,
                fit: t.fit.unwrap_or_default(),
                fit_view: t.fit_view,
                retired: t.retired,
            });
        }
        for m in &it.materials {
            if let Some(v) = &m.icon
                && !valid_key(v)
            {
                return Err(ContentError::Invalid(format!(
                    "material {}: icon `{v}` is not a key",
                    m.id
                )));
            }
            if let Some(t) = &m.tint
                && !t.iter().all(|c| c.is_finite() && (0.0..=1.0).contains(c))
            {
                return Err(ContentError::Invalid(format!(
                    "material {}: a tint is three numbers in [0, 1]",
                    m.id
                )));
            }
            looks.materials.push(MaterialLook {
                key: m.id.clone(),
                layer: m.layer().to_string(),
                icon: m.icon.clone(),
                tint: m.tint,
            });
        }
        for c in &cf.creature {
            for (field, value) in [("model", &c.model), ("icon", &c.icon)] {
                if let Some(v) = value
                    && !valid_key(v)
                {
                    return Err(ContentError::Invalid(format!(
                        "creature {}: {field} `{v}` is not a key",
                        c.key
                    )));
                }
            }
            looks.creatures.push(CreatureLook {
                key: c.key.clone(),
                model: c.model.clone(),
                icon: c.icon.clone(),
            });
        }
        for b in &bf.build {
            if let Some(v) = &b.icon
                && !valid_key(v)
            {
                return Err(ContentError::Invalid(format!(
                    "build {}: icon `{v}` is not a key",
                    b.name
                )));
            }
            looks.builds.push(BuildLook {
                name: b.name.clone(),
                icon: b.icon.clone(),
            });
        }
        if props.len() >= NONE as usize {
            return Err(ContentError::Invalid("too many props".into()));
        }
        looks.props = props;
        Ok(looks)
    }

    /// The index of a prop key in the list a zone sends; `NONE` for a key it has not.
    pub fn prop_index(&self, key: &str) -> u16 {
        self.props
            .iter()
            .position(|p| p == key)
            .map_or(NONE, |i| i as u16)
    }

    /// What a body holds (LOOK.md 6.1): the model of the weapon template it wears, else the
    /// prop of its primary ability (by the ability's place in the pack), else nothing. A
    /// template or an ability the content does not know is an empty hand.
    pub fn held(&self, worn_weapon: Option<&str>, primary: Option<usize>) -> u16 {
        if let Some(t) = worn_weapon
            && let Some(look) = self.templates.iter().find(|l| l.key == t)
            && let Some(model) = &look.model
        {
            return self.prop_index(model);
        }
        if let Some(i) = primary
            && let Some(a) = self.abilities.get(i)
            && let Some(prop) = &a.prop
        {
            return self.prop_index(prop);
        }
        NONE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABILITIES: &str = r#"
[[ability]]
key = "sword"
name = "Sword"
slot = "primary"
cost = 2
prop = "sword_plain"
icon = "sword"
[[ability]]
key = "hammer"
name = "Hammer"
slot = "primary"
cost = 2
prop = "hammer_plain"
[[ability]]
key = "firebolt"
name = "Firebolt"
slot = "secondary"
cost = 4
sound = "whoosh"
"#;

    const ITEMS: &str = r#"
[[template]]
id = "sword"
kind = "weapon"
strikes = "slash"
layers = ["core", "frame"]
model = "sword_iron"
fit = { move = [0.0, -0.02, 0.0], turn = [0, 90, 0], scale = 1.0 }
[[template]]
id = "hammer"
kind = "weapon"
strikes = "blunt"
layers = ["core", "frame"]
model = "hammer_plain"
held = "back"
[[material]]
id = "core/iron"
edge = 30
tint = [0.8, 0.8, 0.85]
"#;

    #[test]
    fn props_are_listed_by_first_appearance_and_a_body_holds_what_it_wears_or_swings() {
        let looks = Looks::load_str(ABILITIES, "", "", ITEMS).unwrap();
        assert_eq!(looks.props, ["sword_plain", "hammer_plain", "sword_iron"]);
        // Worn: the item's model. Bare: the primary's prop. Neither: nothing.
        assert_eq!(looks.held(Some("sword"), Some(0)), 2);
        assert_eq!(looks.held(None, Some(0)), 0);
        assert_eq!(looks.held(None, Some(1)), 1);
        assert_eq!(looks.held(None, Some(2)), NONE);
        assert_eq!(looks.held(Some("cuirass"), Some(0)), 0);
        assert_eq!(looks.held(Some("nothing"), None), NONE);
        assert_eq!(looks.held(None, Some(99)), NONE);
        assert_eq!(looks.templates[1].held, "back");
        assert_eq!(looks.templates[0].fit.turn, [0.0, 90.0, 0.0]);
        assert_eq!(looks.materials[0].tint, Some([0.8, 0.8, 0.85]));
        assert_eq!(looks.abilities[2].sound.as_deref(), Some("whoosh"));
    }

    #[test]
    fn a_key_has_a_shape() {
        assert!(valid_key("sword_iron") && valid_key("core/iron") && valid_key("m5x7"));
        assert!(!valid_key("Sword") && !valid_key("a b") && !valid_key("a/b/c") && !valid_key(""));
        let bad = ABILITIES.replace("sword_plain", "Sword Plain");
        assert!(Looks::load_str(&bad, "", "", ITEMS).is_err());
        let bad = ITEMS.replace("held = \"back\"", "held = \"teeth\"");
        assert!(Looks::load_str(ABILITIES, "", "", &bad).is_err());
    }

    #[test]
    fn the_shipped_content_loads() {
        let dir =
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"));
        let looks = Looks::load_dir(dir).unwrap();
        assert_eq!(
            looks.abilities.len(),
            crate::load_dir(dir, gm_core::tick::TickRate::COMBAT)
                .unwrap()
                .abilities
                .len()
        );
    }
}
