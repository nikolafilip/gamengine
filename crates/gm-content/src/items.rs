//! Item content (ECONOMY.md 4): templates and materials from `items.toml`, with the gear
//! edge cap (PLAN.md 0) checked at load, and what a crafted item does to damage
//! (ITEMS.md 3.2).

use std::path::Path;

use gm_core::matrix::Gear;
use gm_core::vocab::DamageType;
use serde::Deserialize;

use crate::ContentError;

pub const LAYERS: [&str; 5] = ["shard", "core", "catalyst", "frame", "gem"];
/// Gem sockets per item.
pub const MAX_GEMS: u32 = 2;
/// The largest total edge of a fully crafted item over a standard one, per mille.
pub const MAX_EDGE_PER_MILLE: u32 = 250;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemTemplate {
    pub id: String,
    pub kind: String,
    pub layers: Vec<String>,
    /// A weapon: the physical kind it strikes with (`slash`, `pierce`, `blunt`). Its core's
    /// edge sharpens that kind and no other (ITEMS.md 3.2).
    #[serde(default)]
    pub strikes: Option<String>,
    /// An armour: what its core guards against, `physical` (the three kinds of blow) or
    /// `elements` (the five).
    #[serde(default)]
    pub guards: Option<String>,
    /// A stack (MODES.md 11.1): the most of it one holder carries (the strict limit), and
    /// what one of it heals when it is a kit. No layers, no materials.
    #[serde(default)]
    pub cap: Option<u32>,
    #[serde(default)]
    pub heals: Option<i32>,
    /// The look fields (CONTENT.md 3): the prop drawn in the hand of whoever wears an item
    /// of this template, its picture (baked from the model when absent), which hand, how
    /// the file is fitted into the hand and into the first-person view, and whether the
    /// template is retired (nothing new is made of it; what exists keeps its row).
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub held: Option<String>,
    #[serde(default)]
    pub fit: Option<Fit>,
    #[serde(default)]
    pub fit_view: Option<Fit>,
    #[serde(default)]
    pub retired: bool,
}

/// How a prop's file is moved into hand space (CONTENT.md 3.1): metres, degrees about
/// glTF's X, Y and Z in that order, a scale; scaled first, then turned, then moved.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fit {
    #[serde(default, rename = "move")]
    pub mov: [f32; 3],
    #[serde(default)]
    pub turn: [f32; 3],
    #[serde(default = "one")]
    pub scale: f32,
}

fn one() -> f32 {
    1.0
}

impl Default for Fit {
    fn default() -> Fit {
        Fit {
            mov: [0.0; 3],
            turn: [0.0; 3],
            scale: 1.0,
        }
    }
}

impl Eq for ItemTemplate {}

const PHYSICAL: [DamageType; 3] = [DamageType::Slash, DamageType::Pierce, DamageType::Blunt];
const ELEMENTS: [DamageType; 6] = [
    DamageType::Fire,
    DamageType::Water,
    DamageType::Grass,
    DamageType::Electric,
    DamageType::Ground,
    DamageType::Air,
];
/// The nine kinds of damage as a person reads them, in `DamageType`'s order.
const KINDS: [&str; 9] = [
    "slash", "pierce", "blunt", "fire", "water", "grass", "electric", "ground", "air",
];

/// An item as a person is shown it (ITEMS.md 6): everything a screen says about what it
/// is and does, so that no client works any of it out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemView {
    /// Where it is worn; `None` for a part, and for what this content does not know.
    pub place: Option<Place>,
    /// Its edge per damage type, per mille (zeros for what is not worn).
    pub edge: [u16; 9],
    /// What it is: `a weapon, 220 of 250`, `a core, for crafting`.
    pub what: String,
    /// What it does, the strongest first: `slash +11.0%`, `fire +9.5%`.
    pub does: Vec<String>,
}

/// An edge in per mille as what it does (ITEMS.md 3.1): a weapon deals that much more of
/// the kind, an armour takes what `SCALE / (SCALE + edge)` leaves off it; in tenths of a
/// per cent, rounded.
fn per_cent(place: Place, edge: u16) -> String {
    let e = edge as u32;
    match place {
        Place::Weapon => {
            let more = (e * 1000 + Gear::SCALE / 2) / Gear::SCALE;
            format!("+{}.{}%", more / 10, more % 10)
        }
        Place::Armour => {
            let off = (e * 1000 + (Gear::SCALE + e) / 2) / (Gear::SCALE + e);
            format!("-{}.{}%", off / 10, off % 10)
        }
    }
}

/// The edges in words, the strongest first. Kinds of one group that share an edge are
/// said together (`physical`, `elements`, or `other elements` beside one that stands out).
fn words(place: Place, edge: &[u16; 9]) -> Vec<String> {
    let mut said: Vec<(u16, String)> = Vec::new();
    for (group, name) in [(&PHYSICAL[..], "physical"), (&ELEMENTS[..], "elements")] {
        let of = |v: u16| group.iter().filter(|k| edge[**k as usize] == v).count();
        // What most of the group shares, if two or more share anything.
        let shared = group
            .iter()
            .map(|k| edge[*k as usize])
            .filter(|v| *v > 0 && of(*v) >= 2)
            .max_by_key(|v| (of(*v), *v));
        let mut alone = false;
        for k in group {
            let v = edge[*k as usize];
            if v > 0 && Some(v) != shared {
                said.push((v, KINDS[*k as usize].to_string()));
                alone = true;
            }
        }
        match shared {
            Some(v) if of(v) == group.len() => said.push((v, name.to_string())),
            // All but one, and that one said by its own name: these are the others.
            Some(v) if of(v) == group.len() - 1 && alone => said.push((v, format!("other {name}"))),
            // Some of the group, and no word for which: each by its own name.
            Some(v) => {
                for k in group.iter().filter(|k| edge[**k as usize] == v) {
                    said.push((v, KINDS[*k as usize].to_string()));
                }
            }
            None => {}
        }
    }
    said.sort_by_key(|(v, _)| std::cmp::Reverse(*v));
    said.into_iter()
        .map(|(v, what)| format!("{what} {}", per_cent(place, v)))
        .collect()
}

/// Where an item is worn (ITEMS.md 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Weapon,
    Armour,
}

impl Place {
    pub fn name(self) -> &'static str {
        match self {
            Place::Weapon => "weapon",
            Place::Armour => "armour",
        }
    }

    pub fn parse(name: &str) -> Option<Place> {
        match name {
            "weapon" => Some(Place::Weapon),
            "armour" => Some(Place::Armour),
            _ => None,
        }
    }
}

fn physical(kind: &str) -> Option<DamageType> {
    match kind {
        "slash" => Some(DamageType::Slash),
        "pierce" => Some(DamageType::Pierce),
        "blunt" => Some(DamageType::Blunt),
        _ => None,
    }
}

fn element(name: &str) -> Option<DamageType> {
    match name {
        "fire" => Some(DamageType::Fire),
        "water" => Some(DamageType::Water),
        "grass" => Some(DamageType::Grass),
        "electric" => Some(DamageType::Electric),
        "ground" => Some(DamageType::Ground),
        "air" => Some(DamageType::Air),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
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
    /// The look fields (CONTENT.md 3): a picture for the crafting screens, and the tint a
    /// worn item's texture takes when this is its core.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub tint: Option<[f32; 3]>,
}

impl Eq for Material {}

impl Material {
    pub fn layer(&self) -> &str {
        self.id.split('/').next().unwrap_or("")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
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

    /// Where an item of this template is worn; `None` for a component, and for a template
    /// this content does not know.
    pub fn place(&self, template: &str) -> Option<Place> {
        let t = self.templates.iter().find(|t| t.id == template)?;
        Place::parse(&t.kind)
    }

    /// The prop an item of this template puts in the hand (CONTENT.md 3): what a build's
    /// abilities must hold for the item to be worn as a weapon (ITEMS.md 2). `None` for a
    /// template without a model, or one this content does not know.
    pub fn model(&self, template: &str) -> Option<&str> {
        self.templates
            .iter()
            .find(|t| t.id == template)?
            .model
            .as_deref()
    }

    /// A stack template (MODES.md 11.1): its cap and what one of it heals; `None` for
    /// anything else.
    pub fn stack(&self, template: &str) -> Option<(u32, Option<i32>)> {
        let t = self.templates.iter().find(|t| t.id == template)?;
        (t.kind == "stack").then(|| (t.cap.unwrap_or(1).max(1), t.heals))
    }

    /// What a stack of `quantity` is, in words: `balls ×12 of 30`.
    pub fn stack_words(&self, template: &str, quantity: u32) -> String {
        match self.stack(template) {
            Some((cap, _)) => format!("{template} ×{quantity} of {cap}"),
            None => template.to_string(),
        }
    }

    /// What an item of `template` made of `materials` does (ITEMS.md 3.2): where it is
    /// worn, its edge per damage type in `DamageType`'s order (per mille: a weapon's is
    /// what its wearer deals more of, an armour's what its wearer takes less of), and its
    /// whole edge. Each layer puts its material's edge into some of the eight:
    ///
    /// - the core into what the item is made for: the kind a weapon strikes with, the
    ///   three kinds of blow or the five elements an armour guards against;
    /// - the catalyst into its element;
    /// - shard, frame and gems into every kind the core and the catalyst reach, and no
    ///   other: an item is for one build and not for another.
    ///
    /// A material this content does not know, or of a layer the template has no room for,
    /// adds nothing.
    pub fn edges<'a>(
        &self,
        template: &str,
        materials: impl IntoIterator<Item = &'a str>,
    ) -> Option<(Place, [u16; 9], u16)> {
        let t = self.templates.iter().find(|t| t.id == template)?;
        let place = Place::parse(&t.kind)?;
        let known: Vec<&Material> = materials
            .into_iter()
            .filter_map(|id| self.materials.iter().find(|m| m.id == id))
            .filter(|m| t.layers.iter().any(|l| l == m.layer()))
            .collect();
        // What the item is for.
        let mut made_for = [false; 9];
        match place {
            Place::Weapon => {
                if let Some(kind) = t.strikes.as_deref().and_then(physical) {
                    made_for[kind as usize] = true;
                }
            }
            Place::Armour => {
                let kinds = match t.guards.as_deref() {
                    Some("elements") => &ELEMENTS[..],
                    _ => &PHYSICAL[..],
                };
                for kind in kinds {
                    made_for[*kind as usize] = true;
                }
            }
        }
        let core = made_for;
        for m in &known {
            if m.layer() == "catalyst"
                && let Some(e) = m.element.as_deref().and_then(element)
            {
                made_for[e as usize] = true;
            }
        }
        let mut out = [0u32; 9];
        let mut whole = 0u32;
        for m in &known {
            whole += m.edge;
            let mut into = [false; 9];
            match m.layer() {
                "core" => into = core,
                "catalyst" => {
                    if let Some(e) = m.element.as_deref().and_then(element) {
                        into[e as usize] = true;
                    }
                }
                _ => into = made_for,
            }
            for (kind, on) in into.iter().enumerate() {
                if *on {
                    out[kind] += m.edge;
                }
            }
        }
        Some((
            place,
            out.map(|e| e.min(MAX_EDGE_PER_MILLE) as u16),
            whole.min(u16::MAX as u32) as u16,
        ))
    }

    /// The item as a person is shown it. `components` are `(layer, material)`.
    pub fn view<'a>(
        &self,
        template: &str,
        components: impl IntoIterator<Item = (&'a str, &'a str)> + Clone,
    ) -> ItemView {
        let materials = components.clone().into_iter().map(|(_, m)| m);
        match self.edges(template, materials) {
            Some((place, edge, whole)) => ItemView {
                place: Some(place),
                edge,
                what: format!(
                    "{} {}, {whole} of {MAX_EDGE_PER_MILLE}",
                    if place == Place::Weapon { "a" } else { "an" },
                    place.name()
                ),
                does: words(place, &edge),
            },
            None => match self.stack(template) {
                Some((_, heals)) => ItemView {
                    what: format!("{template}, a stack"),
                    does: heals
                        .into_iter()
                        .map(|h| format!("heals {h}, used with F"))
                        .collect(),
                    ..ItemView::default()
                },
                None => ItemView {
                    what: match components.into_iter().next() {
                        Some((layer, _)) if template == "component" => {
                            format!("a {layer}, for crafting")
                        }
                        _ => "nothing this world knows".to_string(),
                    },
                    ..ItemView::default()
                },
            },
        }
    }

    /// The layers a craft of this template may fill; `None`: the content does not know it.
    pub fn layers(&self, template: &str) -> Option<&[String]> {
        self.templates
            .iter()
            .find(|t| t.id == template)
            .map(|t| &t.layers[..])
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
                if element(e).is_none() {
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
            if !["weapon", "armour", "stack"].contains(&t.kind.as_str()) {
                return bad(format!("template {:?}: unknown kind {:?}", t.id, t.kind));
            }
            if t.kind == "stack" {
                // A stack (MODES.md 11.1): a cap within the hub's column, a heal within
                // reason, nothing of a craft.
                match t.cap {
                    Some(1..=1000) => {}
                    _ => return bad(format!("template {:?}: a stack's cap is 1–1000", t.id)),
                }
                if let Some(h) = t.heals
                    && !(1..=1000).contains(&h)
                {
                    return bad(format!("template {:?}: heals is 1–1000", t.id));
                }
                if !t.layers.is_empty() || t.strikes.is_some() || t.guards.is_some() {
                    return bad(format!(
                        "template {:?}: a stack has no layers, strikes with nothing and guards against nothing",
                        t.id
                    ));
                }
                continue;
            }
            if t.cap.is_some() || t.heals.is_some() {
                return bad(format!(
                    "template {:?}: only a stack has a cap or heals",
                    t.id
                ));
            }
            match (t.kind.as_str(), t.strikes.as_deref(), t.guards.as_deref()) {
                ("weapon", Some(kind), None) if physical(kind).is_some() => {}
                ("weapon", _, _) => {
                    return bad(format!(
                        "template {:?}: a weapon strikes with slash, pierce or blunt, and guards against nothing",
                        t.id
                    ));
                }
                (_, None, Some("physical" | "elements")) => {}
                _ => {
                    return bad(format!(
                        "template {:?}: an armour guards against physical or elements, and strikes with nothing",
                        t.id
                    ));
                }
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
        assert!(items.templates.len() >= 7 && items.materials.len() >= 15);
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
    fn each_layer_sharpens_its_own_kind_of_damage() {
        let items = load_items(Path::new(DIR)).expect("items load");
        let at = |t: DamageType| t as usize;
        // The best sword, with an ember catalyst (ITEMS.md 3.2): its core is for the
        // sword's own kind, its catalyst for its element, the rest for those two.
        let best = [
            "shard/boss_scale",
            "core/dragonbone",
            "catalyst/ember",
            "frame/whalebone",
            "gem/opal",
            "gem/opal",
        ];
        let (place, sword, whole) = items.edges("sword", best).unwrap();
        assert_eq!((place, whole), (Place::Weapon, 250));
        let mut expect = [0u16; 9];
        expect[at(DamageType::Slash)] = 220;
        expect[at(DamageType::Fire)] = 190;
        assert_eq!(sword, expect, "nothing for a kind the sword is not for");
        // The same parts in a hammer sharpen Blunt, not Slash.
        let (_, hammer, _) = items.edges("hammer", best).unwrap();
        assert_eq!(
            (hammer[at(DamageType::Blunt)], hammer[at(DamageType::Slash)]),
            (220, 0)
        );
        // A cuirass guards against blows and takes no catalyst: one handed to it (an item
        // made before the rule) adds nothing.
        let plain = [
            "shard/boss_scale",
            "core/dragonbone",
            "frame/whalebone",
            "gem/opal",
            "gem/opal",
        ];
        let (place, cuirass, whole) = items.edges("cuirass", plain).unwrap();
        assert_eq!((place, whole), (Place::Armour, 220));
        assert_eq!(cuirass, [220, 220, 220, 0, 0, 0, 0, 0, 0]);
        assert_eq!(items.edges("cuirass", best).unwrap().1, cuirass);
        // A robe guards against the elements, and its catalyst against one the more.
        let (_, robe, whole) = items.edges("robe", best).unwrap();
        assert_eq!(whole, 250);
        assert_eq!(robe, [0, 0, 0, 250, 220, 220, 220, 220, 220]);
        let (_, robe, _) = items
            .edges("robe", ["core/iron", "catalyst/rime", "frame/oak"])
            .unwrap();
        assert_eq!(robe[at(DamageType::Water)], 70);
        assert_eq!(robe[at(DamageType::Fire)], 40);
        assert_eq!(robe[at(DamageType::Slash)], 0);
        // A component is worn nowhere; nor is what the content does not know. A material it
        // does not know adds nothing.
        assert_eq!(items.edges("component", ["core/iron"]), None);
        assert_eq!(items.place("sword"), Some(Place::Weapon));
        assert_eq!(items.place("lute"), None);
        let (_, odd, _) = items
            .edges("sword", ["core/iron", "core/moonsilver"])
            .unwrap();
        assert_eq!(odd[at(DamageType::Slash)], 30);
        // Nothing an item can be made of goes past the cap in any one number.
        for t in items.templates.iter().filter(|t| t.kind != "stack") {
            let all: Vec<&str> = items.materials.iter().map(|m| m.id.as_str()).collect();
            let (_, edges, _) = items.edges(&t.id, all).unwrap();
            assert!(edges.iter().all(|e| *e <= 250), "{}", t.id);
        }
        // A stack (MODES.md 11.1) is worn nowhere, has a cap, and a kit heals.
        assert_eq!(items.place("ball"), None);
        assert_eq!(items.edges("ball", []), None);
        assert_eq!(items.stack("ball"), Some((30, None)));
        assert_eq!(items.stack("kit"), Some((5, Some(300))));
        assert_eq!(items.stack("sword"), None);
        assert_eq!(items.stack_words("ball", 12), "ball ×12 of 30");
        assert_eq!(
            items.view("kit", []).does,
            vec!["heals 300, used with F".to_string()]
        );
    }

    #[test]
    fn an_item_is_put_into_words_by_the_content() {
        let items = load_items(Path::new(DIR)).expect("items load");
        let parts = |list: &[&'static str]| -> Vec<(&'static str, &'static str)> {
            list.iter()
                .map(|m| (m.split_once('/').unwrap().0, *m))
                .collect()
        };
        let best = parts(&[
            "shard/boss_scale",
            "core/dragonbone",
            "catalyst/ember",
            "frame/whalebone",
            "gem/opal",
            "gem/opal",
        ]);
        // A place counts for half its item's edge (ITEMS.md 3.1): 220 per mille is 11%.
        let sword = items.view("sword", best.clone());
        assert_eq!(sword.what, "a weapon, 250 of 250");
        assert_eq!(sword.does, ["slash +11.0%", "fire +9.5%"]);
        // An armour takes off what 2000 / 2220 leaves: 9.9%.
        let cuirass = items.view("cuirass", parts(&["core/dragonbone", "frame/whalebone"]));
        assert_eq!(cuirass.what, "an armour, 105 of 250");
        assert_eq!(cuirass.does, ["physical -5.0%"]);
        let robe = items.view("robe", best.clone());
        assert_eq!(robe.does, ["fire -11.1%", "other elements -9.9%"]);
        let robe = items.view("robe", parts(&["core/iron", "frame/oak"]));
        assert_eq!(robe.does, ["elements -2.0%"]);
        // Half a per mille is rounded, not lost: 45 is 2.3%.
        let staff = items.view("staff", parts(&["core/tin", "frame/ash", "gem/quartz"]));
        assert_eq!(staff.does, ["blunt +2.3%"]);
        // A part says what it is; what nobody knows says so.
        let part = items.view("component", parts(&["catalyst/ember"]));
        assert_eq!(
            (part.place, part.what.as_str()),
            (None, "a catalyst, for crafting")
        );
        assert!(part.does.is_empty() && part.edge == [0; 9]);
        assert_eq!(items.view("lute", best).what, "nothing this world knows");
        assert_eq!(items.layers("crossbow").unwrap().len(), 4);
        assert_eq!(items.layers("lute"), None);
    }

    #[test]
    fn an_item_over_the_cap_is_refused() {
        let text = r#"
            [[template]]
            id = "sword"
            kind = "weapon"
            strikes = "slash"
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
        // A weapon says what it strikes with, and only a weapon does.
        let ok = text.replace("edge = 25", "edge = 20");
        assert!(load_items_str(&ok.replace("strikes = \"slash\"", "")).is_err());
        assert!(load_items_str(&ok.replace("strikes = \"slash\"", "strikes = \"fire\"")).is_err());
        assert!(load_items_str(&ok.replace("kind = \"weapon\"", "kind = \"armour\"")).is_err());
        // An armour says what it guards against, and only an armour does.
        let armour = ok.replace("kind = \"weapon\"", "kind = \"armour\"");
        let guards = armour.replace("strikes = \"slash\"", "guards = \"elements\"");
        assert!(load_items_str(&guards).is_ok());
        assert!(load_items_str(&guards.replace("elements", "everything")).is_err());
        assert!(
            load_items_str(&ok.replace(
                "strikes = \"slash\"",
                "strikes = \"slash\"\nguards = \"physical\""
            ))
            .is_err()
        );
    }
}
