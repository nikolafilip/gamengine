//! The game master's page (GM.md 4): tune the zone's timings while playing, wear any
//! build at once, make a body whole. Open to a character the zone granted it to (`G`, or
//! the menu); what it asks goes to the zone as `FromClient::Gm`, and the zone's answer
//! (a new `Content`, a `GmNews`) is what the page shows: nothing here is decided here.

use gm_core::build::{Build, ContentPack};
use gm_core::tick::TickRate;
use gm_core::tuning::{AbilityTuning, TEMPO_RANGE, Tuning};
use gm_core::vocab::Verb;
use gm_net::control::GmOp;

use crate::character::{self, columns};
use crate::ui::{self, Canvas, Column, Key, ListEvent, Rect, Ui};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Timing,
    Build,
    Body,
}

impl Tab {
    const ALL: [Tab; 3] = [Tab::Timing, Tab::Build, Tab::Body];

    fn label(self) -> &'static str {
        match self {
            Tab::Timing => "Timing",
            Tab::Build => "Build",
            Tab::Body => "Body",
        }
    }
}

/// What the page asked for in a frame.
#[derive(Clone, Debug, PartialEq)]
pub enum GmAction {
    None,
    Close,
    Send(GmOp),
}

/// What the page is shown each frame.
pub struct GmView<'a> {
    pub pack: &'a ContentPack,
    pub own: &'a Build,
    pub tuning: &'a Tuning,
    pub rate: TickRate,
    /// The zone's last word on what was asked.
    pub note: &'a str,
}

/// The numbers one ability shows, in milliseconds as the zone's ticks make them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Shown {
    cast: Option<u32>,
    windup: Option<u32>,
    active: Option<u32>,
    recovery: Option<u32>,
    cooldown: u32,
}

fn shown(pack: &ContentPack, index: usize, rate: TickRate) -> Shown {
    let Some(def) = pack.abilities.get(index) else {
        return Shown::default();
    };
    let a = &def.ability;
    let mut out = Shown {
        cooldown: rate.ticks_to_ms(a.cooldown.ticks),
        ..Default::default()
    };
    let last = a.steps.iter().map(|s| s.at).max().unwrap_or(0);
    if last > 0 {
        out.cast = Some(rate.ticks_to_ms(last));
    }
    for s in &a.steps {
        if let Verb::MeleeArc(m) = &s.verb {
            out.windup = Some(rate.ticks_to_ms(m.timing.windup));
            out.active = Some(rate.ticks_to_ms(m.timing.active));
            out.recovery = Some(rate.ticks_to_ms(m.timing.recovery));
        }
    }
    out
}

/// The sliders of one ability: what is dragged, until it is sent.
#[derive(Clone, Debug, Default, PartialEq)]
struct Draft {
    index: usize,
    cast: f32,
    windup: f32,
    active: f32,
    recovery: f32,
    cooldown: f32,
}

pub struct GmPage {
    pub tab: Tab,
    tempo: f32,
    /// The tempo the zone last told; a slider left alone follows it.
    tempo_told: f32,
    picked: usize,
    draft: Option<Draft>,
    build: Option<Build>,
    build_from: Option<Build>,
    /// A heal of everyone is asked twice.
    sure: bool,
}

impl Default for GmPage {
    fn default() -> GmPage {
        GmPage {
            tab: Tab::Timing,
            tempo: 1.0,
            tempo_told: 1.0,
            picked: 0,
            draft: None,
            build: None,
            build_from: None,
            sure: false,
        }
    }
}

/// Rows the abilities list shows at once.
const LIST_ROWS: usize = 4;

const MS_RANGE: (f32, f32) = (0.0, 2000.0);
const COOLDOWN_RANGE: (f32, f32) = (0.0, 30000.0);

impl GmPage {
    /// The name a UI script waits for (CLIENT.md 9).
    pub fn name(&self) -> &'static str {
        match self.tab {
            Tab::Timing => "gm timing",
            Tab::Build => "gm build",
            Tab::Body => "gm body",
        }
    }

    pub fn frame<C: Canvas>(&mut self, ui: &mut Ui<'_, C>, v: &GmView<'_>) -> GmAction {
        if ui.key(Key::Escape) {
            return GmAction::Close;
        }
        let s = ui.scale;
        let gap = 5.0 * s;
        let h = ui.button_height();
        let (fh, line) = (ui.field_height(), ui.line());
        // What each tab needs, under the 360 units a panel may have (ui::PANEL_HIGH):
        // the build's tab is the character page's editor, as wide and as tall.
        let w = match self.tab {
            Tab::Build => character::panel_width(ui),
            _ => 440.0 * s,
        };
        let most = ui::PANEL_HIGH * s - ui.panel_height(0.0, true);
        let inner = match self.tab {
            Tab::Timing => {
                (h + gap)
                    + (fh + gap)
                    + (h + gap)
                    + (line + gap)
                    + (ui.list_height(LIST_ROWS) + gap)
                    + 2.0 * (fh + gap)
                    + (h + gap)
                    + h
            }
            Tab::Build => {
                ((h + gap) + character::editor_height(ui, v.pack, w - 16.0 * s) + (gap + h))
                    .min(most)
            }
            Tab::Body => (h + gap) + (2.0 * line + gap) + 2.0 * (h + gap) + h,
        };
        let panel = Rect::centred(ui.size(), w, ui.panel_height(inner, true));
        let inner = ui.panel(panel, "game master");
        let mut col = Column::new(inner, gap);
        // The tabs, the zone's last word beside them, and Close at the end of the row.
        let row = col.take(h);
        let tabs = Rect::new(row.x, row.y, row.w * 0.45, row.h);
        let close = Rect::new(row.x + row.w * 0.85, row.y, row.w * 0.15, row.h);
        if ui.button(close, "Close") {
            return GmAction::Close;
        }
        for (tab, r) in Tab::ALL
            .iter()
            .zip(ui.buttons(tabs, &["Timing", "Build", "Body"]))
        {
            let text = if *tab == self.tab {
                format!("[{}]", tab.label())
            } else {
                tab.label().to_string()
            };
            if ui.button(r, &text) {
                self.tab = *tab;
                self.sure = false;
            }
        }
        ui.label(
            row.x + row.w * 0.47,
            row.y + (row.h - ui.line()) * 0.5,
            row.w * 0.36,
            ui::WARN,
            v.note,
        );
        match self.tab {
            Tab::Timing => self.timing(ui, &mut col, v),
            Tab::Build => self.build(ui, &mut col, v),
            Tab::Body => self.body(ui, &mut col),
        }
    }

    fn timing<C: Canvas>(
        &mut self,
        ui: &mut Ui<'_, C>,
        col: &mut Column,
        v: &GmView<'_>,
    ) -> GmAction {
        let s = ui.scale;
        let gap = 5.0 * s;
        let h = ui.button_height();
        // The tempo: a slider that follows the zone until it is dragged, sent on release.
        if v.tuning.tempo != self.tempo_told {
            self.tempo_told = v.tuning.tempo;
            self.tempo = v.tuning.tempo;
        }
        let label = "tempo  (every windup, window and cast time; 1 = as authored)";
        ui.slider(
            col.take(ui.field_height()),
            label,
            &mut self.tempo,
            TEMPO_RANGE,
        );
        self.tempo = (self.tempo * 20.0).round() / 20.0;
        let row = ui.buttons(col.take(h), &["Set tempo", "Reset all tuning"]);
        if ui.button_if(
            row[0],
            "Set tempo",
            (self.tempo - v.tuning.tempo).abs() > 1e-3,
        ) {
            return GmAction::Send(GmOp::Tempo(self.tempo));
        }
        if ui.button_if(row[1], "Reset all tuning", !v.tuning.is_default()) {
            self.draft = None;
            return GmAction::Send(GmOp::ResetTuning);
        }
        // The abilities with their numbers as the zone runs them now.
        let rows: Vec<Vec<String>> = v
            .pack
            .abilities
            .iter()
            .enumerate()
            .map(|(i, def)| {
                let n = shown(v.pack, i, v.rate);
                let tuned = v.tuning.ability(&def.key).is_some();
                let opt = |x: Option<u32>| x.map_or("-".to_string(), |x| x.to_string());
                vec![
                    format!("{}{}", def.ability.name, if tuned { " *" } else { "" }),
                    opt(n.cast),
                    format!("{}/{}/{}", opt(n.windup), opt(n.active), opt(n.recovery)),
                    n.cooldown.to_string(),
                ]
            })
            .collect();
        let head = col.take(ui.line());
        for (x, text) in [
            (0.0, "ability"),
            (0.40, "cast ms"),
            (0.54, "windup/active/recover"),
            (0.86, "cooldown"),
        ] {
            ui.label(head.x + head.w * x, head.y, head.w * 0.3, ui::FAINT, text);
        }
        ui.focus_default("list", "abilities");
        let event = ui.list(
            col.take(ui.list_height(LIST_ROWS)),
            "abilities",
            &[0.0, 0.40, 0.54, 0.86],
            &rows,
            &mut self.picked,
        );
        if event == ListEvent::Picked || self.draft.as_ref().is_none_or(|d| d.index != self.picked)
        {
            self.draft = Some(Self::draft_of(v, self.picked));
        }
        let Some(def) = v.pack.abilities.get(self.picked) else {
            return GmAction::None;
        };
        let now = shown(v.pack, self.picked, v.rate);
        let Some(d) = self.draft.as_mut() else {
            return GmAction::None;
        };
        let fh = ui.field_height();
        // Two rows of sliders: the cast time and the cooldown, then a swing's three windows.
        let slider = |ui: &mut Ui<'_, C>, r: Rect, what: &str, value: &mut f32, range| {
            ui.slider(r, what, value, range);
            *value = (*value / 10.0).round() * 10.0;
        };
        let top = columns(col.take(fh), 2, gap);
        if now.cast.is_some() {
            slider(ui, top[0], "cast ms", &mut d.cast, MS_RANGE);
        }
        slider(ui, top[1], "cooldown ms", &mut d.cooldown, COOLDOWN_RANGE);
        let swing = columns(col.take(fh), 3, gap);
        if now.windup.is_some() {
            slider(ui, swing[0], "windup ms", &mut d.windup, MS_RANGE);
            slider(ui, swing[1], "active ms", &mut d.active, MS_RANGE);
            slider(ui, swing[2], "recovery ms", &mut d.recovery, MS_RANGE);
        }
        let row = ui.buttons(col.take(h), &["Set ability", "Forget ability"]);
        let changed = Self::draft_of(v, self.picked) != *d;
        if ui.button_if(row[0], "Set ability", changed) {
            let same = |a: Option<u32>, b: f32| a.is_some_and(|a| a == b.round() as u32);
            let set =
                |a: Option<u32>, b: f32| (a.is_some() && !same(a, b)).then_some(b.round() as u32);
            let kept = v.tuning.ability(&def.key).cloned().unwrap_or_default();
            let a = AbilityTuning {
                key: def.key.clone(),
                cast_ms: set(now.cast, d.cast).or(kept.cast_ms),
                windup_ms: set(now.windup, d.windup).or(kept.windup_ms),
                active_ms: set(now.active, d.active).or(kept.active_ms),
                recovery_ms: set(now.recovery, d.recovery).or(kept.recovery_ms),
                cooldown_ms: set(Some(now.cooldown), d.cooldown).or(kept.cooldown_ms),
            };
            self.draft = None;
            return GmAction::Send(GmOp::Ability(a));
        }
        if ui.button_if(
            row[1],
            "Forget ability",
            v.tuning.ability(&def.key).is_some(),
        ) {
            self.draft = None;
            return GmAction::Send(GmOp::Ability(AbilityTuning {
                key: def.key.clone(),
                ..Default::default()
            }));
        }
        GmAction::None
    }

    fn draft_of(v: &GmView<'_>, index: usize) -> Draft {
        let n = shown(v.pack, index, v.rate);
        Draft {
            index,
            cast: n.cast.unwrap_or(0) as f32,
            windup: n.windup.unwrap_or(0) as f32,
            active: n.active.unwrap_or(0) as f32,
            recovery: n.recovery.unwrap_or(0) as f32,
            cooldown: n.cooldown as f32,
        }
    }

    fn build<C: Canvas>(
        &mut self,
        ui: &mut Ui<'_, C>,
        col: &mut Column,
        v: &GmView<'_>,
    ) -> GmAction {
        let h = ui.button_height();
        // The draft starts from what is worn, and starts over when that changes under it.
        if self.build_from.as_ref() != Some(v.own) {
            self.build_from = Some(v.own.clone());
            self.build = Some(v.own.clone());
        }
        let Some(b) = self.build.as_mut() else {
            return GmAction::None;
        };
        let gap = 5.0 * ui.scale;
        let rest = col.rest();
        character::build_editor(
            ui,
            Rect::new(rest.x, rest.y, rest.w, rest.h - gap - h),
            v.pack,
            v.rate,
            b,
        );
        // The buttons, and what the pack says of the draft when it is not whole.
        let (valid, verdict) = character::verdict(v.pack, b);
        let row = Rect::new(rest.x, rest.y + rest.h - h, rest.w, h);
        let buttons = Rect::new(row.x, row.y, row.w * 0.3, row.h);
        if !valid {
            ui.label(
                row.x + row.w * 0.32,
                row.y + (row.h - ui.line()) * 0.5,
                row.w * 0.68,
                ui::WARN,
                &verdict,
            );
        }
        let row = ui.buttons(buttons, &["Wear it now", "Back to worn"]);
        if ui.button_if(row[0], "Wear it now", valid && b != v.own) {
            return GmAction::Send(GmOp::Respec(b.clone()));
        }
        if ui.button_if(row[1], "Back to worn", b != v.own) {
            *b = v.own.clone();
        }
        GmAction::None
    }

    fn body<C: Canvas>(&mut self, ui: &mut Ui<'_, C>, col: &mut Column) -> GmAction {
        let h = ui.button_height();
        ui.paragraph(
            col.take(ui.line() * 2.0),
            ui::FAINT,
            "Full health, stamina and focus, every cooldown ready, every status gone. \
             A dead body respawns as it does.",
        );
        if ui.button(col.take(h), "Heal me") {
            return GmAction::Send(GmOp::Heal { everyone: false });
        }
        let label = if self.sure {
            "Heal everyone: sure?"
        } else {
            "Heal everyone here"
        };
        if ui.button(col.take(h), label) {
            if self.sure {
                self.sure = false;
                return GmAction::Send(GmOp::Heal { everyone: true });
            }
            self.sure = true;
        }
        GmAction::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::{Recorder, tidy};
    use crate::ui::{UiInput, UiState};
    use gm_core::sim::test_content;

    fn view<'a>(pack: &'a ContentPack, own: &'a Build, tuning: &'a Tuning) -> GmView<'a> {
        GmView {
            pack,
            own,
            tuning,
            rate: TickRate::TOWN,
            note: "",
        }
    }

    fn frame(page: &mut GmPage, st: &mut UiState, input: &UiInput, v: &GmView<'_>) -> GmAction {
        let mut canvas = Recorder::new(1280.0, 720.0);
        let mut ui = Ui::begin(&mut canvas, st, input, page.name(), 300.0);
        let action = page.frame(&mut ui, v);
        ui.end();
        action
    }

    fn click(page: &mut GmPage, st: &mut UiState, text: &str, v: &GmView<'_>) -> GmAction {
        frame(page, st, &UiInput::default(), v);
        let at = st
            .find(text)
            .unwrap_or_else(|| panic!("nothing says {text:?}"))
            .rect
            .centre();
        let press = UiInput {
            cursor: at,
            pressed: true,
            down: true,
            ..Default::default()
        };
        frame(page, st, &press, v);
        let release = UiInput {
            cursor: at,
            released: true,
            ..Default::default()
        };
        frame(page, st, &release, v)
    }

    #[test]
    fn every_tab_is_whole_at_every_size() {
        let pack = test_content::pack(TickRate::TOWN);
        let own = pack.builds[0].build.clone();
        let tuning = Tuning::default();
        let v = view(&pack, &own, &tuning);
        for size in crate::ui::tests::SIZES {
            for tab in Tab::ALL {
                let (mut page, mut st) = (GmPage::default(), UiState::default());
                page.tab = tab;
                let mut canvas = Recorder::new(size.0, size.1);
                let input = UiInput::default();
                let mut ui = Ui::begin(&mut canvas, &mut st, &input, page.name(), 300.0);
                page.frame(&mut ui, &v);
                ui.end();
                tidy(&canvas, &st, 300.0);
                assert!(
                    st.clipped.is_empty(),
                    "{} at {size:?}: {:?}",
                    page.name(),
                    st.clipped
                );
            }
        }
    }

    #[test]
    fn the_tempo_is_sent_only_once_moved_and_an_ability_once_dragged() {
        let pack = test_content::pack(TickRate::TOWN);
        let own = pack.builds[0].build.clone();
        let tuning = Tuning::default();
        let v = view(&pack, &own, &tuning);
        let (mut page, mut st) = (GmPage::default(), UiState::default());
        // As told: nothing to set, nothing to reset (a button not offered is not seen).
        frame(&mut page, &mut st, &UiInput::default(), &v);
        assert!(st.find("Set tempo").is_none(), "{:?}", st.seen);
        assert!(st.find("Reset all tuning").is_none());
        page.tempo = 1.5;
        assert_eq!(
            click(&mut page, &mut st, "Set tempo", &v),
            GmAction::Send(GmOp::Tempo(1.5))
        );
        // The zone said 1.5: the slider follows, and a reset is offered.
        let told = Tuning {
            tempo: 1.5,
            abilities: Vec::new(),
        };
        let v2 = view(&pack, &own, &told);
        frame(&mut page, &mut st, &UiInput::default(), &v2);
        assert_eq!(page.tempo, 1.5);
        assert_eq!(
            click(&mut page, &mut st, "Reset all tuning", &v2),
            GmAction::Send(GmOp::ResetTuning)
        );
        // An ability's numbers: nothing moved, nothing offered; a windup moved, that alone.
        frame(&mut page, &mut st, &UiInput::default(), &v);
        assert!(st.find("Set ability").is_none());
        let d = page.draft.as_mut().expect("a draft of the picked ability");
        let key = pack.abilities[d.index].key.clone();
        d.windup += 200.0;
        match click(&mut page, &mut st, "Set ability", &v) {
            GmAction::Send(GmOp::Ability(a)) => {
                assert_eq!(a.key, key);
                assert!(a.windup_ms.is_some());
                assert!(a.active_ms.is_none() && a.cooldown_ms.is_none(), "{a:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_preset_is_a_build_and_a_point_moved_is_not() {
        let pack = test_content::pack(TickRate::TOWN);
        let own = pack.builds[0].build.clone();
        let tuning = Tuning::default();
        let v = view(&pack, &own, &tuning);
        let (mut page, mut st) = (GmPage::default(), UiState::default());
        page.tab = Tab::Build;
        frame(&mut page, &mut st, &UiInput::default(), &v);
        assert!(st.shows("0 of 30 points left"), "{:?}", st.seen);
        assert!(st.shows("kit 40/40"), "{:?}", st.seen);
        // Worn already: nothing to wear.
        assert!(st.find("Wear it now").is_none());
        let b = page.build.as_mut().unwrap();
        b.attributes.str_ += 1;
        frame(&mut page, &mut st, &UiInput::default(), &v);
        assert!(st.shows("attributes take 31 of 30 points"), "{:?}", st.seen);
        assert!(st.find("Wear it now").is_none(), "over the points");
        // Back to a preset: wearable, as it is not what is worn.
        page.build = Some(pack.builds[1].build.clone());
        match click(&mut page, &mut st, "Wear it now", &v) {
            GmAction::Send(GmOp::Respec(b)) => assert_eq!(b, pack.builds[1].build),
            other => panic!("{other:?}"),
        }
        // Healing everyone is asked twice.
        page.tab = Tab::Body;
        assert_eq!(
            click(&mut page, &mut st, "Heal everyone here", &v),
            GmAction::None
        );
        assert_eq!(
            click(&mut page, &mut st, "Heal everyone: sure?", &v),
            GmAction::Send(GmOp::Heal { everyone: true })
        );
    }
}
