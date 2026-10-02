//! The toolkit the screens are made of (CLIENT.md 3). Immediate mode: a screen is a function
//! that runs every frame, lays its widgets out and learns at once what was clicked. Nothing
//! is kept between frames but the focus, the carets, the scroll positions and what the last
//! frame showed. Everything is drawn with two primitives, a rectangle and a line of text, so
//! the toolkit runs against the HUD and, in tests, against a recorder.

use std::collections::HashMap;

use crate::font::{self, ADVANCE, GLYPH_H};

/// What the toolkit draws with.
pub trait Canvas {
    /// The frame's size in pixels.
    fn size(&self) -> (f32, f32);
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]);
    fn text(&mut self, x: f32, y: f32, scale: f32, color: [f32; 4], text: &str);
}

pub const PLATE: [f32; 4] = [0.06, 0.06, 0.08, 0.88];
pub const EDGE: [f32; 4] = [0.36, 0.36, 0.46, 1.0];
pub const WELL: [f32; 4] = [0.02, 0.02, 0.03, 0.90];
pub const BUTTON: [f32; 4] = [0.17, 0.17, 0.24, 1.0];
pub const BUTTON_HOT: [f32; 4] = [0.26, 0.26, 0.37, 1.0];
pub const BUTTON_DOWN: [f32; 4] = [0.34, 0.34, 0.48, 1.0];
pub const PICKED: [f32; 4] = [0.22, 0.26, 0.40, 1.0];
pub const TEXT: [f32; 4] = [0.92, 0.92, 0.88, 1.0];
pub const FAINT: [f32; 4] = [0.60, 0.60, 0.62, 1.0];
pub const OFF: [f32; 4] = [0.40, 0.40, 0.42, 1.0];
pub const FOCUS: [f32; 4] = [0.90, 0.75, 0.20, 1.0];
pub const WARN: [f32; 4] = [0.95, 0.55, 0.15, 1.0];

/// The tallest panel any screen draws, in units (the new character's is 283).
pub const PANEL_HIGH: f32 = 300.0;

/// The one scale HUD and screens are drawn at: whole dots, larger on larger frames, or the
/// one somebody chose (1 to 4; 0: by the frame), and never so large that a panel `need`
/// units wide and the tallest one would not fit.
pub fn scale_for(size: (f32, f32), need: f32, chosen: u8) -> f32 {
    let mut s: f32 = match (chosen, size.1) {
        (0, h) if h < 540.0 => 1.0,
        (0, h) if h < 1000.0 => 2.0,
        (0, h) if h < 1600.0 => 3.0,
        (0, _) => 4.0,
        (n, _) => n.min(4) as f32,
    };
    while s > 1.0 && (need * s > size.0 || PANEL_HIGH * s > size.1) {
        s -= 1.0;
    }
    s
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    /// A rectangle of this size in the middle of a frame.
    pub fn centred(size: (f32, f32), w: f32, h: f32) -> Rect {
        Rect::new(
            ((size.0 - w) * 0.5).round(),
            ((size.1 - h) * 0.5).round(),
            w,
            h,
        )
    }

    pub fn contains(&self, p: (f32, f32)) -> bool {
        p.0 >= self.x && p.0 < self.x + self.w && p.1 >= self.y && p.1 < self.y + self.h
    }

    pub fn centre(&self) -> (f32, f32) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    /// The rectangle inside, `pad` away from every edge.
    pub fn inset(&self, pad: f32) -> Rect {
        Rect::new(
            self.x + pad,
            self.y + pad,
            (self.w - 2.0 * pad).max(0.0),
            (self.h - 2.0 * pad).max(0.0),
        )
    }
}

/// Rows handed out from the top of a rectangle downwards.
pub struct Column {
    area: Rect,
    at: f32,
    gap: f32,
}

impl Column {
    pub fn new(area: Rect, gap: f32) -> Column {
        Column {
            area,
            at: area.y,
            gap,
        }
    }

    /// The next row, `h` high and as wide as the column.
    pub fn take(&mut self, h: f32) -> Rect {
        let r = Rect::new(self.area.x, self.at, self.area.w, h);
        self.at += h + self.gap;
        r
    }

    /// What is left below the rows taken so far.
    pub fn rest(&self) -> Rect {
        Rect::new(
            self.area.x,
            self.at,
            self.area.w,
            (self.area.y + self.area.h - self.at).max(0.0),
        )
    }
}

/// A key the screens act on (text arrives as characters, not as keys).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Enter,
    Escape,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

impl Key {
    /// The name a UI script uses (CLIENT.md 9).
    pub fn parse(name: &str) -> Option<Key> {
        Some(match name {
            "Enter" => Key::Enter,
            "Escape" => Key::Escape,
            "Tab" => Key::Tab,
            "BackTab" => Key::BackTab,
            "Backspace" => Key::Backspace,
            "Delete" => Key::Delete,
            "Left" => Key::Left,
            "Right" => Key::Right,
            "Up" => Key::Up,
            "Down" => Key::Down,
            "Home" => Key::Home,
            "End" => Key::End,
            "PageUp" => Key::PageUp,
            "PageDown" => Key::PageDown,
            _ => return None,
        })
    }
}

/// What happened since the last frame, as a screen needs it.
#[derive(Clone, Debug, Default)]
pub struct UiInput {
    pub cursor: (f32, f32),
    /// The left button went down, came up, is down.
    pub pressed: bool,
    pub released: bool,
    pub down: bool,
    /// The press was the second of a double click.
    pub double: bool,
    pub wheel: f32,
    /// Keys pressed, repeats included, in order.
    pub keys: Vec<Key>,
    /// Characters typed, in order.
    pub text: String,
    /// Seconds since the client started: the caret blinks by it.
    pub time: f32,
}

/// What a frame showed: a script finds a button by its text, a test asks what was said.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub kind: SeenKind,
    pub text: String,
    pub rect: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeenKind {
    Label,
    Button,
    Field,
    Row,
    Check,
    Slider,
}

/// What the toolkit keeps between frames.
#[derive(Default)]
pub struct UiState {
    /// The screen drawn last frame: the focus does not survive a change of screen.
    pub screen: String,
    /// The widget that has the keyboard.
    focus: Option<String>,
    /// Caret positions of text fields, in characters.
    carets: HashMap<String, usize>,
    /// First visible row of lists.
    scrolls: HashMap<String, usize>,
    /// The widget the button went down on: a click is a press and a release on the same.
    pressed_on: Option<String>,
    /// Focusable widgets in the order the last frame drew them.
    order: Vec<String>,
    /// The row of a list the last press picked: a double click activates a row only when
    /// both of its presses were on it (the first may have been on a button that put the
    /// list under the pointer).
    last_row: Option<(String, usize)>,
    /// The row each list showed as selected last frame: a selection changed from outside
    /// is scrolled into view.
    selections: HashMap<String, usize>,
    /// Everything the last frame showed.
    pub seen: Vec<Seen>,
    /// Texts the last frame had no room to draw whole (a label cut, a paragraph with
    /// lines left over), titles and list cells apart: a test asks that there are none.
    pub clipped: Vec<String>,
}

impl UiState {
    /// Whether a text field has the keyboard.
    #[cfg(test)]
    pub fn typing(&self) -> bool {
        self.focus.as_ref().is_some_and(|f| f.starts_with("field:"))
    }

    #[cfg(test)]
    pub fn focus(&self) -> Option<&str> {
        self.focus.as_deref()
    }

    /// Something on the screen to click that is called `text`: a button of that name; a
    /// field, a box or a slider with that label; a row whose first cells are that. (Not
    /// anything that merely contains it: `Play` is not the row of a character called
    /// Player while the button is off.)
    pub fn find(&self, text: &str) -> Option<&Seen> {
        let clickable = || self.seen.iter().filter(|s| s.kind != SeenKind::Label);
        clickable().find(|s| s.text == text).or_else(|| {
            clickable().find(|s| {
                s.text
                    .strip_prefix(text)
                    .is_some_and(|rest| rest.starts_with("  ") || rest.starts_with(':'))
            })
        })
    }

    pub fn shows(&self, text: &str) -> bool {
        self.seen.iter().any(|s| s.text.contains(text))
    }
}

/// How a text field behaves.
#[derive(Clone, Copy, Debug)]
pub struct Field {
    /// Shown as dots.
    pub secret: bool,
    pub max_chars: usize,
    /// The hub counts some things in bytes (a name is at most 24).
    pub max_bytes: usize,
    /// Spaces are refused (an email; a password is free to have them).
    pub no_spaces: bool,
    /// Whatever the keyboard gives is taken, drawable by the font or not (a password).
    pub any: bool,
}

impl Field {
    pub const fn text(max_chars: usize) -> Field {
        Field {
            secret: false,
            max_chars,
            max_bytes: usize::MAX,
            no_spaces: false,
            any: false,
        }
    }

    #[cfg(test)]
    pub const fn secret(max_chars: usize) -> Field {
        Field {
            secret: true,
            ..Field::text(max_chars)
        }
    }
}

/// What a list was asked to do this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListEvent {
    None,
    /// Another row was selected.
    Picked,
    /// The selected row was double clicked, or Enter was pressed on it.
    Activated,
}

/// One frame of a screen.
pub struct Ui<'a, C: Canvas> {
    pub canvas: &'a mut C,
    state: &'a mut UiState,
    input: &'a UiInput,
    /// Which of `input.keys` a widget has used.
    used: Vec<bool>,
    text_used: bool,
    pub scale: f32,
    order: Vec<String>,
    seen: Vec<Seen>,
    clipped: Vec<String>,
    /// A row of a list was pressed this frame.
    row_pressed: bool,
}

impl<'a, C: Canvas> Ui<'a, C> {
    /// Start a frame of the screen called `screen`, whose widest panel is `need` units.
    #[cfg(test)]
    pub fn begin(
        canvas: &'a mut C,
        state: &'a mut UiState,
        input: &'a UiInput,
        screen: &str,
        need: f32,
    ) -> Ui<'a, C> {
        Ui::begin_at(canvas, state, input, screen, need, 0)
    }

    /// The same at a scale somebody chose (1 to 4; 0: what the frame's size gives). A
    /// chosen scale still gives way where the panel would not fit.
    pub fn begin_at(
        canvas: &'a mut C,
        state: &'a mut UiState,
        input: &'a UiInput,
        screen: &str,
        need: f32,
        chosen: u8,
    ) -> Ui<'a, C> {
        if state.screen != screen {
            state.screen = screen.to_string();
            state.focus = None;
            state.pressed_on = None;
            state.carets.clear();
            state.scrolls.clear();
            state.selections.clear();
            state.last_row = None;
        }
        let scale = scale_for(canvas.size(), need, chosen);
        Ui {
            canvas,
            used: vec![false; input.keys.len()],
            state,
            input,
            text_used: false,
            scale,
            order: Vec::new(),
            seen: Vec::new(),
            clipped: Vec::new(),
            row_pressed: false,
        }
    }

    pub fn size(&self) -> (f32, f32) {
        self.canvas.size()
    }

    /// The height of a line of text, a button and a field at this scale.
    pub fn line(&self) -> f32 {
        (GLYPH_H + 5.0) * self.scale
    }

    pub fn button_height(&self) -> f32 {
        (GLYPH_H + 10.0) * self.scale
    }

    /// The height of a list that shows `rows` rows.
    pub fn list_height(&self, rows: usize) -> f32 {
        rows as f32 * (self.line() + 2.0 * self.scale) + 2.0 * self.scale
    }

    /// The height of a panel whose inside is `inner` high (see `panel`).
    pub fn panel_height(&self, inner: f32, titled: bool) -> f32 {
        let pad = 8.0 * self.scale;
        let top = if titled {
            pad + self.line() + 2.0 * self.scale
        } else {
            pad
        };
        top + inner + pad
    }

    /// A row of buttons across `area`, each as wide as its text needs and all of them
    /// sharing what room is left; too little room is shared out as well.
    pub fn buttons(&self, area: Rect, labels: &[&str]) -> Vec<Rect> {
        let s = self.scale;
        let gap = 6.0 * s;
        let need: Vec<f32> = labels
            .iter()
            .map(|l| self.text_width(l) + 12.0 * s)
            .collect();
        let room = area.w - gap * labels.len().saturating_sub(1) as f32;
        let total: f32 = need.iter().sum();
        let spare = (room - total) / labels.len().max(1) as f32;
        let squeeze = if total > room && total > 0.0 {
            room / total
        } else {
            1.0
        };
        let mut x = area.x;
        need.iter()
            .map(|n| {
                let w = if squeeze < 1.0 {
                    n * squeeze
                } else {
                    n + spare
                };
                let r = Rect::new(x.round(), area.y, w.floor(), area.h);
                x += w + gap;
                r
            })
            .collect()
    }

    pub fn text_width(&self, text: &str) -> f32 {
        font::text_width(self.scale, text)
    }

    /// Characters that fit in `w` pixels.
    pub fn fit(&self, w: f32) -> usize {
        ((w + self.scale) / (ADVANCE * self.scale)).floor().max(0.0) as usize
    }

    /// A key press no widget has used yet: the screen's own (Enter, Escape).
    pub fn key(&mut self, key: Key) -> bool {
        match (0..self.input.keys.len()).find(|&i| !self.used[i] && self.input.keys[i] == key) {
            Some(i) => {
                self.used[i] = true;
                true
            }
            None => false,
        }
    }

    fn outline(&mut self, r: Rect, color: [f32; 4]) {
        let t = self.scale;
        self.canvas.rect(r.x, r.y, r.w, t, color);
        self.canvas.rect(r.x, r.y + r.h - t, r.w, t, color);
        self.canvas.rect(r.x, r.y, t, r.h, color);
        self.canvas.rect(r.x + r.w - t, r.y, t, r.h, color);
    }

    fn note(&mut self, kind: SeenKind, text: &str, rect: Rect) {
        self.seen.push(Seen {
            kind,
            text: text.to_string(),
            rect,
        });
    }

    /// A plate with an edge and a title; returns the room inside it.
    pub fn panel(&mut self, r: Rect, title: &str) -> Rect {
        let s = self.scale;
        self.canvas.rect(r.x, r.y, r.w, r.h, PLATE);
        self.outline(r, EDGE);
        let pad = 8.0 * s;
        if title.is_empty() {
            return r.inset(pad);
        }
        // A title longer than the panel is cut to it.
        let shown: String = title.chars().take(self.fit(r.w - 2.0 * pad)).collect();
        self.canvas.text(r.x + pad, r.y + pad, s, FOCUS, &shown);
        self.note(
            SeenKind::Label,
            title,
            Rect::new(r.x + pad, r.y + pad, self.text_width(&shown), GLYPH_H * s),
        );
        let top = pad + self.line() + 2.0 * s;
        self.canvas
            .rect(r.x + pad, r.y + top - 4.0 * s, r.w - 2.0 * pad, s, EDGE);
        Rect::new(
            r.x + pad,
            r.y + top,
            (r.w - 2.0 * pad).max(0.0),
            (r.h - top - pad).max(0.0),
        )
    }

    /// A line of text, cut to what fits in `w` pixels when `w` is positive.
    pub fn label(&mut self, x: f32, y: f32, w: f32, color: [f32; 4], text: &str) {
        let shown: String = if w > 0.0 {
            text.chars().take(self.fit(w)).collect()
        } else {
            text.to_string()
        };
        if shown.len() < text.len() {
            self.clipped.push(text.to_string());
        }
        self.canvas.text(x, y, self.scale, color, &shown);
        let rect = Rect::new(x, y, self.text_width(&shown), GLYPH_H * self.scale);
        self.note(SeenKind::Label, text, rect);
    }

    /// Small print, ending at `right`: a word in a corner, at half the scale and never
    /// under one.
    pub fn small(&mut self, right: f32, y: f32, color: [f32; 4], text: &str) {
        let s = (self.scale * 0.5).max(1.0);
        let tw = font::text_width(s, text);
        self.canvas.text(right - tw, y, s, color, text);
        self.note(
            SeenKind::Label,
            text,
            Rect::new(right - tw, y, tw, GLYPH_H * s),
        );
    }

    /// Text wrapped at words into `r`; returns the height used.
    pub fn paragraph(&mut self, r: Rect, color: [f32; 4], text: &str) -> f32 {
        let lines = wrap(text, self.fit(r.w).max(1));
        let line = self.line();
        for (i, l) in lines.iter().enumerate() {
            let y = r.y + line * i as f32;
            if y + GLYPH_H * self.scale > r.y + r.h && r.h > 0.0 {
                self.clipped.push(text.to_string());
                break;
            }
            self.canvas.text(r.x, y, self.scale, color, l);
        }
        self.note(SeenKind::Label, text, r);
        line * lines.len() as f32
    }

    fn focusable(&mut self, id: &str) -> bool {
        self.order.push(id.to_string());
        self.state.focus.as_deref() == Some(id)
    }

    /// Give the keyboard to a widget if nothing has it (a screen's first field).
    pub fn focus_default(&mut self, kind: &str, label: &str) {
        if self.state.focus.is_none() {
            self.state.focus = Some(format!("{kind}:{label}"));
        }
    }

    /// Give the keyboard to a widget.
    pub fn focus(&mut self, kind: &str, label: &str) {
        self.state.focus = Some(format!("{kind}:{label}"));
    }

    /// Press-and-release on `r`: the mouse half of every clickable widget.
    fn clicked(&mut self, id: &str, r: Rect) -> (bool, bool, bool) {
        let hot = r.contains(self.input.cursor);
        if self.input.pressed && hot {
            self.state.pressed_on = Some(id.to_string());
            self.state.focus = Some(id.to_string());
        }
        let held = self.state.pressed_on.as_deref() == Some(id);
        let click = self.input.released && held && hot;
        (hot, held && self.input.down, click)
    }

    pub fn button(&mut self, r: Rect, text: &str) -> bool {
        self.button_if(r, text, true)
    }

    /// A button that can be off: drawn faint, takes no click and no focus.
    pub fn button_if(&mut self, r: Rect, text: &str, enabled: bool) -> bool {
        let s = self.scale;
        let id = format!("button:{text}");
        let (mut pressed, mut fill, mut ink) = (false, BUTTON, TEXT);
        if enabled {
            let focused = self.focusable(&id);
            let (hot, down, click) = self.clicked(&id, r);
            pressed = click || (focused && self.key(Key::Enter));
            fill = match (down, hot) {
                (true, _) => BUTTON_DOWN,
                (false, true) => BUTTON_HOT,
                _ => BUTTON,
            };
            self.canvas.rect(r.x, r.y, r.w, r.h, fill);
            self.outline(r, if focused { FOCUS } else { EDGE });
        } else {
            fill[3] = 0.6;
            ink = OFF;
            self.canvas.rect(r.x, r.y, r.w, r.h, fill);
            self.outline(r, OFF);
        }
        let tw = self.text_width(text);
        self.canvas.text(
            (r.x + (r.w - tw) * 0.5).round(),
            (r.y + (r.h - GLYPH_H * s) * 0.5).round(),
            s,
            ink,
            text,
        );
        if enabled {
            self.note(SeenKind::Button, text, r);
        }
        pressed
    }

    /// One line of text to edit, with its label above it. Returns whether it changed.
    pub fn text_field(&mut self, r: Rect, label: &str, value: &mut String, how: Field) -> bool {
        let s = self.scale;
        let id = format!("field:{label}");
        let focused = self.focusable(&id);
        let label_h = self.line();
        self.canvas.text(r.x, r.y, s, FAINT, label);
        let well = Rect::new(r.x, r.y + label_h, r.w, (r.h - label_h).max(0.0));
        let (_, _, _) = self.clicked(&id, well);
        let focused = focused || self.state.focus.as_deref() == Some(id.as_str());
        let mut chars: Vec<char> = value.chars().collect();
        let mut caret = (*self.state.carets.get(&id).unwrap_or(&chars.len())).min(chars.len());
        let mut changed = false;
        if focused {
            // A click puts the caret where it landed.
            if self.input.pressed && well.contains(self.input.cursor) {
                let first = first_shown(chars.len(), caret, self.fit(well.w - 8.0 * s));
                let col = ((self.input.cursor.0 - well.x - 4.0 * s) / (ADVANCE * s)).round();
                caret = (first + col.max(0.0) as usize).min(chars.len());
            }
            for i in 0..self.input.keys.len() {
                if self.used[i] {
                    continue;
                }
                let done = match self.input.keys[i] {
                    Key::Left => {
                        caret = caret.saturating_sub(1);
                        true
                    }
                    Key::Right => {
                        caret = (caret + 1).min(chars.len());
                        true
                    }
                    Key::Home => {
                        caret = 0;
                        true
                    }
                    Key::End => {
                        caret = chars.len();
                        true
                    }
                    Key::Backspace => {
                        if caret > 0 {
                            caret -= 1;
                            chars.remove(caret);
                            changed = true;
                        }
                        true
                    }
                    Key::Delete => {
                        if caret < chars.len() {
                            chars.remove(caret);
                            changed = true;
                        }
                        true
                    }
                    _ => false,
                };
                self.used[i] |= done;
            }
            if !self.text_used {
                self.text_used = true;
                let mut bytes: usize = chars.iter().map(|c| c.len_utf8()).sum();
                for c in self.input.text.chars() {
                    let fits = chars.len() < how.max_chars && bytes + c.len_utf8() <= how.max_bytes;
                    let known = how.any || font::has_glyph(c);
                    if fits && known && !(how.no_spaces && c == ' ') {
                        chars.insert(caret, c);
                        caret += 1;
                        bytes += c.len_utf8();
                        changed = true;
                    }
                }
            }
        }
        if changed {
            *value = chars.iter().collect();
        }
        self.state.carets.insert(id, caret);

        self.canvas.rect(well.x, well.y, well.w, well.h, WELL);
        self.outline(well, if focused { FOCUS } else { EDGE });
        let room = self.fit(well.w - 8.0 * s);
        let first = first_shown(chars.len(), caret, room);
        let shown: String = chars
            .iter()
            .skip(first)
            .take(room)
            .map(|c| if how.secret { '*' } else { *c })
            .collect();
        let ty = (well.y + (well.h - GLYPH_H * s) * 0.5).round();
        self.canvas.text(well.x + 4.0 * s, ty, s, TEXT, &shown);
        if focused && ((self.input.time * 2.0) as u32).is_multiple_of(2) {
            let cx = well.x + 4.0 * s + (caret - first) as f32 * ADVANCE * s - s;
            self.canvas.rect(cx, ty - s, s, (GLYPH_H + 2.0) * s, FOCUS);
        }
        // A script and a test see the label, and the value unless it is a secret.
        let said = if how.secret {
            label.to_string()
        } else {
            format!("{label}: {value}")
        };
        self.note(SeenKind::Field, &said, well);
        changed
    }

    /// The height a text field needs: its label and its well.
    pub fn field_height(&self) -> f32 {
        self.line() + self.button_height()
    }

    /// Rows of columns, one of them selected. `columns` are where each cell starts, as a
    /// part of the list's width.
    pub fn list(
        &mut self,
        r: Rect,
        name: &str,
        columns: &[f32],
        rows: &[Vec<String>],
        selected: &mut usize,
    ) -> ListEvent {
        let s = self.scale;
        let id = format!("list:{name}");
        let focused = self.focusable(&id);
        let row_h = self.line() + 2.0 * s;
        let room = ((r.h - 2.0 * s) / row_h).floor().max(1.0) as usize;
        let mut event = ListEvent::None;
        let before = *selected;
        if rows.is_empty() {
            *selected = 0;
        } else {
            *selected = (*selected).min(rows.len() - 1);
        }
        let mut first = *self.state.scrolls.get(&id).unwrap_or(&0);
        let hot = r.contains(self.input.cursor);
        if self.input.pressed && hot {
            self.state.focus = Some(id.clone());
        }
        let focused = focused || self.state.focus.as_deref() == Some(id.as_str());
        if focused && !rows.is_empty() {
            let last = rows.len() - 1;
            for i in 0..self.input.keys.len() {
                if self.used[i] {
                    continue;
                }
                let done = match self.input.keys[i] {
                    Key::Up => {
                        *selected = selected.saturating_sub(1);
                        true
                    }
                    Key::Down => {
                        *selected = (*selected + 1).min(last);
                        true
                    }
                    Key::PageUp => {
                        *selected = selected.saturating_sub(room);
                        true
                    }
                    Key::PageDown => {
                        *selected = (*selected + room).min(last);
                        true
                    }
                    Key::Home => {
                        *selected = 0;
                        true
                    }
                    Key::End => {
                        *selected = last;
                        true
                    }
                    Key::Enter => {
                        event = ListEvent::Activated;
                        true
                    }
                    _ => false,
                };
                self.used[i] |= done;
            }
        }
        if hot && self.input.wheel != 0.0 {
            let turn = -self.input.wheel.round() as i64;
            first = (first as i64 + turn).clamp(0, rows.len().saturating_sub(room) as i64) as usize;
        }
        // The selection stays in view when the keys moved it, and when it was changed
        // from outside since this list was last drawn (the character played last, the
        // one just made).
        if *selected != before || self.state.selections.get(&id) != Some(&*selected) {
            if *selected < first {
                first = *selected;
            } else if *selected >= first + room {
                first = *selected + 1 - room;
            }
        }
        first = first.min(rows.len().saturating_sub(room));

        self.canvas.rect(r.x, r.y, r.w, r.h, WELL);
        self.outline(r, if focused { FOCUS } else { EDGE });
        for (i, row) in rows.iter().enumerate().skip(first).take(room) {
            let rr = Rect::new(
                r.x + s,
                r.y + s + row_h * (i - first) as f32,
                r.w - 2.0 * s,
                row_h,
            );
            let over = rr.contains(self.input.cursor);
            if self.input.pressed && over {
                // A double click activates the row both of its presses were on.
                let here = Some((id.clone(), i));
                if self.input.double && self.state.last_row == here {
                    event = ListEvent::Activated;
                }
                *selected = i;
                self.state.last_row = here;
                self.row_pressed = true;
            }
            if i == *selected {
                self.canvas.rect(rr.x, rr.y, rr.w, rr.h, PICKED);
            } else if over {
                self.canvas.rect(rr.x, rr.y, rr.w, rr.h, BUTTON);
            }
            for (c, cell) in row.iter().enumerate() {
                let from = columns.get(c).copied().unwrap_or(0.0);
                let to = columns.get(c + 1).copied().unwrap_or(1.0);
                let x = rr.x + 4.0 * s + (rr.w - 8.0 * s) * from;
                let fit = self.fit((rr.w - 8.0 * s) * (to - from) - 2.0 * s);
                let shown: String = cell.chars().take(fit).collect();
                let ink = if c == 0 { TEXT } else { FAINT };
                self.canvas.text(x, rr.y + 3.0 * s, s, ink, &shown);
            }
            self.note(SeenKind::Row, &row.join("  "), rr);
        }
        // A bar on the right says where in the list the window is.
        if rows.len() > room {
            let track = r.h - 2.0 * s;
            let bar = (track * room as f32 / rows.len() as f32).max(6.0 * s);
            let at = (track - bar) * first as f32 / (rows.len() - room) as f32;
            self.canvas
                .rect(r.x + r.w - 3.0 * s, r.y + s + at, 2.0 * s, bar, EDGE);
        }
        self.state.scrolls.insert(id.clone(), first);
        self.state.selections.insert(id, *selected);
        if event == ListEvent::None && *selected != before {
            event = ListEvent::Picked;
        }
        event
    }

    /// One of a few, in a row under its label: the one picked is lit. Returns whether
    /// another was picked. (A slider would do for a number; for a choice it jumps about
    /// under the hand when what it sets moves the slider itself.)
    pub fn choice(&mut self, r: Rect, label: &str, options: &[&str], picked: &mut usize) -> bool {
        let s = self.scale;
        let label_h = self.line();
        self.canvas.text(r.x, r.y, s, FAINT, label);
        let row = Rect::new(r.x, r.y + label_h, r.w, (r.h - label_h).max(0.0));
        let before = *picked;
        for (i, (option, at)) in options.iter().zip(self.buttons(row, options)).enumerate() {
            let id = format!("choice:{label}:{option}");
            let focused = self.focusable(&id);
            let (hot, down, click) = self.clicked(&id, at);
            if click || (focused && self.key(Key::Enter)) {
                *picked = i;
            }
            let fill = match (i == *picked, down, hot) {
                (true, _, _) => PICKED,
                (false, true, _) => BUTTON_DOWN,
                (false, false, true) => BUTTON_HOT,
                _ => BUTTON,
            };
            self.canvas.rect(at.x, at.y, at.w, at.h, fill);
            self.outline(at, if focused { FOCUS } else { EDGE });
            let tw = self.text_width(option);
            self.canvas.text(
                (at.x + (at.w - tw) * 0.5).round(),
                (at.y + (at.h - GLYPH_H * s) * 0.5).round(),
                s,
                TEXT,
                option,
            );
            let lit = if i == *picked { ": on" } else { "" };
            self.note(SeenKind::Check, &format!("{label} {option}{lit}"), at);
        }
        *picked != before
    }

    /// A box to tick, with its label to the right. Returns whether it changed.
    pub fn checkbox(&mut self, r: Rect, label: &str, value: &mut bool) -> bool {
        let s = self.scale;
        let id = format!("check:{label}");
        let focused = self.focusable(&id);
        let (hot, _, click) = self.clicked(&id, r);
        let changed = click || (focused && self.key(Key::Enter));
        if changed {
            *value = !*value;
        }
        let side = GLYPH_H * s + 4.0 * s;
        let b = Rect::new(r.x, (r.y + (r.h - side) * 0.5).round(), side, side);
        self.canvas
            .rect(b.x, b.y, b.w, b.h, if hot { BUTTON_HOT } else { WELL });
        self.outline(b, if focused { FOCUS } else { EDGE });
        if *value {
            let m = b.inset(3.0 * s);
            self.canvas.rect(m.x, m.y, m.w, m.h, FOCUS);
        }
        self.canvas.text(
            b.x + side + 6.0 * s,
            (r.y + (r.h - GLYPH_H * s) * 0.5).round(),
            s,
            TEXT,
            label,
        );
        let said = format!("{label}: {}", if *value { "on" } else { "off" });
        self.note(SeenKind::Check, &said, r);
        changed
    }

    /// A value between two ends, with its label above. Returns whether it changed.
    pub fn slider(&mut self, r: Rect, label: &str, value: &mut f32, range: (f32, f32)) -> bool {
        let s = self.scale;
        let id = format!("slider:{label}");
        let focused = self.focusable(&id);
        let label_h = self.line();
        let track = Rect::new(r.x, r.y + label_h, r.w, (r.h - label_h).max(0.0));
        let (_, down, _) = self.clicked(&id, track);
        let focused = focused || self.state.focus.as_deref() == Some(id.as_str());
        let before = *value;
        let span = (range.1 - range.0).max(f32::EPSILON);
        if down {
            let t = ((self.input.cursor.0 - track.x) / track.w.max(1.0)).clamp(0.0, 1.0);
            *value = range.0 + span * t;
        }
        if focused {
            if self.key(Key::Left) {
                *value -= span / 20.0;
            }
            if self.key(Key::Right) {
                *value += span / 20.0;
            }
        }
        *value = value.clamp(range.0, range.1);
        let t = (*value - range.0) / span;
        self.canvas
            .text(r.x, r.y, s, FAINT, &format!("{label}  {value:.2}"));
        let mid = track.y + track.h * 0.5;
        self.canvas.rect(
            track.x,
            mid - s,
            track.w,
            2.0 * s,
            if focused { FOCUS } else { EDGE },
        );
        let knob = 6.0 * s;
        self.canvas.rect(
            (track.x + (track.w - knob) * t).round(),
            track.y + 2.0 * s,
            knob,
            (track.h - 4.0 * s).max(s),
            TEXT,
        );
        self.note(SeenKind::Slider, &format!("{label}: {value:.2}"), track);
        *value != before
    }

    /// End the frame: Tab moves the focus through what was drawn, and what was drawn is
    /// kept for scripts and tests.
    pub fn end(mut self) {
        let step = if self.key(Key::Tab) {
            1
        } else if self.key(Key::BackTab) {
            -1
        } else {
            0
        };
        if step != 0 && !self.order.is_empty() {
            let n = self.order.len() as i64;
            let at = self
                .state
                .focus
                .as_ref()
                .and_then(|f| self.order.iter().position(|o| o == f));
            let next = match at {
                Some(i) => (i as i64 + step).rem_euclid(n),
                None if step > 0 => 0,
                None => n - 1,
            };
            self.state.focus = Some(self.order[next as usize].clone());
        }
        // A focus on something this frame did not draw is nobody's.
        if self
            .state
            .focus
            .as_ref()
            .is_some_and(|f| !self.order.contains(f))
        {
            self.state.focus = None;
        }
        if self.input.released {
            self.state.pressed_on = None;
        }
        // A press that was not on a row ends what a double click could finish.
        if self.input.pressed && !self.row_pressed {
            self.state.last_row = None;
        }
        self.state.order = std::mem::take(&mut self.order);
        self.state.seen = std::mem::take(&mut self.seen);
        self.state.clipped = std::mem::take(&mut self.clipped);
    }
}

/// The first character of a field's text that is drawn, so that the caret is in view.
fn first_shown(len: usize, caret: usize, room: usize) -> usize {
    if len <= room || room == 0 {
        0
    } else {
        (caret + 1).saturating_sub(room).min(len - room)
    }
}

/// Text broken at spaces into lines of at most `width` characters; a word longer than a
/// line is cut.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut len = 0;
        for word in paragraph.split_whitespace() {
            let mut word: Vec<char> = word.chars().collect();
            loop {
                let need = word.len() + usize::from(len > 0);
                if len + need <= width {
                    if len > 0 {
                        line.push(' ');
                    }
                    line.extend(word.iter());
                    len += need;
                    break;
                }
                if len > 0 {
                    lines.push(std::mem::take(&mut line));
                    len = 0;
                    continue;
                }
                // A word longer than a line: what fits, and the rest goes on.
                lines.push(word.drain(..width).collect());
            }
        }
        lines.push(line);
    }
    lines
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A canvas that remembers what was drawn on it.
    #[derive(Default)]
    pub struct Recorder {
        pub size: (f32, f32),
        pub rects: Vec<(Rect, [f32; 4])>,
        pub texts: Vec<(f32, f32, String)>,
    }

    impl Recorder {
        pub fn new(w: f32, h: f32) -> Recorder {
            Recorder {
                size: (w, h),
                ..Default::default()
            }
        }
    }

    /// The frame sizes a screen must be whole in: a small laptop, the common ones, 4K,
    /// and the gate's own small window.
    pub const SIZES: [(f32, f32); 5] = [
        (640.0, 360.0),
        (1024.0, 600.0),
        (1366.0, 768.0),
        (1920.0, 1080.0),
        (3840.0, 2160.0),
    ];

    /// What a string comparison cannot see: everything a screen showed lies inside its
    /// panel (the first plate drawn) and inside the frame, a button's text fits its
    /// button, and no two things one can click overlap.
    pub fn tidy(canvas: &Recorder, state: &UiState, need: f32) {
        let scale = scale_for(canvas.size, need, 0);
        let panel = canvas.rects.first().expect("a panel was drawn").0;
        let (w, h) = canvas.size;
        assert!(
            panel.h <= PANEL_HIGH * scale + 0.5,
            "{}: a panel of {} units is taller than the scale rule knows of",
            state.screen,
            panel.h / scale
        );
        assert!(
            panel.x >= 0.0 && panel.y >= 0.0 && panel.x + panel.w <= w && panel.y + panel.h <= h,
            "{}: the panel {panel:?} is not inside a frame of {w}x{h}",
            state.screen
        );
        let inside = |r: &Rect| {
            r.x >= panel.x - 0.5
                && r.y >= panel.y - 0.5
                && r.x + r.w <= panel.x + panel.w + 0.5
                && r.y + r.h <= panel.y + panel.h + 0.5
        };
        for seen in &state.seen {
            assert!(
                inside(&seen.rect),
                "{} at {w}x{h}: {:?} at {:?} is outside the panel {panel:?}",
                state.screen,
                seen.text,
                seen.rect
            );
            if seen.kind == SeenKind::Button {
                let tw = font::text_width(scale, &seen.text);
                assert!(
                    tw <= seen.rect.w - 2.0 * scale,
                    "{} at {w}x{h}: the button {:?} is {} wide and its text {tw}",
                    state.screen,
                    seen.text,
                    seen.rect.w
                );
            }
        }
        let clickable: Vec<&Seen> = state
            .seen
            .iter()
            .filter(|s| s.kind != SeenKind::Label)
            .collect();
        for (i, a) in clickable.iter().enumerate() {
            for b in &clickable[..i] {
                let apart = a.rect.x + a.rect.w <= b.rect.x
                    || b.rect.x + b.rect.w <= a.rect.x
                    || a.rect.y + a.rect.h <= b.rect.y
                    || b.rect.y + b.rect.h <= a.rect.y;
                assert!(
                    apart,
                    "{} at {w}x{h}: {:?} and {:?} overlap",
                    state.screen, a.text, b.text
                );
            }
        }
    }

    impl Canvas for Recorder {
        fn size(&self) -> (f32, f32) {
            self.size
        }
        fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
            self.rects.push((Rect::new(x, y, w, h), color));
        }
        fn text(&mut self, x: f32, y: f32, _scale: f32, _color: [f32; 4], text: &str) {
            self.texts.push((x, y, text.to_string()));
        }
    }

    /// One frame of a small screen: a field, a secret field, a list and two buttons.
    struct Form {
        name: String,
        secret: String,
        picked: usize,
        ok: u32,
        cancel: u32,
        event: ListEvent,
    }

    fn rows() -> Vec<Vec<String>> {
        (0..20)
            .map(|i| vec![format!("row {i}"), format!("{i}")])
            .collect()
    }

    fn frame(form: &mut Form, state: &mut UiState, input: &UiInput) {
        let mut canvas = Recorder::new(1280.0, 720.0);
        let mut ui = Ui::begin(&mut canvas, state, input, "form", 200.0);
        ui.focus_default("field", "name");
        ui.text_field(
            Rect::new(100.0, 100.0, 200.0, 60.0),
            "name",
            &mut form.name,
            Field::text(8),
        );
        ui.text_field(
            Rect::new(100.0, 170.0, 200.0, 60.0),
            "password",
            &mut form.secret,
            Field::secret(64),
        );
        form.event = ui.list(
            Rect::new(100.0, 240.0, 300.0, 5.0 * (ui.line() + 4.0) + 4.0),
            "rows",
            &[0.0, 0.8],
            &rows(),
            &mut form.picked,
        );
        if ui.button(Rect::new(100.0, 500.0, 90.0, 34.0), "OK") {
            form.ok += 1;
        }
        if ui.button(Rect::new(200.0, 500.0, 90.0, 34.0), "Cancel") {
            form.cancel += 1;
        }
        ui.end();
    }

    fn form() -> (Form, UiState) {
        (
            Form {
                name: String::new(),
                secret: String::new(),
                picked: 0,
                ok: 0,
                cancel: 0,
                event: ListEvent::None,
            },
            UiState::default(),
        )
    }

    fn click(at: (f32, f32)) -> [UiInput; 2] {
        [
            UiInput {
                cursor: at,
                pressed: true,
                down: true,
                ..Default::default()
            },
            UiInput {
                cursor: at,
                released: true,
                ..Default::default()
            },
        ]
    }

    fn typed(text: &str) -> UiInput {
        UiInput {
            text: text.into(),
            ..Default::default()
        }
    }

    fn keys(keys: &[Key]) -> UiInput {
        UiInput {
            keys: keys.to_vec(),
            ..Default::default()
        }
    }

    #[test]
    fn a_click_inside_a_button_presses_it_and_one_outside_does_not() {
        let (mut f, mut st) = form();
        for input in click((145.0, 517.0)) {
            frame(&mut f, &mut st, &input);
        }
        assert_eq!((f.ok, f.cancel), (1, 0));
        // Just outside its right edge, in the gap between the two buttons.
        for input in click((195.0, 517.0)) {
            frame(&mut f, &mut st, &input);
        }
        assert_eq!((f.ok, f.cancel), (1, 0));
        // Pressed on one button and released on the other: nobody's click.
        let [press, _] = click((145.0, 517.0));
        frame(&mut f, &mut st, &press);
        let [_, release] = click((245.0, 517.0));
        frame(&mut f, &mut st, &release);
        assert_eq!((f.ok, f.cancel), (1, 0));
        for input in click((245.0, 517.0)) {
            frame(&mut f, &mut st, &input);
        }
        assert_eq!((f.ok, f.cancel), (1, 1));
    }

    #[test]
    fn a_text_field_takes_characters_up_to_its_length_and_edits_in_the_middle() {
        let (mut f, mut st) = form();
        frame(&mut f, &mut st, &UiInput::default());
        assert_eq!(
            st.focus(),
            Some("field:name"),
            "the first field has the keyboard"
        );
        assert!(st.typing());
        frame(&mut f, &mut st, &typed("Aldric the Bold"));
        assert_eq!(f.name, "Aldric t", "eight characters and no more");
        frame(&mut f, &mut st, &keys(&[Key::Backspace, Key::Backspace]));
        assert_eq!(f.name, "Aldric");
        frame(&mut f, &mut st, &keys(&[Key::Home, Key::Right]));
        frame(&mut f, &mut st, &typed("x"));
        assert_eq!(f.name, "Axldric");
        frame(
            &mut f,
            &mut st,
            &keys(&[Key::Delete, Key::Left, Key::Backspace]),
        );
        assert_eq!(
            f.name, "xdric",
            "delete took the l, backspace the A before the caret"
        );
        frame(&mut f, &mut st, &keys(&[Key::End]));
        frame(&mut f, &mut st, &typed("čé\u{7}"));
        assert_eq!(
            f.name, "xdricč",
            "what the font cannot draw cannot be typed"
        );
    }

    #[test]
    fn a_secret_is_shown_as_dots_and_not_told_to_scripts() {
        let (mut f, mut st) = form();
        frame(&mut f, &mut st, &keys(&[Key::Tab]));
        assert_eq!(st.focus(), Some("field:password"));
        let mut canvas = Recorder::new(1280.0, 720.0);
        let input = typed("hunter22");
        let mut ui = Ui::begin(&mut canvas, &mut st, &input, "form", 200.0);
        ui.text_field(
            Rect::new(100.0, 170.0, 200.0, 60.0),
            "password",
            &mut f.secret,
            Field::secret(64),
        );
        ui.end();
        assert_eq!(f.secret, "hunter22");
        assert!(canvas.texts.iter().any(|t| t.2 == "********"));
        assert!(canvas.texts.iter().all(|t| !t.2.contains("hunter")));
        assert!(st.seen.iter().all(|s| !s.text.contains("hunter")));
        assert!(st.shows("password"));
    }

    #[test]
    fn tab_walks_the_focus_in_the_order_drawn_and_enter_presses_the_focused_button() {
        let (mut f, mut st) = form();
        frame(&mut f, &mut st, &UiInput::default());
        let mut order = Vec::new();
        for _ in 0..5 {
            frame(&mut f, &mut st, &keys(&[Key::Tab]));
            order.push(st.focus().unwrap().to_string());
        }
        assert_eq!(
            order,
            [
                "field:password",
                "list:rows",
                "button:OK",
                "button:Cancel",
                "field:name"
            ]
        );
        frame(&mut f, &mut st, &keys(&[Key::BackTab, Key::BackTab]));
        assert_eq!(st.focus(), Some("button:Cancel"), "one step back per frame");
        frame(&mut f, &mut st, &keys(&[Key::BackTab]));
        assert_eq!(st.focus(), Some("button:OK"));
        frame(&mut f, &mut st, &keys(&[Key::Enter]));
        assert_eq!((f.ok, f.cancel), (1, 0));
    }

    #[test]
    fn a_list_selects_scrolls_and_activates() {
        let (mut f, mut st) = form();
        frame(&mut f, &mut st, &keys(&[Key::Tab]));
        frame(&mut f, &mut st, &keys(&[Key::Tab]));
        assert_eq!(st.focus(), Some("list:rows"));
        frame(&mut f, &mut st, &keys(&[Key::Down, Key::Down]));
        assert_eq!((f.picked, f.event), (2, ListEvent::Picked));
        // Past the five rows that fit: the window follows the selection.
        frame(&mut f, &mut st, &keys(&[Key::PageDown]));
        assert_eq!(f.picked, 7);
        assert!(st.shows("row 7") && !st.shows("row 0"));
        frame(&mut f, &mut st, &keys(&[Key::End]));
        assert_eq!(f.picked, 19);
        assert!(st.shows("row 19") && st.shows("row 15") && !st.shows("row 14"));
        frame(&mut f, &mut st, &keys(&[Key::Enter]));
        assert_eq!(f.event, ListEvent::Activated);
        // The wheel moves the window, not the selection.
        let wheel = UiInput {
            cursor: (150.0, 260.0),
            wheel: 3.0,
            ..Default::default()
        };
        frame(&mut f, &mut st, &wheel);
        assert_eq!(f.picked, 19);
        assert!(st.shows("row 12") && !st.shows("row 19"));
        // A click picks the row under it; a double click activates it.
        let row = st.find("row 13").unwrap().rect.centre();
        let [press, release] = click(row);
        frame(&mut f, &mut st, &press);
        assert_eq!((f.picked, f.event), (13, ListEvent::Picked));
        frame(&mut f, &mut st, &release);
        let double = UiInput {
            double: true,
            ..press.clone()
        };
        frame(&mut f, &mut st, &double);
        assert_eq!((f.picked, f.event), (13, ListEvent::Activated));
    }

    #[test]
    fn a_double_click_is_two_presses_on_one_row() {
        let (mut f, mut st) = form();
        frame(&mut f, &mut st, &UiInput::default());
        // The first press was on a button (the one that put the list under the pointer,
        // say), the second lands on a row: the row is picked, and that is all.
        for input in click((145.0, 517.0)) {
            frame(&mut f, &mut st, &input);
        }
        let row = st.find("row 3").unwrap().rect.centre();
        let second = UiInput {
            cursor: row,
            pressed: true,
            down: true,
            double: true,
            ..Default::default()
        };
        frame(&mut f, &mut st, &second);
        assert_eq!((f.picked, f.event), (3, ListEvent::Picked));
        // One press on a row and the next on another: picked, not activated.
        let other = UiInput {
            cursor: st.find("row 1").unwrap().rect.centre(),
            ..second.clone()
        };
        frame(&mut f, &mut st, &other);
        assert_eq!((f.picked, f.event), (1, ListEvent::Picked));
        // Twice on the same: that is a double click.
        frame(&mut f, &mut st, &other);
        assert_eq!((f.picked, f.event), (1, ListEvent::Activated));
    }

    #[test]
    fn a_selection_made_elsewhere_is_scrolled_into_view() {
        let (mut f, mut st) = form();
        frame(&mut f, &mut st, &UiInput::default());
        assert!(st.shows("row 0") && !st.shows("row 17"));
        // The screen picked a row by itself (the character played last, the one just
        // made): the list shows it.
        f.picked = 17;
        frame(&mut f, &mut st, &UiInput::default());
        assert!(st.shows("row 17"), "{:?}", st.seen.last());
        // The wheel may still look elsewhere without the selection pulling it back.
        let wheel = UiInput {
            cursor: (150.0, 260.0),
            wheel: 8.0,
            ..Default::default()
        };
        frame(&mut f, &mut st, &wheel);
        frame(&mut f, &mut st, &UiInput::default());
        assert!(!st.shows("row 17") && st.shows("row 5"));
    }

    #[test]
    fn what_is_clicked_is_found_by_its_name_not_by_a_part_of_it() {
        let mut st = UiState::default();
        let mut picked = 0usize;
        let (mut on, mut size) = (false, 2usize);
        let mut canvas = Recorder::new(1280.0, 720.0);
        let input = UiInput::default();
        let mut ui = Ui::begin(&mut canvas, &mut st, &input, "characters", 340.0);
        let rows = vec![
            vec!["Player One".to_string(), "blade".to_string()],
            vec!["Aldric".to_string(), "frostweaver".to_string()],
        ];
        ui.list(
            Rect::new(10.0, 10.0, 400.0, 80.0),
            "characters",
            &[0.0, 0.5],
            &rows,
            &mut picked,
        );
        ui.button_if(Rect::new(10.0, 100.0, 80.0, 30.0), "Play", false);
        ui.button(Rect::new(100.0, 100.0, 120.0, 30.0), "Log out");
        ui.checkbox(
            Rect::new(10.0, 140.0, 300.0, 30.0),
            "show the password",
            &mut on,
        );
        ui.choice(
            Rect::new(10.0, 180.0, 400.0, 60.0),
            "size of text",
            &["by the window", "1", "2", "3", "4"],
            &mut size,
        );
        ui.end();
        // Play is off: nothing answers to it, least of all the row of Player One.
        assert!(st.find("Play").is_none());
        assert_eq!(st.find("Player One").map(|s| s.kind), Some(SeenKind::Row));
        assert_eq!(
            st.find("Aldric  frostweaver").map(|s| s.kind),
            Some(SeenKind::Row)
        );
        assert!(
            st.find("Aldric  frost").is_none(),
            "a part of a cell is not its name"
        );
        assert_eq!(
            st.find("show the password").map(|s| s.kind),
            Some(SeenKind::Check)
        );
        assert!(st.find("show").is_none());
        assert!(st.find("Log out").is_some() && st.find("Log").is_none());
        // One of a few: each is found by the label and its own word; the lit one says so.
        assert!(st.shows("size of text 2: on") && !st.shows("size of text 3: on"));
        let three = st.find("size of text 3").expect("the option").rect.centre();
        for input in click(three) {
            let mut canvas = Recorder::new(1280.0, 720.0);
            let mut ui = Ui::begin(&mut canvas, &mut st, &input, "characters", 340.0);
            ui.choice(
                Rect::new(10.0, 180.0, 400.0, 60.0),
                "size of text",
                &["by the window", "1", "2", "3", "4"],
                &mut size,
            );
            ui.end();
        }
        assert_eq!(size, 3);
    }

    #[test]
    fn text_that_did_not_fit_is_known_of() {
        let mut st = UiState::default();
        let mut canvas = Recorder::new(1280.0, 720.0);
        let input = UiInput::default();
        let mut ui = Ui::begin(&mut canvas, &mut st, &input, "notes", 340.0);
        let line = ui.line();
        // Room for two lines, words for three; and a label cut at its width.
        ui.paragraph(
            Rect::new(10.0, 10.0, 120.0, 2.0 * line),
            TEXT,
            "one two three four five six seven eight",
        );
        ui.paragraph(Rect::new(10.0, 100.0, 400.0, 2.0 * line), TEXT, "this fits");
        ui.label(10.0, 200.0, 60.0, TEXT, "a label too long for it");
        ui.label(10.0, 230.0, 0.0, TEXT, "a label as long as it likes");
        ui.end();
        assert_eq!(
            st.clipped,
            [
                "one two three four five six seven eight",
                "a label too long for it"
            ]
        );
    }

    #[test]
    fn the_focus_does_not_survive_a_change_of_screen() {
        let (mut f, mut st) = form();
        frame(&mut f, &mut st, &keys(&[Key::Tab]));
        assert!(st.focus().is_some());
        let mut canvas = Recorder::new(1280.0, 720.0);
        let input = UiInput::default();
        let mut ui = Ui::begin(&mut canvas, &mut st, &input, "other", 200.0);
        assert!(!ui.button(Rect::new(0.0, 0.0, 50.0, 20.0), "Back"));
        ui.end();
        assert_eq!(st.focus(), None);
        assert_eq!(st.screen, "other");
        assert!(st.shows("Back") && !st.shows("OK"));
    }

    #[test]
    fn checkboxes_and_sliders_change_by_mouse_and_keys() {
        let mut st = UiState::default();
        let (mut on, mut v) = (false, 1.0f32);
        let run = |st: &mut UiState, input: &UiInput, on: &mut bool, v: &mut f32| {
            let mut canvas = Recorder::new(1280.0, 720.0);
            let mut ui = Ui::begin(&mut canvas, st, input, "settings", 200.0);
            ui.checkbox(Rect::new(10.0, 10.0, 200.0, 30.0), "invert", on);
            ui.slider(Rect::new(10.0, 50.0, 200.0, 50.0), "speed", v, (0.0, 2.0));
            ui.end();
        };
        for input in click((20.0, 25.0)) {
            run(&mut st, &input, &mut on, &mut v);
        }
        assert!(on);
        assert!(st.shows("invert: on"));
        // A press at three quarters of the track.
        let [press, release] = click((160.0, 85.0));
        run(&mut st, &press, &mut on, &mut v);
        assert!((v - 1.5).abs() < 0.01, "{v}");
        run(&mut st, &release, &mut on, &mut v);
        run(&mut st, &keys(&[Key::Left]), &mut on, &mut v);
        assert!((v - 1.4).abs() < 0.01, "{v}");
    }

    #[test]
    fn text_wraps_at_words() {
        assert_eq!(wrap("a wall of iron", 6), ["a wall", "of", "iron"]);
        assert_eq!(wrap("unbreakable", 4), ["unbr", "eaka", "ble"]);
        assert_eq!(wrap("one\ntwo", 10), ["one", "two"]);
        assert_eq!(wrap("", 10), [""]);
    }

    #[test]
    fn the_scale_grows_with_the_frame_and_keeps_a_panel_inside_it() {
        assert_eq!(scale_for((640.0, 360.0), 300.0, 0), 1.0);
        assert_eq!(scale_for((1280.0, 720.0), 300.0, 0), 2.0);
        assert_eq!(scale_for((1920.0, 1080.0), 300.0, 0), 3.0);
        assert_eq!(scale_for((3840.0, 2160.0), 300.0, 0), 4.0);
        // Too low for the tallest panel at the scale its height would give: one less.
        assert_eq!(scale_for((960.0, 540.0), 300.0, 0), 1.0);
        assert_eq!(scale_for((1024.0, 600.0), 300.0, 0), 2.0);
        // A scale somebody chose gives way the same, in both directions.
        assert_eq!(scale_for((1920.0, 1080.0), 340.0, 1), 1.0);
        assert_eq!(scale_for((1920.0, 1080.0), 340.0, 4), 3.0);
        assert_eq!(scale_for((1280.0, 720.0), 340.0, 4), 2.0);
        assert_eq!(scale_for((640.0, 360.0), 340.0, 3), 1.0);
        // A tall narrow window: the panel must still fit.
        assert_eq!(scale_for((700.0, 1200.0), 300.0, 0), 2.0);
        assert_eq!(scale_for((250.0, 1200.0), 300.0, 0), 1.0);
    }
}
