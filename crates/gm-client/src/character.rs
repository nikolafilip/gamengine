//! The character's page (MATRIX.md 9.1, CLIENT.md 4.6): the thirty attribute points and the
//! kit, edited here and worn at the trainer in the town. The editor itself is shared with
//! the game master's page (GM.md 4), which wears any build at once, anywhere. Nothing is
//! decided here: the zone validates and answers (`FromZone::RespecResult`).

use gm_core::build::{AbilityDef, BUDGET, Build, ContentPack, MAX_ACTIVES, Slot};
use gm_core::matrix::{ArmourClass, Aspects, Attributes, Element};
use gm_core::tick::TickRate;
use gm_core::vocab::{ArchetypeFrame, Guard, MoveKind, Origin, Shape, Trigger, Verb};

use crate::ui::{self, Canvas, Column, Key, ListEvent, Rect, RowMark, Ui};

/// `n` columns across `r` with `gap` between them.
pub fn columns(r: Rect, n: usize, gap: f32) -> Vec<Rect> {
    let w = (r.w - gap * (n as f32 - 1.0)) / n as f32;
    (0..n)
        .map(|i| Rect::new(r.x + i as f32 * (w + gap), r.y, w, r.h))
        .collect()
}

/// The editor's panel, in units: as wide as this where the frame allows (two columns:
/// the body on the left, the kit's lists on the right, every ability in sight), else
/// what the frame has less a margin.
pub const EDITOR_WIDE: f32 = 820.0;
/// The body's column, in units.
const BODY_WIDE: f32 = 296.0;
/// The presets' row has a label before the buttons from this width (units) up.
const PRESET_LABEL_AT: f32 = 700.0;
/// A kit column at least this wide (units) shows the actives in two lists side by
/// side; narrower, in one that scrolls.
const SPLIT_ACTIVES_AT: f32 = 420.0;
/// The attributes' rows: a line and a little, not a button's height (five of them),
/// and a little apart.
const ATTR_ROW_EXTRA: f32 = 2.0;
const ATTR_GAP: f32 = 2.0;

pub fn panel_width<C: Canvas>(ui: &Ui<'_, C>) -> f32 {
    (EDITOR_WIDE * ui.scale).min(ui.size().0 - 16.0 * ui.scale)
}

/// What the body's column takes under the presets: frame, armour, aspects, the
/// attributes' header and five rows.
fn body_height<C: Canvas>(ui: &Ui<'_, C>) -> f32 {
    let s = ui.scale;
    let gap = 5.0 * s;
    let (fh, line) = (ui.field_height(), ui.line());
    3.0 * (fh + gap) + (line + gap) + 5.0 * (line + ATTR_ROW_EXTRA * s) + 4.0 * ATTR_GAP * s
}

/// Rows the kit's tallest list would show with nothing hidden.
fn kit_rows(pack: &ContentPack, split: bool) -> usize {
    let count = |slot: Slot| {
        pack.abilities
            .iter()
            .filter(|d| d.slot == slot && !d.creature)
            .count()
    };
    let actives = count(Slot::Active);
    let actives = if split { actives.div_ceil(2) } else { actives };
    let middle = count(Slot::Secondary) + count(Slot::Guard) + 1;
    // The middle column stacks two lists with a label between: counted in rows.
    count(Slot::Primary).max(actives).max(middle + 2)
}

/// What the editor takes of a panel `w` wide when every list shows all its rows: the
/// presets' row, the taller of its two columns, and two lines that read an ability.
pub fn editor_height<C: Canvas>(ui: &Ui<'_, C>, pack: &ContentPack, w: f32) -> f32 {
    let s = ui.scale;
    let gap = 5.0 * s;
    let (h, line) = (ui.button_height(), ui.line());
    let split = w - BODY_WIDE * s - 2.0 * gap >= SPLIT_ACTIVES_AT * s;
    let kit = (line + gap) + ui.list_height(kit_rows(pack, split));
    (h + gap) + body_height(ui).max(kit) + gap + 2.0 * line
}

/// The build editor across `area`: a preset to start from over both columns; the body
/// on the left (frame, armour, aspects, the five attributes with what each buys); the
/// kit on the right (every list whole where the room allows); and under them a line
/// that reads the ability under the cursor. `b` is the draft; what the caller does
/// with it is the caller's business.
pub fn build_editor<C: Canvas>(
    ui: &mut Ui<'_, C>,
    area: Rect,
    pack: &ContentPack,
    rate: TickRate,
    b: &mut Build,
) {
    let s = ui.scale;
    let gap = 5.0 * s;
    let (h, line) = (ui.button_height(), ui.line());
    let mut col = Column::new(area, gap);
    // A preset to start from: a row of them, the one the draft is lit, "custom" when
    // it is none; a word before them where the row is wide.
    let names: Vec<&str> = pack.builds.iter().map(|nb| nb.name.as_str()).collect();
    let preset = pack
        .builds
        .iter()
        .position(|nb| &nb.build == b)
        .unwrap_or(names.len());
    let mut options = names.clone();
    options.push("custom");
    let row = col.take(h);
    let buttons = if area.w >= PRESET_LABEL_AT * s {
        let label_w = ui.text_width("start from") + 2.0 * gap;
        ui.label(
            row.x,
            row.y + (row.h - line) * 0.5,
            label_w,
            ui::FAINT,
            "start from",
        );
        Rect::new(row.x + label_w, row.y, row.w - label_w, row.h)
    } else {
        row
    };
    for (i, r) in ui.buttons(buttons, &options).into_iter().enumerate() {
        let text = if i == preset {
            format!("[{}]", options[i])
        } else {
            options[i].to_string()
        };
        if ui.button(r, &text)
            && let Some(nb) = pack.builds.get(i)
        {
            *b = nb.build.clone();
        }
    }
    // The two columns, and under them what room is left (a line or two) to read an
    // ability in.
    let rest = col.rest();
    let kit_h = (line + gap)
        + ui.list_height(kit_rows(pack, rest.w - BODY_WIDE * s - 2.0 * gap >= SPLIT_ACTIVES_AT * s));
    let columns_h = body_height(ui).max(kit_h).min(rest.h - gap - line);
    let read_lines = (((rest.h - columns_h - gap) / line).floor() as usize).clamp(1, 2);
    let both = Rect::new(rest.x, rest.y, rest.w, columns_h);
    let read = Rect::new(
        rest.x,
        rest.y + rest.h - line * read_lines as f32,
        rest.w,
        line * read_lines as f32,
    );
    let body = Rect::new(both.x, both.y, BODY_WIDE * s, both.h);
    let kit = Rect::new(
        both.x + body.w + 2.0 * gap,
        both.y,
        both.w - body.w - 2.0 * gap,
        both.h,
    );
    body_column(ui, body, b);
    let over = kit_columns(ui, kit, pack, b);
    // What the ability under the cursor does, in the lines there are (the last cut if
    // it must be); else what the lines are for.
    match over.and_then(|i| pack.abilities.get(i as usize)) {
        Some(def) => {
            let text = words_of(def, rate);
            let lines = ui::wrap_measured(&text, read.w, |t| ui.text_width(t));
            for (n, l) in lines.iter().take(read_lines).enumerate() {
                let shown = if n + 1 == read_lines && lines.len() > read_lines {
                    ui.fit_text(read.w, &format!("{l} ..."))
                } else {
                    l.clone()
                };
                ui.label(read.x, read.y + line * n as f32, 0.0, ui::TEXT, &shown);
            }
        }
        None => ui.label(
            read.x,
            read.y,
            read.w,
            ui::FAINT,
            "point at an ability to read what it does",
        ),
    }
}

/// The frame and the armour, the aspects, and the attributes: a name, `-`, the value,
/// `+`, and what the points buy, a row each (MATRIX.md 6).
fn body_column<C: Canvas>(ui: &mut Ui<'_, C>, area: Rect, b: &mut Build) {
    let s = ui.scale;
    let gap = 5.0 * s;
    let (fh, line) = (ui.field_height(), ui.line());
    let mut col = Column::new(area, gap);
    const FRAMES: [ArchetypeFrame; 4] = [
        ArchetypeFrame::Colossus,
        ArchetypeFrame::Striker,
        ArchetypeFrame::Caster,
        ArchetypeFrame::Infiltrator,
    ];
    let mut frame = FRAMES.iter().position(|f| *f == b.frame).unwrap_or(0);
    if ui.choice(
        col.take(fh),
        "frame",
        &["colossus", "striker", "caster", "infiltrator"],
        &mut frame,
    ) {
        b.frame = FRAMES[frame];
    }
    let mut armour = b.armour as usize;
    if ui.choice(
        col.take(fh),
        "armour  (kit points)",
        &["cloth 0", "leather 4", "mail 8", "plate 12"],
        &mut armour,
    ) {
        b.armour = ArmourClass::from_index(armour as u8).unwrap_or_default();
    }
    // Aspects: one or two of five (the second costs 10).
    let row = col.take(fh);
    ui.label(
        row.x,
        row.y,
        row.w,
        ui::FAINT,
        "aspects  (one; a second costs 10 kit points)",
    );
    // Each box as wide as its word needs, the spare shared between them.
    let boxes = Rect::new(row.x, row.y + line, row.w, row.h - line);
    let names = ["flame", "shadow", "storm", "frost", "stone"];
    let side = ui.ascent() + 4.0 * s;
    let needs: Vec<f32> = names
        .iter()
        .map(|n| side + 6.0 * s + ui.text_width(n))
        .collect();
    let spare = ((boxes.w - needs.iter().sum::<f32>()) / 4.0).max(0.0);
    let mut x = boxes.x;
    let cells: Vec<Rect> = needs
        .iter()
        .map(|w| {
            let r = Rect::new(x.round(), boxes.y, w.ceil(), boxes.h);
            x += w + spare;
            r
        })
        .collect();
    for (e, r) in Element::ALL.iter().zip(cells) {
        let mut on = b.aspects.contains(*e);
        if ui.checkbox(r, names[*e as usize], &mut on) {
            let bit = Aspects::one(*e).0;
            b.aspects = Aspects(if on {
                b.aspects.0 | bit
            } else {
                b.aspects.0 & !bit
            });
        }
    }
    // The attributes (5..25, thirty points to spend above 5) under their budget.
    let spent = b.attributes.cost();
    let head = col.take(line);
    // (A draft from elsewhere may be over the points: said so, in the warning's colour.)
    let (budget, ink) = match Attributes::FREE_POINTS.checked_sub(spent) {
        Some(left) => (
            format!("attributes   {left} of {} points left", Attributes::FREE_POINTS),
            ui::FOCUS,
        ),
        None => (
            format!("attributes   {spent} of {} points", Attributes::FREE_POINTS),
            ui::WARN,
        ),
    };
    ui.label(head.x, head.y, head.w, ink, &budget);
    const NAMES: [&str; 5] = ["STR", "AGI", "CON", "INT", "SPR"];
    let mut attrs = b.attributes.as_array();
    let d = b.derived();
    let buys = [
        format!("blows x{:.2}", d.physical_mult),
        format!("speed {:.0} u/s", d.max_speed),
        format!("health {}", d.health),
        format!("spells x{:.2}", d.elemental_mult),
        format!("ward {:.0}%", d.ward * 100.0),
    ];
    // A row a line and a little, less the little where the column is short (the game
    // master's page has a row of tabs over the editor).
    let short = col.rest().h - 4.0 * ATTR_GAP * s;
    let row_h = (line + ATTR_ROW_EXTRA * s).min(short / 5.0).max(line);
    let (name_w, button_w, value_w) = (34.0 * s, 20.0 * s, 26.0 * s);
    let rows = col.take(5.0 * row_h + 4.0 * ATTR_GAP * s);
    for i in 0..5 {
        let row = Rect::new(
            rows.x,
            rows.y + i as f32 * (row_h + ATTR_GAP * s),
            rows.w,
            row_h,
        );
        let text_y = row.y + (row.h - line) * 0.5;
        ui.label(row.x, text_y, name_w, ui::TEXT, NAMES[i]);
        let minus = Rect::new(row.x + name_w, row.y, button_w, row.h);
        let plus = Rect::new(minus.x + button_w + value_w, row.y, button_w, row.h);
        if ui.button_if(minus, "-", attrs[i] > Attributes::MIN) {
            attrs[i] -= 1;
        }
        if ui.button_if(
            plus,
            "+",
            attrs[i] < Attributes::MAX && spent < Attributes::FREE_POINTS,
        ) {
            attrs[i] += 1;
        }
        let value = attrs[i].to_string();
        let vw = ui.text_width(&value);
        ui.label(
            minus.x + button_w + (value_w - vw) * 0.5,
            text_y,
            value_w,
            ui::TEXT,
            &value,
        );
        let words_x = plus.x + button_w + 2.0 * gap;
        ui.label(words_x, text_y, row.x + row.w - words_x, ui::FAINT, &buys[i]);
    }
    let [str_, agi, con, int, spr] = attrs;
    b.attributes = Attributes {
        str_,
        agi,
        con,
        int,
        spr,
    };
}

/// The kit's lists side by side under its budget: the weapons; the secondaries over
/// the guards (or none); the actives, in two lists where there is room. A click puts
/// a row in or takes it out. Returns the ability under the cursor.
fn kit_columns<C: Canvas>(
    ui: &mut Ui<'_, C>,
    area: Rect,
    pack: &ContentPack,
    b: &mut Build,
) -> Option<u16> {
    let s = ui.scale;
    let gap = 5.0 * s;
    let line = ui.line();
    let split = area.w >= SPLIT_ACTIVES_AT * s;
    let of = |slot: Slot| -> Vec<(u16, String, String)> {
        pack.abilities
            .iter()
            .enumerate()
            .filter(|(_, d)| d.slot == slot && !d.creature)
            .map(|(i, d)| (i as u16, d.ability.name.clone(), d.cost.to_string()))
            .collect()
    };
    let (primaries, secondaries, guards, actives) = (
        of(Slot::Primary),
        of(Slot::Secondary),
        of(Slot::Guard),
        of(Slot::Active),
    );
    let cols = columns(area, if split { 4 } else { 3 }, gap);
    // The labels, and the budget over the actives.
    let head_y = area.y;
    ui.label(cols[0].x, head_y, cols[0].w, ui::FAINT, "weapon");
    ui.label(cols[1].x, head_y, cols[1].w, ui::FAINT, "secondary");
    // The budget over the actives, which are the kit's choice; short where narrow.
    let budget = if split {
        format!("actives, up to {MAX_ACTIVES}   kit {}/{BUDGET} points", b.cost(pack))
    } else {
        format!("kit {}/{BUDGET}", b.cost(pack))
    };
    let budget_w = area.x + area.w - cols[2].x;
    ui.label(cols[2].x, head_y, budget_w, ui::FOCUS, &budget);
    // The lists: as many rows as the column has room for, the column's own count at most.
    let lists_y = area.y + line + gap;
    let room = area.y + area.h - lists_y;
    let row_h = line + 2.0 * s;
    let fits = ((room - 2.0 * s) / row_h).floor().max(1.0) as usize;
    let row_height = |rows: usize| rows as f32 * row_h + 2.0 * s;
    let list_rect = move |c: Rect, rows: usize| {
        Rect::new(c.x, lists_y, c.w, row_height(rows.min(fits).max(1)))
    };
    // Name, then the cost in its own column.
    const COLS: [f32; 2] = [0.0, 0.86];
    let rows_of = |list: &[(u16, String, String)], none: bool| -> Vec<Vec<String>> {
        let mut rows: Vec<Vec<String>> = list
            .iter()
            .map(|(_, n, c)| vec![n.clone(), c.clone()])
            .collect();
        if none {
            rows.push(vec!["none".to_string(), String::new()]);
        }
        rows
    };
    let mut over: Option<u16> = None;
    let mut pick = |ui: &mut Ui<'_, C>,
                    r: Rect,
                    name: &str,
                    list: &[(u16, String, String)],
                    current: Option<u16>,
                    none: bool|
     -> Option<Option<u16>> {
        let rows = rows_of(list, none);
        let mut at = match current {
            Some(c) => list.iter().position(|(i, ..)| *i == c).unwrap_or(ui::NONE),
            None if none => rows.len() - 1,
            None => ui::NONE,
        };
        let event = ui.list(r, name, &COLS, &rows, &mut at);
        if let Some(i) = ui.row_over(name) {
            over = list.get(i).map(|(i, ..)| *i);
        }
        match event {
            ListEvent::Picked | ListEvent::Activated => Some(list.get(at).map(|(i, ..)| *i)),
            ListEvent::None => None,
        }
    };
    if let Some(Some(i)) = pick(
        ui,
        list_rect(cols[0], primaries.len()),
        "primary",
        &primaries,
        Some(b.primary),
        false,
    ) {
        b.primary = i;
    }
    // The middle column: the secondaries, then the guards under their own label.
    let sec = list_rect(cols[1], secondaries.len());
    if let Some(Some(i)) = pick(ui, sec, "secondary", &secondaries, Some(b.secondary), false) {
        b.secondary = i;
    }
    let guard_label_y = sec.y + sec.h + gap;
    ui.label(cols[1].x, guard_label_y, cols[1].w, ui::FAINT, "guard");
    let guard_rows = guards.len() + 1;
    let guard_room = area.y + area.h - (guard_label_y + line + gap);
    let guard_fits = ((guard_room - 2.0 * s) / row_h).floor().max(1.0) as usize;
    let guard = Rect::new(
        cols[1].x,
        guard_label_y + line + gap,
        cols[1].w,
        row_height(guard_rows.min(guard_fits)),
    );
    if let Some(g) = pick(ui, guard, "guard", &guards, b.guard, true) {
        b.guard = g;
    }
    // Actives: the ones in the build are lit; a click toggles the row and the list goes
    // back to nothing selected, so the next click on the same row toggles it again.
    let halves: Vec<&[(u16, String, String)]> = if split {
        let (a, z) = actives.split_at(actives.len().div_ceil(2));
        vec![a, z]
    } else {
        vec![&actives[..]]
    };
    for (n, part) in halves.iter().enumerate() {
        let rows: Vec<Vec<String>> = part
            .iter()
            .map(|(_, name, cost)| vec![name.clone(), cost.clone()])
            .collect();
        let marks: Vec<RowMark> = part
            .iter()
            .map(|(i, ..)| {
                if b.actives.contains(i) {
                    RowMark::Picked
                } else {
                    RowMark::Plain
                }
            })
            .collect();
        let name = if n == 0 { "actives" } else { "actives b" };
        let mut at = ui::NONE;
        let r = list_rect(cols[2 + n], part.len());
        if ui.list_marked(r, name, &COLS, &rows, &marks, &mut at) != ListEvent::None
            && let Some((i, ..)) = part.get(at)
        {
            if b.actives.contains(i) {
                b.actives.retain(|a| a != i);
            } else if b.actives.len() < MAX_ACTIVES {
                b.actives.push(*i);
            }
        }
        if let Some(i) = ui.row_over(name) {
            over = part.get(i).map(|(i, ..)| *i);
        }
    }
    over
}

/// Seconds from ticks, short: `14 s`, `0.6 s`.
fn secs(rate: TickRate, ticks: u32) -> String {
    let ms = rate.ticks_to_ms(ticks);
    if ms % 1000 == 0 {
        format!("{} s", ms / 1000)
    } else {
        format!("{:.1} s", ms as f32 / 1000.0)
    }
}

/// An ability in a line a player can read: what it costs, when it is back, and what
/// it does, from its script (VOCABULARY.md 5). Nothing here decides anything; the
/// numbers are the pack's.
pub fn words_of(def: &AbilityDef, rate: TickRate) -> String {
    let a = &def.ability;
    let mut parts = vec![format!("{}: {} pts", a.name, def.cost)];
    if a.cost.stamina > 0 {
        parts.push(format!("{} stamina", a.cost.stamina));
    }
    if a.cost.focus > 0 {
        parts.push(format!("{} focus", a.cost.focus));
    }
    if a.cooldown.ticks > 0 {
        parts.push(format!("{} cooldown", secs(rate, a.cooldown.ticks)));
    }
    if let Some(e) = def.aspect {
        parts.push(format!("needs {}", e.name()));
    }
    let damage = |d: &gm_core::vocab::DamagePacket| -> String {
        let mut w = format!("{} {}", d.amount, d.dtype.name());
        if d.stagger > 0 {
            w.push_str(", staggers");
        }
        w
    };
    let status = |st: &gm_core::vocab::ApplyStatus| -> String {
        format!("{} for {}", st.status.name(), secs(rate, st.duration))
    };
    let shape = |sh: &Shape| -> String {
        match sh {
            Shape::Sphere { radius } | Shape::Cylinder { radius, .. } => {
                format!("{radius:.0} u around")
            }
            Shape::Cone { length, .. } => format!("a cone of {length:.0} u"),
            Shape::Box { half_extents } => format!("{:.0} u across", half_extents[0] * 2.0),
        }
    };
    let origin = |o: &Origin| -> &'static str {
        match o {
            Origin::Aim { .. } => " where you aim",
            Origin::Impact => " where it lands",
            _ => " you",
        }
    };
    let mut does: Vec<String> = Vec::new();
    for step in &a.steps {
        match &step.verb {
            Verb::MeleeArc(m) => {
                let mut w = format!("a swing of {:.0} u: {}", m.reach, damage(&m.damage));
                if m.max_targets > 1 {
                    w.push_str(&format!(", up to {}", m.max_targets));
                }
                does.push(w);
            }
            Verb::Projectile(p) => {
                let mut w = if p.damage.amount > 0 {
                    format!("a bolt: {}", damage(&p.damage))
                } else {
                    "a dart".to_string()
                };
                for t in &p.on_hit {
                    match t {
                        Trigger::Status(st) => w.push_str(&format!(", {} on whom it hits", status(st))),
                        Trigger::Area(ar) => {
                            w.push_str(&format!(", then {}", shape(&ar.shape)));
                            if let Some(d) = &ar.damage {
                                w.push_str(&format!(": {}", damage(d)));
                            }
                            for st in &ar.effects {
                                w.push_str(&format!(", {}", status(st)));
                            }
                        }
                    }
                }
                does.push(w);
            }
            Verb::AreaEffect(ar) => {
                let mut w = format!("{}{}", shape(&ar.shape), origin(&ar.origin));
                if let Some(d) = &ar.damage {
                    w.push_str(&format!(": {}", damage(d)));
                }
                for st in &ar.effects {
                    w.push_str(&format!(", {}", status(st)));
                }
                if ar.duration > 0 {
                    w.push_str(&format!(", for {}", secs(rate, ar.duration)));
                }
                does.push(w);
            }
            Verb::ApplyStatus(st) => does.push(format!("{} on yourself", status(st))),
            Verb::MoveSelf(m) => does.push(match m.kind {
                MoveKind::Dash { speed, duration } => {
                    format!("a dash at {speed:.0} u/s for {}", secs(rate, duration))
                }
                MoveKind::Leap { .. } => "a leap".to_string(),
                MoveKind::Charge { speed, duration, .. } => {
                    format!("a charge at {speed:.0} u/s for {}", secs(rate, duration))
                }
                MoveKind::Blink { distance } => format!("a blink of {distance:.0} u"),
            }),
            Verb::Guard(Guard::Block(bl)) => does.push(format!(
                "a block: {:.0}% less from the front, {} stamina a hit",
                bl.mitigation * 100.0,
                bl.stamina_per_hit
            )),
            Verb::Guard(Guard::Parry(pa)) => does.push(format!(
                "a parry: a window of {}, and the attacker pays for it",
                secs(rate, pa.window)
            )),
        }
    }
    let mut text = parts.join(", ");
    if !does.is_empty() {
        text.push_str(". ");
        text.push_str(&does.join("; "));
    }
    if a.move_scale == 0.0 {
        text.push_str("; you stand still for it");
    } else if a.move_scale < 0.5 {
        text.push_str(&format!("; you move at {:.0}%", a.move_scale * 100.0));
    }
    text
}

/// The pack's word on a draft: the points, the kit's cost, and what is wrong if anything.
pub fn verdict(pack: &ContentPack, b: &Build) -> (bool, String) {
    let points = b.attributes.cost();
    let spent = b.cost(pack);
    match b.validate(pack) {
        Ok(()) => (
            true,
            format!(
                "{points} of {} points, kit {spent} of {BUDGET}",
                Attributes::FREE_POINTS
            ),
        ),
        Err(e) => (false, format!("{e}")),
    }
}

/// What the page asked for in a frame.
#[derive(Clone, Debug, PartialEq)]
pub enum CharacterAction {
    None,
    Close,
    /// Wear this build: the zone decides where and when (MATRIX.md 9.1).
    Wear(Build),
}

/// What the page is shown each frame.
pub struct CharacterView<'a> {
    pub pack: &'a ContentPack,
    /// The build worn now.
    pub own: &'a Build,
    /// The zone's tick rate: the abilities' ticks read in seconds.
    pub rate: TickRate,
    /// Standing by the trainer (within `TRAINER_REACH` of one) in a zone of the world; in a
    /// team zone (the arena) a build is worn at the next respawn, so `true` there.
    pub at_trainer: bool,
    /// The zone's last word on what was asked.
    pub note: &'a str,
}

#[derive(Default)]
pub struct CharacterPage {
    draft: Option<Build>,
    draft_from: Option<Build>,
}

impl CharacterPage {
    /// The name a UI script waits for (CLIENT.md 9).
    pub fn name(&self) -> &'static str {
        "character"
    }

    pub fn frame<C: Canvas>(
        &mut self,
        ui: &mut Ui<'_, C>,
        v: &CharacterView<'_>,
    ) -> CharacterAction {
        if ui.key(Key::Escape) {
            return CharacterAction::Close;
        }
        let s = ui.scale;
        let gap = 5.0 * s;
        let h = ui.button_height();
        // As tall as the editor wants with every list whole, under what a panel may be
        // (ui::PANEL_HIGH): past that the lists scroll.
        let w = panel_width(ui);
        let want = editor_height(ui, v.pack, w - 16.0 * s) + gap + h;
        let most = ui::PANEL_HIGH * s - ui.panel_height(0.0, true);
        let inner = want.min(most);
        let panel = Rect::centred(ui.size(), w, ui.panel_height(inner, true));
        let inner = ui.panel(panel, "character");
        let col = Column::new(inner, gap);
        // The draft starts from what is worn, and starts over when that changes under it.
        if self.draft_from.as_ref() != Some(v.own) {
            self.draft_from = Some(v.own.clone());
            self.draft = Some(v.own.clone());
        }
        let Some(b) = self.draft.as_mut() else {
            return CharacterAction::None;
        };
        let rest = col.rest();
        build_editor(
            ui,
            Rect::new(rest.x, rest.y, rest.w, rest.h - gap - h),
            v.pack,
            v.rate,
            b,
        );
        // The buttons, and beside them what is wrong with the draft, else the zone's
        // word, else where this may be worn.
        let row = Rect::new(rest.x, rest.y + rest.h - h, rest.w, h);
        let (valid, words) = verdict(v.pack, b);
        let note = if !valid {
            words.as_str()
        } else if !v.note.is_empty() {
            v.note
        } else if v.at_trainer {
            ""
        } else {
            "to wear it, stand by the trainer at the town board"
        };
        ui.label(
            row.x + row.w * 0.42,
            row.y + (row.h - ui.line()) * 0.5,
            row.w * 0.58,
            ui::WARN,
            note,
        );
        let buttons = ui.buttons(
            Rect::new(row.x, row.y, row.w * 0.4, row.h),
            &["Wear it", "Back to worn", "Close"],
        );
        if ui.button_if(buttons[0], "Wear it", valid && b != v.own && v.at_trainer) {
            return CharacterAction::Wear(b.clone());
        }
        if ui.button_if(buttons[1], "Back to worn", b != v.own) {
            *b = v.own.clone();
        }
        if ui.button(buttons[2], "Close") {
            return CharacterAction::Close;
        }
        CharacterAction::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::{Recorder, tidy};
    use crate::ui::{UiInput, UiState};
    use gm_core::sim::test_content;

    fn frame(
        page: &mut CharacterPage,
        st: &mut UiState,
        input: &UiInput,
        v: &CharacterView<'_>,
    ) -> CharacterAction {
        let mut canvas = Recorder::new(1280.0, 720.0);
        let mut ui = Ui::begin(&mut canvas, st, input, page.name(), 300.0);
        let action = page.frame(&mut ui, v);
        ui.end();
        action
    }

    fn click(
        page: &mut CharacterPage,
        st: &mut UiState,
        text: &str,
        v: &CharacterView<'_>,
    ) -> CharacterAction {
        frame(page, st, &UiInput::default(), v);
        let at = st
            .find(text)
            .unwrap_or_else(|| panic!("nothing says {text:?}"))
            .rect
            .centre();
        click_at(page, st, at, v)
    }

    /// The `+` or `-` of the attribute called `name`: the button on its row.
    fn click_attr(
        page: &mut CharacterPage,
        st: &mut UiState,
        name: &str,
        button: &str,
        v: &CharacterView<'_>,
    ) -> CharacterAction {
        frame(page, st, &UiInput::default(), v);
        let row = st
            .seen
            .iter()
            .find(|s| s.text == name)
            .unwrap_or_else(|| panic!("no attribute {name:?}"))
            .rect;
        let at = st
            .seen
            .iter()
            .find(|s| s.text == button && (s.rect.centre().1 - row.centre().1).abs() < row.h)
            .unwrap_or_else(|| panic!("no {button:?} on the row of {name:?}: {:?}", st.seen))
            .rect
            .centre();
        click_at(page, st, at, v)
    }

    fn click_at(
        page: &mut CharacterPage,
        st: &mut UiState,
        at: (f32, f32),
        v: &CharacterView<'_>,
    ) -> CharacterAction {
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
    fn the_page_is_whole_at_every_size() {
        let pack = test_content::pack(TickRate::TOWN);
        let own = pack.builds[0].build.clone();
        let v = CharacterView {
            pack: &pack,
            own: &own,
            rate: TickRate::TOWN,
            at_trainer: false,
            note: "",
        };
        for size in crate::ui::tests::SIZES {
            let (mut page, mut st) = (CharacterPage::default(), UiState::default());
            let mut canvas = Recorder::new(size.0, size.1);
            let input = UiInput::default();
            let mut ui = Ui::begin(&mut canvas, &mut st, &input, page.name(), 300.0);
            page.frame(&mut ui, &v);
            ui.end();
            tidy(&canvas, &st, 300.0);
            assert!(st.clipped.is_empty(), "at {size:?}: {:?}", st.clipped);
        }
    }

    #[test]
    fn thirty_points_and_the_trainer() {
        let pack = test_content::pack(TickRate::TOWN);
        let own = pack.builds[1].build.clone();
        let away = CharacterView {
            pack: &pack,
            own: &own,
            rate: TickRate::TOWN,
            at_trainer: false,
            note: "",
        };
        let (mut page, mut st) = (CharacterPage::default(), UiState::default());
        frame(&mut page, &mut st, &UiInput::default(), &away);
        assert!(st.shows("0 of 30 points left"), "{:?}", st.seen);
        assert!(st.shows("kit 30/40"), "{:?}", st.seen);
        // All thirty spent: no `+` is offered; a point taken off STR, and `+` is.
        assert!(st.find("+").is_none(), "{:?}", st.seen);
        assert_eq!(
            click_attr(&mut page, &mut st, "STR", "-", &away),
            CharacterAction::None
        );
        frame(&mut page, &mut st, &UiInput::default(), &away);
        assert!(st.shows("1 of 30 points left"), "{:?}", st.seen);
        assert_eq!(
            click_attr(&mut page, &mut st, "CON", "+", &away),
            CharacterAction::None
        );
        frame(&mut page, &mut st, &UiInput::default(), &away);
        assert!(st.shows("0 of 30 points left"), "{:?}", st.seen);
        // Away from the trainer the build is not worn; beside it, it is.
        assert!(st.find("Wear it").is_none(), "{:?}", st.seen);
        assert!(st.shows("stand by the trainer"));
        let here = CharacterView {
            at_trainer: true,
            ..away
        };
        match click(&mut page, &mut st, "Wear it", &here) {
            CharacterAction::Wear(b) => {
                assert_eq!(b.attributes.con, own.attributes.con + 1);
                assert_eq!(b.attributes.str_, own.attributes.str_ - 1);
            }
            other => panic!("{other:?}"),
        }
        // A kit past its budget is said so, and not worn.
        let b = page.draft.as_mut().unwrap();
        b.armour = ArmourClass::Plate;
        b.actives = ["overhead", "haste", "war_standard", "dash"]
            .iter()
            .map(|k| pack.find(k).unwrap())
            .collect();
        frame(&mut page, &mut st, &UiInput::default(), &here);
        assert!(st.shows("kit costs 52 of 40 points"), "{:?}", st.seen);
        assert!(st.find("Wear it").is_none());
    }
}
