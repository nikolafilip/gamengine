//! Item content (ECONOMY.md 4): templates and materials from `items.toml`, with the gear
//! edge cap (PLAN.md 0) checked at load.

use std::path::Path;

use serde::Deserialize;

use crate::ContentError;

pub const LAYERS: [&str; 5] = ["shard", "core", "catalyst", "frame", "gem"];
/// Gem sockets per item.
pub const MAX_GEMS: u32 = 2;
/// The largest total edge of a fully crafted item over a standard one, per mille.
pub const MAX_EDGE_PER_MILLE: u32 = 250;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemTemplate {
    pub id: String,
    pub kind: String,
    pub layers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Material {
    /// `layer/name`.
    pub id: String,
    /// Per mille of the item's base stats.
    pub edge: u32,
    #[serde(default)]
    pub element: Option<String>,
    #[serde(default)]
    pub note: String,
}

impl Material {
    pub fn layer(&self) -> &str {
        self.id.split('/').next().unwrap_or("")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemContent {
    #[serde(default, rename = "template")]
    pub templates: Vec<ItemTemplate>,
    #[serde(default, rename = "material")]
    pub materials: Vec<Material>,
}

impl ItemContent {
    /// The best edge a craft of this template can reach, per mille.
    pub fn best_edge(&self, template: &ItemTemplate) -> u32 {
        template
            .layers
            .iter()
            .map(|layer| {
                let best = self
                    .materials
                    .iter()
                    .filter(|m| m.layer() == layer)
                    .map(|m| m.edge)
                    .max()
                    .unwrap_or(0);
                if layer == "gem" {
                    best * MAX_GEMS
                } else {
                    best
                }
            })
            .sum()
    }

    pub fn template_ids(&self) -> Vec<String> {
        self.templates.iter().map(|t| t.id.clone()).collect()
    }

    fn validate(&self) -> Result<(), ContentError> {
        let bad = |s: String| Err(ContentError::Invalid(s));
        for (i, m) in self.materials.iter().enumerate() {
            let Some((layer, name)) = m.id.split_once('/') else {
                return bad(format!("material {:?} is not layer/name", m.id));
            };
            if !LAYERS.contains(&layer) || name.is_empty() {
                return bad(format!("material {:?} names no known layer", m.id));
            }
            if self.materials[..i].iter().any(|o| o.id == m.id) {
                return bad(format!("material {:?} is defined twice", m.id));
            }
            if let Some(e) = &m.element {
                if layer != "catalyst" {
                    return bad(format!(
                        "material {:?}: only catalysts carry an element",
                        m.id
                    ));
                }
                if !["flame", "shadow", "storm", "frost", "stone"].contains(&e.as_str()) {
                    return bad(format!("material {:?}: unknown element {e:?}", m.id));
                }
            }
        }
        for (i, t) in self.templates.iter().enumerate() {
            if t.id.is_empty() || t.id == "component" {
                return bad(format!("template id {:?} is reserved or empty", t.id));
            }
            if self.templates[..i].iter().any(|o| o.id == t.id) {
                return bad(format!("template {:?} is defined twice", t.id));
            }
            if !["weapon", "armour"].contains(&t.kind.as_str()) {
                return bad(format!("template {:?}: unknown kind {:?}", t.id, t.kind));
            }
            for layer in &t.layers {
                if !LAYERS.contains(&layer.as_str()) {
                    return bad(format!("template {:?}: unknown layer {layer:?}", t.id));
                }
            }
            for needed in ["core", "frame"] {
                if !t.layers.iter().any(|l| l == needed) {
                    return bad(format!("template {:?} must take a {needed}", t.id));
                }
            }
            let edge = self.best_edge(t);
            if edge > MAX_EDGE_PER_MILLE {
                return bad(format!(
                    "template {:?}: the best craft gives {edge} per mille, over the cap of {MAX_EDGE_PER_MILLE}",
                    t.id
                ));
            }
        }
        Ok(())
    }
}

pub fn load_items_str(text: &str) -> Result<ItemContent, ContentError> {
    let content: ItemContent = toml::from_str(text).map_err(|e| ContentError::Toml {
        path: "items.toml".into(),
        source: e,
    })?;
    content.validate()?;
    Ok(content)
}

pub fn load_items(dir: &Path) -> Result<ItemContent, ContentError> {
    let path = dir.join("items.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| ContentError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    load_items_str(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");

    #[test]
    fn shipped_items_load_under_the_edge_cap() {
        let items = load_items(Path::new(DIR)).expect("items load");
        assert!(items.templates.len() >= 6 && items.materials.len() >= 15);
        let best = items
            .templates
            .iter()
            .map(|t| items.best_edge(t))
            .max()
            .unwrap();
        // The best sword: scale 55 + dragonbone 60 + catalyst 30 + whalebone 45 + two opals.
        assert_eq!(best, 250);
    }

    #[test]
    fn an_item_over_the_cap_is_refused() {
        let text = r#"
            [[template]]
            id = "sword"
            kind = "weapon"
            layers = ["core", "frame", "gem"]
            [[material]]
            id = "core/a"
            edge = 150
            [[material]]
            id = "frame/b"
            edge = 60
            [[material]]
            id = "gem/c"
            edge = 25
        "#;
        let err = load_items_str(text).unwrap_err().to_string();
        assert!(err.contains("260 per mille"), "{err}");
        assert!(load_items_str(&text.replace("edge = 25", "edge = 20")).is_ok());
        // A template without a frame, and a material without a layer, are refused.
        assert!(
            load_items_str(&text.replace(r#"["core", "frame", "gem"]"#, r#"["core", "gem"]"#))
                .is_err()
        );
        assert!(load_items_str(&text.replace("core/a", "a")).is_err());
    }
}
