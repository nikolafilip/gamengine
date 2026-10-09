//! The toolkit the screens are made of (CLIENT.md 3, LOOK.md 2). Immediate mode: a screen is
//! a function that runs every frame, lays its widgets out and learns at once what was
//! clicked. Nothing is kept between frames but the focus, the carets, the scroll positions,
//! a drag and what the last frame showed. Everything is drawn with a few primitives (a
//! rectangle, a line of text, a piece of the skin, an icon, a wedge), each with a plain
//! fallback, so the toolkit runs against the HUD with or without a bundle and, in tests,
//! against a recorder.

use std::collections::HashMap;

use crate::font::{self, GLYPH_H};
pub use crate::hud::FaceId;

/// What the toolkit draws with. The pictures are optional: a canvas without them (the
/// test recorder, a client without a bundle) answers `false` and the toolkit draws the
/// plain thing instead (LOOK.md 1.2).
pub trait Canvas {
    /// The frame's size in pixels.
    fn size(&self) -> (f32, f32);
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]);
    /// Text in the small face, its top left at `(x, y)`.
    fn text(&mut self, x: f32, y: f32, scale: f32, color: [f32; 4], text: &str);
    /// Text in `face`, the top of its line at `(x, y)`.
    fn text_in(&mut self, face: FaceId, x: f32, y: f32, scale: f32, color: [f32; 4], text: &str) {
        let _ = face;
        self.text(x, y, scale, color, text);
    }
    fn width_in(&self, face: FaceId, scale: f32, text: &str) -> f32 {
        let _ = face;
        font::text_width(scale, text)
    }
    /// A face's line height and ascent in dots.
    fn metrics(&self, face: FaceId) -> (f32, f32) {
        let _ = face;
        (GLYPH_H + 2.0, GLYPH_H)
    }
    fn has_face(&self, face: FaceId) -> bool {
        face == FaceId::Small
    }
    /// A piece of the skin stretched to the rectangle; `false` when there is none.
    fn image(&mut self, x: f32, y: f32, w: f32, h: f32, piece: &str, color: [f32; 4]) -> bool {
        let _ = (x, y, w, h, piece, color);
        false
    }
    /// A nine-slice of the skin; `false` when there is none.
    #[allow(clippy::too_many_arguments)]
    fn frame(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        piece: &str,
        scale: f32,
        color: [f32; 4],
    ) -> bool {
        let _ = (x, y, w, h, piece, scale, color);
        false
    }
    /// An icon by key, `side` pixels square; `false` when there is none.
    fn icon(&mut self, x: f32, y: f32, side: f32, key: &str, color: [f32; 4]) -> bool {
        let _ = (x, y, side, key, color);
        false
    }
    /// Everything after this goes to layer `n` (LOOK.md 2.1).
    fn layer(&mut self, n: usize) {
        let _ = n;
    }
}

/// The layers (LOOK.md 2.1): plates, bodies drawn into the screen, ink, what is over it
/// all (a tooltip, a dragged icon); the fifth is the cursor's, for when one is drawn.
pub const LAYER_PLATES: usize = 0;
pub const LAYER_INK: usize = 2;
pub const LAYER_OVER: usize = 3;

/// Lines of a tooltip: text and colour.
pub type TipLines = Vec<(String, [f32; 4])>;

pub const PLATE: [f32; 4] = [0.06, 0.06, 0.08, 0.88];
pub const EDGE: [f32; 4] = [0.36, 0.36, 0.46, 1.0];
/// A drag of this many units across the paperdoll turns the body once around.
pub const TURN_DOTS: f32 = 240.0;
pub const WELL: [f32; 4] = [0.02, 0.02, 0.03, 0.90];
pub const BUTTON: [f32; 4] = [0.17, 0.17, 0.24, 1.0];
pub const BUTTON_HOT: [f32; 4] = [0.26, 0.26, 0.37, 1.0];
pub const BUTTON_DOWN: [f32; 4] = [0.34, 0.34, 0.48, 1.0];
pub const PICKED: [f32; 4] = [0.22, 0.26, 0.40, 1.0];
pub const TEXT: [f32; 4] = [0.92, 0.92, 0.88, 1.0];
pub const FAINT: [f32; 4] = [0.60, 0.60, 0.62, 1.0];
pub const OFF: [f32; 4] = [0.40, 0.40, 0.42, 1.0];
/// A row that cannot be taken (`RowMark::Off`): dimmer than a button that is off, so
/// that it is told from a plain row's faint cells at a glance.
pub const OFF_DIM: [f32; 4] = [0.33, 0.33, 0.36, 1.0];
pub const FOCUS: [f32; 4] = [0.90, 0.75, 0.20, 1.0];
pub const WARN: [f32; 4] = [0.95, 0.55, 0.15, 1.0];

/// The tallest panel any screen draws, in units (the trade window with its three grids;
/// the inventory with its grid and equip panel is 335, the new character's 283).
pub const PANEL_HIGH: f32 = 360.0;

/// The one scale HUD and screens are drawn at: whole dots, larger on larger frames, or the
/// one somebody chose (1 to 4; 0: by the frame), and never so large that a panel `need`
/// units wide and the tallest one would not fit. By the frame, a line of text is 2 to 3
/// hundredths of the frame's height: two pixels a dot from 720 lines to 1200 (the tallest
/// panel is then two thirds of a 1080-line frame, not all of it), three to 1800, four on
/// 4K. The bundle has an atlas for each (LOOK.md 2.2).
pub fn scale_for(size: (f32, f32), need: f32, chosen: u8) -> f32 {
    let mut s: f32 = match (chosen, size.1) {
        (0, h) if h < 600.0 => 1.0,
        (0, h) if h < 1300.0 => 2.0,
        (0, h) if h < 1900.0 => 3.0,
        (0, _) => 4.0,
        (n, _) => n.min(4) as f32,
    };
    while s > 1.0 && (need * s > size.0 || PANEL_HIGH * s > size.1) {
        s -= 1.0;
    }
    s
}

/// The scale a touch screen asks for when none is chosen (MODES.md 5.6): a dot per
/// device pixel of the browser's ratio (a phone's 2.6 to 3.5 is 3; a tablet's 2 is 2),
/// at least 2, because a finger is wider than a mouse. `scale_for` still shrinks it to
/// what the panels can fit.
pub fn touch_scale(device_pixel_ratio: f32) -> u8 {
    (device_pixel_ratio.round() as u8).clamp(2, 4)
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
    /// The game's own two keys that open a screen (ITEMS.md 6): `I`, the inventory, and
    /// `E`, the stall the body stands at. They are keys only in the game: while a screen
    /// has the keyboard the same keys are letters.
    Inventory,
    Use,
    /// And `P`: the people here and of the party (PARTY.md 8).
    People,
    /// And `K`: the character's page (MATRIX.md 9.1): the points and the kit.
    Character,
    /// And `G`: the game master's page (GM.md 4), for whom the zone granted it.
    Gm,
    /// And `F`: the kit (MODES.md 11.3), the one key of the game itself a script presses.
    Kit,
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
            "I" => Key::Inventory,
            "E" => Key::Use,
            "P" => Key::People,
            "K" => Key::Character,
            "G" => Key::Gm,
            "F" => Key::Kit,
            _ => return None,
        })
    }
}

/// What happened since the last frame, as a screen needs it.
#[derive(Clone, Debug, Default)]
pub struct UiInput {
    pub cursor: (f32, f32),
    /// Where the pointer was last frame (a drag across the paperdoll turns it).
    pub last_cursor: (f32, f32),
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
    /// A picture: its text is the icon's or the piece's key.
    Image,
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
    /// Cells of lists the last frame cut to their column. Some lists may (a long name in
    /// the characters' list); a screen whose cells are prices and numbers asks that there
    /// are none.
    pub cut_cells: Vec<String>,
    /// Buttons pressed since this was last taken (SOUND.md 3: a click is heard).
    pub presses: u32,
    /// A drag in progress (LOOK.md 2.4): the slot it started on (grid name, thing id,
    /// icon key, the thing's name), and where the press was.
    drag: Option<Drag>,
    /// The drag that was let go this frame: the widgets look at it to see whether it
    /// landed on them (`Ui::dropped`).
    drag_ended: Option<Drag>,
    /// What is hovered and since when (seconds): the tooltip's clock.
    hover: Option<(String, f32)>,
    /// A button that is off was pressed (a tap on a phone), and when: it says why it is
    /// off for a moment after (`button_or`).
    explained: Option<(String, f32)>,
    /// Bodies the screen wants drawn into it this frame (LOOK.md 5), for the app.
    pub paperdolls: Vec<Paperdoll>,
    /// The paperdoll's turn, by a drag across it, in turns.
    pub paperdoll_turn: f32,
}

/// A thing being dragged from a slot.
#[derive(Clone, Debug, PartialEq)]
pub struct Drag {
    pub grid: String,
    pub id: i64,
    pub icon: Option<String>,
    pub name: String,
    pub from: (f32, f32),
    /// The press has moved far enough to be a drag (four dots).
    pub live: bool,
}

/// A body drawn into a rectangle of the screen (LOOK.md 5).
#[derive(Clone, Debug, PartialEq)]
pub struct Paperdoll {
    pub rect: Rect,
    /// Turned this many turns from facing the viewer.
    pub turn: f32,
    /// Drawn in the open, over whatever is behind the screen (the selector of CLIENT.md
    /// 4.2): no well under it.
    pub open: bool,
}

impl UiState {
    /// Whether a text field has the keyboard.
    #[cfg(test)]
    pub fn typing(&self) -> bool {
        self.typing_in().is_some()
    }

    /// The label of the text field that has the keyboard, if one has it (a page tells its
    /// browser, which has the phone's keyboard, WEB.md 3.6).
    #[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
    pub fn typing_in(&self) -> Option<&str> {
        self.focus.as_deref().and_then(|f| f.strip_prefix("field:"))
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

    /// A picture by its key was drawn last frame (an icon in a slot, a portrait).
    pub fn shows_image(&self, key: &str) -> bool {
        self.seen
            .iter()
            .any(|s| s.kind == SeenKind::Image && s.text == key)
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
    /// Only the ten digits are taken (an amount).
    pub digits: bool,
}

impl Field {
    pub const fn text(max_chars: usize) -> Field {
        Field {
            secret: false,
            max_chars,
            max_bytes: usize::MAX,
            no_spaces: false,
            any: false,
            digits: false,
        }
    }

    pub const fn number(max_digits: usize) -> Field {
        Field {
            digits: true,
            ..Field::text(max_digits)
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

/// A list's selection when no row is picked: none is lit, the arrow keys start from an
/// end, and nothing can be activated. (What was picked is gone: nothing takes its place by
/// standing in its row.)
pub const NONE: usize = usize::MAX;

/// How a row of a list is drawn (`Ui::list_marked`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowMark {
    Plain,
    /// It calls for a second look.
    Marked,
    /// It is gone: shown for what it was.
    Struck,
    /// It is in (one of several that may be): lit like the row picked.
    Picked,
    /// It cannot be taken as things stand (CLIENT.md 4.6): drawn faint, a click on it
    /// picks nothing; the cursor still finds it, so a line can say why.
    Off,
}

/// A thing in a slot of a grid (LOOK.md 2.4).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlotThing {
    /// What it is picked by (an item's id, a listing's id).
    pub id: i64,
    pub name: String,
    /// What a script finds the cell by (ITEMS.md 6: the row's words, `sword  slash
    /// +2.0%  worn`); the name alone when empty.
    pub said: String,
    /// Its icon's key in the atlas.
    pub icon: Option<String>,
    pub worn: bool,
    /// Drawn dim: it cannot be acted on now.
    pub off: bool,
    /// It cannot be dragged (a listing at another's stall is bought, not moved).
    pub fixed: bool,
    pub mark: Option<SlotMark>,
    pub count: Option<u32>,
    /// A line under the cell (a price), and its colour.
    pub under: String,
    pub under_colour: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotMark {
    Worn,
    New,
    Taken,
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
    /// The face the screen's words are in (the text face when the bundle's atlas has it,
    /// else the small one) and the face of titles.
    pub face: FaceId,
    pub title_face: FaceId,
    /// The face's line height and ascent in dots.
    line_dots: f32,
    ascent_dots: f32,
    order: Vec<String>,
    seen: Vec<Seen>,
    clipped: Vec<String>,
    cut: Vec<String>,
    /// A row of a list was pressed this frame.
    row_pressed: bool,
    /// The row of a list the cursor is over this frame: the list's name and the row.
    row_over: Option<(String, usize)>,
    /// The tooltip to draw at the end of the frame (over everything), if a slot was
    /// hovered long enough.
    tooltip: Option<(Rect, TipLines)>,
    /// What a drag that ended this frame was dropped on (`Ui::drop_target`).
    dropped: Option<(String, String)>,
    /// A slot or grid was hovered this frame (the tooltip's clock runs while one is).
    hovered: Option<String>,
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
        let face = if canvas.has_face(FaceId::Text) {
            FaceId::Text
        } else {
            FaceId::Small
        };
        let title_face = if canvas.has_face(FaceId::Title) {
            FaceId::Title
        } else {
            face
        };
        let (line_dots, ascent_dots) = canvas.metrics(face);
        // A drag that was let go lands this frame: the widgets say where (`dropped`).
        if input.released && state.drag.is_some() && !input.down {
            state.drag_ended = state.drag.take();
        }
        Ui {
            canvas,
            used: vec![false; input.keys.len()],
            state,
            input,
            text_used: false,
            scale,
            face,
            title_face,
            line_dots,
            ascent_dots,
            order: Vec::new(),
            seen: Vec::new(),
            clipped: Vec::new(),
            cut: Vec::new(),
            row_pressed: false,
            row_over: None,
            tooltip: None,
            dropped: None,
            hovered: None,
        }
    }

    pub fn size(&self) -> (f32, f32) {
        self.canvas.size()
    }

    /// The height of a line of text, a button and a field at this scale.
    pub fn line(&self) -> f32 {
        (self.line_dots + 3.0) * self.scale
    }

    /// The height of the face's capitals at this scale: what a line of text occupies
    /// above its baseline (the small face's 7; the text face's 9).
    pub fn ascent(&self) -> f32 {
        self.ascent_dots * self.scale
    }

    pub fn button_height(&self) -> f32 {
        (self.line_dots + 8.0) * self.scale
    }

    /// The height of a list that shows `rows` rows.
    pub fn list_height(&self, rows: usize) -> f32 {
        rows as f32 * (self.line() + 2.0 * self.scale) + 2.0 * self.scale
    }

    /// The height of a panel whose inside is `inner` high (see `panel`).
    pub fn panel_height(&self, inner: f32, titled: bool) -> f32 {
        let pad = 8.0 * self.scale;
        // What `panel` takes over the room inside: its title in the title face, which
        // is taller than a line of text where the bundle has one.
        let top = if titled {
            pad * 0.75 + self.title_line() + pad * 0.75
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
        self.canvas.width_in(self.face, self.scale, text)
    }

    /// Characters that fit in `w` pixels: a count by the face's average advance, for
    /// wrapping and for what a cell has room for; `fit_text` is exact for one text.
    pub fn fit(&self, w: f32) -> usize {
        let avg = self.canvas.width_in(self.face, self.scale, "nenonenonen") / 11.0;
        ((w + self.scale) / avg.max(1.0)).floor().max(0.0) as usize
    }

    /// The longest prefix of `text` that fits in `w` pixels.
    pub fn fit_text(&self, w: f32, text: &str) -> String {
        let mut out = String::new();
        for c in text.chars() {
            out.push(c);
            if self.text_width(&out) > w {
                out.pop();
                break;
            }
        }
        out
    }

    /// Text in the screen's face with the top of its line at `(x, y)`.
    pub fn ink(&mut self, x: f32, y: f32, color: [f32; 4], text: &str) {
        self.canvas
            .text_in(self.face, x, y, self.scale, color, text);
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
        self.canvas.layer(LAYER_PLATES);
        if !self.canvas.frame(r.x, r.y, r.w, r.h, "panel", s, [1.0; 4]) {
            self.canvas.rect(r.x, r.y, r.w, r.h, PLATE);
            self.outline(r, EDGE);
        }
        self.canvas.layer(LAYER_INK);
        let pad = 8.0 * s;
        if title.is_empty() {
            return r.inset(pad);
        }
        // A title longer than the panel is cut to it, in the title face.
        let (title_line, _) = self.canvas.metrics(self.title_face);
        let title_h = title_line * s;
        let shown = {
            let mut out = String::new();
            for c in title.chars() {
                out.push(c);
                if self.canvas.width_in(self.title_face, s, &out) > r.w - 2.0 * pad {
                    out.pop();
                    break;
                }
            }
            out
        };
        let tw = self.canvas.width_in(self.title_face, s, &shown);
        self.canvas.layer(LAYER_PLATES);
        let bar = Rect::new(r.x + 3.0 * s, r.y + 3.0 * s, r.w - 6.0 * s, title_h + pad);
        let framed = self
            .canvas
            .frame(bar.x, bar.y, bar.w, bar.h, "panel_title", s, [1.0; 4]);
        self.canvas.layer(LAYER_INK);
        self.canvas.text_in(
            self.title_face,
            r.x + pad,
            r.y + pad * 0.75,
            s,
            FOCUS,
            &shown,
        );
        self.note(
            SeenKind::Label,
            title,
            Rect::new(r.x + pad, r.y + pad * 0.75, tw, title_h),
        );
        let top = pad * 0.75 + title_h + pad * 0.75;
        if !framed {
            self.canvas
                .rect(r.x + pad, r.y + top - 3.0 * s, r.w - 2.0 * pad, s, EDGE);
        }
        Rect::new(
            r.x + pad,
            r.y + top,
            (r.w - 2.0 * pad).max(0.0),
            (r.h - top - pad).max(0.0),
        )
    }

    /// A gold edge round `r`, over what was drawn there: the tab that is shown, the
    /// preset a draft is (CLIENT.md 4.3). Drawn after the widget it marks.
    pub fn mark(&mut self, r: Rect) {
        self.canvas.layer(LAYER_INK);
        self.outline(r, FOCUS);
        let t = self.scale;
        self.outline(r.inset(t), FOCUS);
    }

    /// The height of a line in the title face at this scale.
    pub fn title_line(&self) -> f32 {
        let (line, _) = self.canvas.metrics(self.title_face);
        line * self.scale
    }

    /// A line in the title face, in the middle of `r`'s width (a name over a body,
    /// CLIENT.md 4.2); cut to the width. Returns the line's height.
    pub fn heading(&mut self, r: Rect, color: [f32; 4], text: &str) -> f32 {
        let s = self.scale;
        let mut shown = String::new();
        for c in text.chars() {
            shown.push(c);
            if self.canvas.width_in(self.title_face, s, &shown) > r.w {
                shown.pop();
                break;
            }
        }
        if shown.len() < text.len() {
            self.clipped.push(text.to_string());
        }
        let tw = self.canvas.width_in(self.title_face, s, &shown);
        let x = (r.x + (r.w - tw) * 0.5).round();
        self.canvas.layer(LAYER_INK);
        self.canvas
            .text_in(self.title_face, x, r.y, s, color, &shown);
        let h = self.title_line();
        self.note(SeenKind::Label, text, Rect::new(x, r.y, tw, h));
        h
    }

    /// A line of text, cut to what fits in `w` pixels when `w` is positive.
    pub fn label(&mut self, x: f32, y: f32, w: f32, color: [f32; 4], text: &str) {
        let shown: String = if w > 0.0 {
            self.fit_text(w, text)
        } else {
            text.to_string()
        };
        if shown.len() < text.len() {
            self.clipped.push(text.to_string());
        }
        self.canvas
            .text_in(self.face, x, y, self.scale, color, &shown);
        let rect = Rect::new(x, y, self.text_width(&shown), self.ascent());
        self.note(SeenKind::Label, text, rect);
    }

    /// One line in several colours, its parts a space apart (an amount of coin: a colour
    /// for each unit). A script sees it as one text. Returns its width.
    pub fn spans(&mut self, x: f32, y: f32, parts: &[(String, [f32; 4])]) -> f32 {
        let mut whole = String::new();
        let mut at = x;
        let space = self.text_width(" ");
        for (text, color) in parts {
            if !whole.is_empty() {
                whole.push(' ');
                at += space;
            }
            self.canvas
                .text_in(self.face, at, y, self.scale, *color, text);
            at += self.text_width(text);
            whole.push_str(text);
        }
        let w = self.text_width(&whole);
        self.note(SeenKind::Label, &whole, Rect::new(x, y, w, self.ascent()));
        w
    }

    /// Coin with its two pieces before the numbers (LOOK.md 7): a gold coin and a silver
    /// one from the skin, or the coloured words alone. Returns its width.
    pub fn coins(&mut self, x: f32, y: f32, parts: &[(String, [f32; 4])]) -> f32 {
        let s = self.scale;
        let side = self.ascent();
        let mut whole = String::new();
        let mut at = x;
        let space = self.text_width(" ");
        for (text, color) in parts {
            if !whole.is_empty() {
                whole.push(' ');
                at += space;
            }
            let piece = if text.ends_with(" g") {
                "coin_gold"
            } else {
                "coin_silver"
            };
            if self.canvas.image(at, y, side, side, piece, [1.0; 4]) {
                at += side + 2.0 * s;
            }
            self.canvas.text_in(self.face, at, y, s, *color, text);
            at += self.text_width(text);
            whole.push_str(text);
        }
        self.note(
            SeenKind::Label,
            &whole,
            Rect::new(x, y, at - x, self.ascent()),
        );
        at - x
    }

    /// Small print, ending at `right`: a word in a corner, in the screen's face at half
    /// the scale and never under one.
    pub fn small(&mut self, right: f32, y: f32, color: [f32; 4], text: &str) {
        let s = (self.scale * 0.5).max(1.0);
        let tw = self.canvas.width_in(self.face, s, text);
        self.canvas
            .text_in(self.face, right - tw, y, s, color, text);
        self.note(
            SeenKind::Label,
            text,
            Rect::new(right - tw, y, tw, self.ascent_dots * s),
        );
    }

    /// Text wrapped at words into `r`; returns the height used.
    pub fn paragraph(&mut self, r: Rect, color: [f32; 4], text: &str) -> f32 {
        let lines = wrap_measured(text, r.w, |t| self.text_width(t));
        let line = self.line();
        for (i, l) in lines.iter().enumerate() {
            let y = r.y + line * i as f32;
            if y + self.ascent() > r.y + r.h && r.h > 0.0 {
                self.clipped.push(text.to_string());
                break;
            }
            self.canvas.text_in(self.face, r.x, y, self.scale, color, l);
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
        self.button_or(r, text, if enabled { None } else { Some("") })
    }

    /// A button that is off for a reason (`Some(why)`), else on: off, it says why beside
    /// the pointer while the pointer rests on it (after the tooltip's 150 ms) and for two
    /// seconds after a press on it (a tap on a phone), so that a click on a grey button
    /// never looks like a click that did nothing. An empty reason says nothing.
    pub fn button_or(&mut self, r: Rect, text: &str, off: Option<&str>) -> bool {
        let s = self.scale;
        let id = format!("button:{text}");
        let enabled = off.is_none();
        if let Some(why) = off.filter(|why| !why.is_empty()) {
            let hot = r.contains(self.input.cursor);
            let time = self.input.time;
            if hot && self.input.pressed {
                self.state.explained = Some((id.clone(), time));
            }
            let tapped =
                matches!(&self.state.explained, Some((k, at)) if *k == id && time - at < 2.0);
            let mut rested = false;
            if hot {
                let since = match &self.state.hover {
                    Some((k, t)) if *k == id => *t,
                    _ => time,
                };
                self.hovered = Some(id.clone());
                self.state.hover = Some((id.clone(), since));
                rested = time - since >= 0.15;
            }
            if (rested || tapped) && self.tooltip.is_none() {
                self.tooltip = Some((
                    Rect::new(self.input.cursor.0, self.input.cursor.1, 0.0, 0.0),
                    vec![(why.to_string(), TEXT)],
                ));
            }
        }
        let (mut pressed, mut fill, mut ink) = (false, BUTTON, TEXT);
        let mut piece = "button_off";
        let mut focused = false;
        if enabled {
            focused = self.focusable(&id);
            let (hot, down, click) = self.clicked(&id, r);
            pressed = click || (focused && self.key(Key::Enter));
            if pressed {
                self.state.presses += 1;
            }
            (fill, piece) = match (down, hot) {
                (true, _) => (BUTTON_DOWN, "button_down"),
                (false, true) => (BUTTON_HOT, "button_hot"),
                _ => (BUTTON, "button"),
            };
        } else {
            fill[3] = 0.6;
            ink = OFF;
        }
        self.canvas.layer(LAYER_PLATES);
        if !self.canvas.frame(r.x, r.y, r.w, r.h, piece, s, [1.0; 4]) {
            self.canvas.rect(r.x, r.y, r.w, r.h, fill);
            self.outline(
                r,
                if !enabled {
                    OFF
                } else if focused {
                    FOCUS
                } else {
                    EDGE
                },
            );
        } else if focused {
            self.outline(r, FOCUS);
        }
        self.canvas.layer(LAYER_INK);
        let tw = self.text_width(text);
        self.canvas.text_in(
            self.face,
            (r.x + (r.w - tw) * 0.5).round(),
            (r.y + (r.h - self.ascent()) * 0.5).round(),
            s,
            ink,
            text,
        );
        if enabled {
            self.note(SeenKind::Button, text, r);
        } else if let Some(why) = off.filter(|why| !why.is_empty()) {
            // Off, and the reason it would give: a label, so that no script finds it.
            self.note(SeenKind::Label, &format!("{text} (off: {why})"), r);
        }
        pressed
    }

    /// Put a field's caret at the end of its text (a screen that wrote the text itself).
    pub fn caret_to_end(&mut self, label: &str) {
        self.state
            .carets
            .insert(format!("field:{label}"), usize::MAX);
    }

    /// One line of text to edit, with its label above it. Returns whether it changed.
    pub fn text_field(&mut self, r: Rect, label: &str, value: &mut String, how: Field) -> bool {
        let s = self.scale;
        let id = format!("field:{label}");
        let focused = self.focusable(&id);
        let label_h = self.line();
        self.canvas.layer(LAYER_INK);
        self.canvas.text_in(self.face, r.x, r.y, s, FAINT, label);
        let well = Rect::new(r.x, r.y + label_h, r.w, (r.h - label_h).max(0.0));
        let (_, _, _) = self.clicked(&id, well);
        let focused = focused || self.state.focus.as_deref() == Some(id.as_str());
        let mut chars: Vec<char> = value.chars().collect();
        let mut caret = (*self.state.carets.get(&id).unwrap_or(&chars.len())).min(chars.len());
        let mut changed = false;
        let room = self.fit(well.w - 8.0 * s);
        if focused {
            // A click puts the caret where it landed: at the character whose start is
            // nearest, measured in the face.
            if self.input.pressed && well.contains(self.input.cursor) {
                let first = first_shown(chars.len(), caret, room);
                let shown: Vec<char> = chars.iter().skip(first).take(room).copied().collect();
                let x = self.input.cursor.0 - well.x - 4.0 * s;
                let mut best = (f32::MAX, 0usize);
                for i in 0..=shown.len() {
                    let prefix: String = shown[..i].iter().collect();
                    let d = (self.text_width(&prefix) - x).abs();
                    if d < best.0 {
                        best = (d, i);
                    }
                }
                caret = (first + best.1).min(chars.len());
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
                // An amount that arrives all at once (a paste) with anything but digits in
                // it is not an amount this field can read: `12.50` is not 1250.
                let pasted = self.input.text.chars().count() > 1;
                let amount = self.input.text.chars().all(|c| c.is_ascii_digit());
                let text = if how.digits && pasted && !amount {
                    ""
                } else {
                    self.input.text.as_str()
                };
                for c in text.chars() {
                    let fits = chars.len() < how.max_chars && bytes + c.len_utf8() <= how.max_bytes;
                    // (Full: the rest is dropped whole, not sieved for what still fits.)
                    if !fits {
                        break;
                    }
                    let known = how.any || font::has_glyph(c);
                    let wanted =
                        !(how.no_spaces && c == ' ') && (!how.digits || c.is_ascii_digit());
                    if known && wanted {
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

        self.canvas.layer(LAYER_PLATES);
        let piece = if focused { "field_focus" } else { "field" };
        if !self
            .canvas
            .frame(well.x, well.y, well.w, well.h, piece, s, [1.0; 4])
        {
            self.canvas.rect(well.x, well.y, well.w, well.h, WELL);
            self.outline(well, if focused { FOCUS } else { EDGE });
        }
        self.canvas.layer(LAYER_INK);
        let first = first_shown(chars.len(), caret, room);
        let shown: String = chars
            .iter()
            .skip(first)
            .take(room)
            .map(|c| if how.secret { '*' } else { *c })
            .collect();
        let ty = (well.y + (well.h - self.ascent()) * 0.5).round();
        self.canvas
            .text_in(self.face, well.x + 4.0 * s, ty, s, TEXT, &shown);
        if focused && ((self.input.time * 2.0) as u32).is_multiple_of(2) {
            let before: String = shown.chars().take(caret - first).collect();
            let cx = well.x + 4.0 * s + self.text_width(&before) - s;
            self.canvas
                .rect(cx, ty - s, s, self.ascent() + 2.0 * s, FOCUS);
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
        self.list_marked(r, name, columns, rows, &[], selected)
    }

    /// A list some of whose rows are marked (`marks` by row; rows past its end are
    /// plain): one that calls for a second look is drawn in the colour of a warning, one
    /// that is gone is drawn faint with a line through it. (PARTY.md 6: what was not
    /// agreed to, and what was taken back.)
    pub fn list_marked(
        &mut self,
        r: Rect,
        name: &str,
        columns: &[f32],
        rows: &[Vec<String>],
        marks: &[RowMark],
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
            *selected = if *selected == NONE { NONE } else { 0 };
        } else if *selected != NONE {
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
                // With nothing picked the keys start from an end.
                let nothing = *selected == NONE;
                let done = match self.input.keys[i] {
                    Key::Up | Key::PageUp | Key::End if nothing => {
                        *selected = last;
                        true
                    }
                    Key::Down | Key::PageDown | Key::Home if nothing => {
                        *selected = 0;
                        true
                    }
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
                        if !nothing {
                            event = ListEvent::Activated;
                        }
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
        let moved = *selected != before || self.state.selections.get(&id) != Some(&*selected);
        if moved && *selected != NONE {
            if *selected < first {
                first = *selected;
            } else if *selected >= first + room {
                first = *selected + 1 - room;
            }
        }
        first = first.min(rows.len().saturating_sub(room));

        self.canvas.layer(LAYER_PLATES);
        if !self.canvas.frame(r.x, r.y, r.w, r.h, "well", s, [1.0; 4]) {
            self.canvas.rect(r.x, r.y, r.w, r.h, WELL);
            self.outline(r, if focused { FOCUS } else { EDGE });
        } else if focused {
            self.outline(r, FOCUS);
        }
        self.canvas.layer(LAYER_INK);
        for (i, row) in rows.iter().enumerate().skip(first).take(room) {
            let rr = Rect::new(
                r.x + s,
                r.y + s + row_h * (i - first) as f32,
                r.w - 2.0 * s,
                row_h,
            );
            let over = rr.contains(self.input.cursor);
            if over {
                self.row_over = Some((name.to_string(), i));
            }
            let off = marks.get(i) == Some(&RowMark::Off);
            if self.input.pressed && over && !off {
                // A double click activates the row both of its presses were on.
                let here = Some((id.clone(), i));
                if self.input.double && self.state.last_row == here {
                    event = ListEvent::Activated;
                }
                *selected = i;
                self.state.last_row = here;
                self.row_pressed = true;
            }
            if i == *selected || marks.get(i) == Some(&RowMark::Picked) {
                self.canvas.rect(rr.x, rr.y, rr.w, rr.h, PICKED);
            } else if over && !off {
                self.canvas.rect(rr.x, rr.y, rr.w, rr.h, BUTTON);
            }
            for (c, cell) in row.iter().enumerate() {
                let from = columns.get(c).copied().unwrap_or(0.0);
                let to = columns.get(c + 1).copied().unwrap_or(1.0);
                let x = rr.x + 4.0 * s + (rr.w - 8.0 * s) * from;
                let shown = self.fit_text((rr.w - 8.0 * s) * (to - from) - 2.0 * s, cell);
                if shown.len() < cell.len() {
                    self.cut.push(cell.clone());
                }
                let mark = marks.get(i).copied().unwrap_or(RowMark::Plain);
                let ink = match mark {
                    RowMark::Marked => WARN,
                    RowMark::Struck => OFF,
                    RowMark::Off => OFF_DIM,
                    RowMark::Plain | RowMark::Picked if c == 0 => TEXT,
                    RowMark::Plain | RowMark::Picked => FAINT,
                };
                self.canvas
                    .text_in(self.face, x, rr.y + 2.0 * s, s, ink, &shown);
            }
            if marks.get(i) == Some(&RowMark::Struck) {
                let y = rr.y + rr.h * 0.5;
                self.canvas.rect(rr.x + 3.0 * s, y, rr.w - 6.0 * s, s, OFF);
            }
            self.note(SeenKind::Row, &row.join("  "), rr);
        }
        // A bar on the right says where in the list the window is, and moves it
        // (LOOK.md 2.4: a scrollbar, for a machine without a wheel).
        if rows.len() > room {
            first = self.scrollbar(&id, r, first, rows.len(), room);
        }
        self.state.scrolls.insert(id.clone(), first);
        self.state.selections.insert(id, *selected);
        if event == ListEvent::None && *selected != before {
            event = ListEvent::Picked;
        }
        event
    }

    /// The row of the list called `name` the cursor is over, once that list was drawn
    /// this frame (what a line under the lists describes).
    pub fn row_over(&self, name: &str) -> Option<usize> {
        self.row_over
            .as_ref()
            .filter(|(n, _)| n == name)
            .map(|(_, i)| *i)
    }

    /// One of a few, in a row under its label: the one picked is lit. Returns whether
    /// another was picked. (A slider would do for a number; for a choice it jumps about
    /// under the hand when what it sets moves the slider itself.)
    pub fn choice(&mut self, r: Rect, label: &str, options: &[&str], picked: &mut usize) -> bool {
        let s = self.scale;
        let label_h = self.line();
        self.canvas.layer(LAYER_INK);
        self.canvas.text_in(self.face, r.x, r.y, s, FAINT, label);
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
            let piece = match (i == *picked, down, hot) {
                (true, _, _) => "button_down",
                (false, true, _) => "button_down",
                (false, false, true) => "button_hot",
                _ => "button",
            };
            self.canvas.layer(LAYER_PLATES);
            if !self
                .canvas
                .frame(at.x, at.y, at.w, at.h, piece, s, [1.0; 4])
            {
                self.canvas.rect(at.x, at.y, at.w, at.h, fill);
                self.outline(at, if focused { FOCUS } else { EDGE });
            } else if focused || i == *picked {
                self.outline(at, FOCUS);
            }
            self.canvas.layer(LAYER_INK);
            let tw = self.text_width(option);
            self.canvas.text_in(
                self.face,
                (at.x + (at.w - tw) * 0.5).round(),
                (at.y + (at.h - self.ascent()) * 0.5).round(),
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
        let side = self.ascent() + 4.0 * s;
        let b = Rect::new(r.x, (r.y + (r.h - side) * 0.5).round(), side, side);
        self.canvas.layer(LAYER_PLATES);
        let piece = if *value { "check_on" } else { "check_off" };
        if !self.canvas.image(b.x, b.y, b.w, b.h, piece, [1.0; 4]) {
            self.canvas
                .rect(b.x, b.y, b.w, b.h, if hot { BUTTON_HOT } else { WELL });
            self.outline(b, if focused { FOCUS } else { EDGE });
            if *value {
                let m = b.inset(3.0 * s);
                self.canvas.rect(m.x, m.y, m.w, m.h, FOCUS);
            }
        } else if focused {
            self.outline(b, FOCUS);
        }
        self.canvas.layer(LAYER_INK);
        self.canvas.text_in(
            self.face,
            b.x + side + 6.0 * s,
            (r.y + (r.h - self.ascent()) * 0.5).round(),
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
        self.canvas.layer(LAYER_INK);
        self.canvas.text_in(
            self.face,
            r.x,
            r.y,
            s,
            FAINT,
            &format!("{label}  {value:.2}"),
        );
        let mid = track.y + track.h * 0.5;
        self.canvas.layer(LAYER_PLATES);
        if !self.canvas.frame(
            track.x,
            mid - 4.0 * s,
            track.w,
            8.0 * s,
            "slider_rail",
            s,
            [1.0; 4],
        ) {
            self.canvas.rect(
                track.x,
                mid - s,
                track.w,
                2.0 * s,
                if focused { FOCUS } else { EDGE },
            );
        }
        let knob = 6.0 * s;
        let kx = (track.x + (track.w - knob) * t).round();
        let (ky, kh) = (track.y + 2.0 * s, (track.h - 4.0 * s).max(s));
        if !self
            .canvas
            .frame(kx - s, ky, knob + 2.0 * s, kh, "slider_knob", s, [1.0; 4])
        {
            self.canvas.rect(kx, ky, knob, kh, TEXT);
        }
        if focused {
            self.outline(Rect::new(kx - s, ky, knob + 2.0 * s, kh), FOCUS);
        }
        self.canvas.layer(LAYER_INK);
        self.note(SeenKind::Slider, &format!("{label}: {value:.2}"), track);
        *value != before
    }

    /// A scrollbar at the right edge of `r` for a window of `room` of `len` rows starting
    /// at `first`; returns the new `first` (a drag on the knob, a click on the rail).
    fn scrollbar(&mut self, id: &str, r: Rect, first: usize, len: usize, room: usize) -> usize {
        let s = self.scale;
        let track = Rect::new(r.x + r.w - 7.0 * s, r.y + 2.0 * s, 6.0 * s, r.h - 4.0 * s);
        let past = len.saturating_sub(room).max(1);
        let bar = (track.h * room as f32 / len as f32).max(8.0 * s);
        let mut first = first.min(past);
        let bar_id = format!("scroll:{id}");
        let (_, held, _) = self.clicked(&bar_id, track);
        if held && self.input.down {
            let t = ((self.input.cursor.1 - track.y - bar * 0.5) / (track.h - bar).max(1.0))
                .clamp(0.0, 1.0);
            first = (t * past as f32).round() as usize;
        }
        let at = (track.h - bar) * first as f32 / past as f32;
        self.canvas.layer(LAYER_INK);
        if !self.canvas.frame(
            track.x,
            track.y,
            track.w,
            track.h,
            "scroll_rail",
            s,
            [1.0; 4],
        ) {
            self.canvas
                .rect(track.x + 2.0 * s, track.y, 2.0 * s, track.h, WELL);
        }
        if !self.canvas.frame(
            track.x,
            track.y + at,
            track.w,
            bar,
            "scroll_knob",
            s,
            [1.0; 4],
        ) {
            self.canvas
                .rect(track.x + 2.0 * s, track.y + at, 2.0 * s, bar, EDGE);
        }
        first
    }

    /// The side of a slot at this scale (LOOK.md 2.4: 36 dots, an icon of 32 with a rim).
    pub fn slot_side(&self) -> f32 {
        36.0 * self.scale
    }

    /// A grid of `cols` slots across `r`: each thing by its id, with its icon key (an
    /// icon the atlas lacks is drawn as the thing's initial), its name, its corner mark
    /// (`worn`, `new`, `taken`) and a line under the cell (a price). Hovering shows the
    /// tooltip the caller gives for the hovered thing; a press picks; a press that moves
    /// starts a drag. `selected` is the id picked (`None` for nothing; a thing that is
    /// gone unpicks itself, ITEMS.md 6). Returns the event, as a list does.
    #[allow(clippy::too_many_arguments)]
    pub fn grid(
        &mut self,
        r: Rect,
        name: &str,
        cols: usize,
        capacity: usize,
        things: &[SlotThing],
        selected: &mut Option<i64>,
        under: f32,
        tooltip: &dyn Fn(&SlotThing) -> TipLines,
    ) -> ListEvent {
        let s = self.scale;
        let id = format!("grid:{name}");
        let side = self.slot_side();
        let gap = 2.0 * s;
        let cols = cols.max(1);
        let rows_shown = ((r.h + gap) / (side + under + gap)).floor().max(1.0) as usize;
        let rows_all = things.len().max(capacity).div_ceil(cols).max(1);
        let mut first_row = *self.state.scrolls.get(&id).unwrap_or(&0);
        let hot_grid = r.contains(self.input.cursor);
        if hot_grid && self.input.wheel != 0.0 {
            let turn = -self.input.wheel.round() as i64;
            first_row = (first_row as i64 + turn)
                .clamp(0, rows_all.saturating_sub(rows_shown) as i64)
                as usize;
        }
        first_row = first_row.min(rows_all.saturating_sub(rows_shown));
        // The thing picked must still be there.
        if let Some(sel) = *selected
            && !things.iter().any(|t| t.id == sel)
        {
            *selected = None;
        }
        let before = *selected;
        let mut event = ListEvent::None;
        let dropped_here = self
            .state
            .drag_ended
            .as_ref()
            .filter(|d| d.live && r.contains(self.input.cursor))
            .cloned();
        if let Some(d) = &dropped_here {
            self.dropped = Some((name.to_string(), d.grid.clone()));
        }
        let mut hovered: Option<usize> = None;
        for (i, thing) in things.iter().enumerate() {
            let (row, col) = (i / cols, i % cols);
            if row < first_row || row >= first_row + rows_shown {
                continue;
            }
            let cell = Rect::new(
                r.x + col as f32 * (side + gap),
                r.y + (row - first_row) as f32 * (side + under + gap),
                side,
                side,
            );
            let over = cell.contains(self.input.cursor);
            let thing_id = format!("{id}:{}", thing.id);
            if over {
                hovered = Some(i);
            }
            if self.input.pressed && over {
                *selected = Some(thing.id);
                self.state.last_row = Some((id.clone(), i));
                self.row_pressed = true;
                self.state.focus = Some(id.clone());
                if !thing.fixed {
                    self.state.drag = Some(Drag {
                        grid: name.to_string(),
                        id: thing.id,
                        icon: thing.icon.clone(),
                        name: thing.name.clone(),
                        from: self.input.cursor,
                        live: false,
                    });
                }
            }
            let picked = *selected == Some(thing.id);
            let piece = if thing.off {
                "slot_off"
            } else if picked {
                "slot_picked"
            } else if thing.worn {
                "slot_worn"
            } else if over {
                "slot_hot"
            } else {
                "slot"
            };
            self.canvas.layer(LAYER_PLATES);
            if !self
                .canvas
                .frame(cell.x, cell.y, cell.w, cell.h, piece, s, [1.0; 4])
            {
                let fill = if picked {
                    PICKED
                } else if over {
                    BUTTON
                } else {
                    WELL
                };
                self.canvas.rect(cell.x, cell.y, cell.w, cell.h, fill);
                self.outline(
                    cell,
                    if thing.worn {
                        [0.3, 0.7, 0.3, 1.0]
                    } else {
                        EDGE
                    },
                );
            }
            self.canvas.layer(LAYER_INK);
            let inner = cell.inset(2.0 * s);
            let tint = if thing.off {
                [0.5, 0.5, 0.5, 1.0]
            } else {
                [1.0; 4]
            };
            let drawn = thing
                .icon
                .as_deref()
                .is_some_and(|k| self.canvas.icon(inner.x, inner.y, inner.w, k, tint));
            if drawn && let Some(k) = &thing.icon {
                self.note(SeenKind::Image, k, inner);
            }
            if !drawn {
                // No picture: the thing's initial, large, as a glyph of its own.
                let initial: String = thing.name.chars().take(2).collect();
                let tw = self.canvas.width_in(self.title_face, s, &initial);
                let (lh, _) = self.canvas.metrics(self.title_face);
                self.canvas.text_in(
                    self.title_face,
                    (cell.x + (cell.w - tw) * 0.5).round(),
                    (cell.y + (cell.h - lh * s) * 0.5).round(),
                    s,
                    if thing.off { OFF } else { FAINT },
                    &initial,
                );
            }
            if let Some(mark) = thing.mark {
                let m = 12.0 * s;
                let piece = match mark {
                    SlotMark::Worn => "mark_worn",
                    SlotMark::New => "mark_new",
                    SlotMark::Taken => "mark_taken",
                };
                if !self
                    .canvas
                    .image(cell.x + cell.w - m - s, cell.y + s, m, m, piece, [1.0; 4])
                {
                    let colour = match mark {
                        SlotMark::Worn => [0.3, 0.75, 0.3, 1.0],
                        SlotMark::New => FOCUS,
                        SlotMark::Taken => WARN,
                    };
                    self.canvas
                        .rect(cell.x + cell.w - m - s, cell.y + s, m, m, colour);
                }
            }
            if let Some(count) = thing.count.filter(|c| *c > 1) {
                let t = count.to_string();
                let tw = self.text_width(&t);
                self.ink(
                    cell.x + cell.w - tw - 2.0 * s,
                    cell.y + cell.h - self.ascent() - 2.0 * s,
                    TEXT,
                    &t,
                );
            }
            if under > 0.0 && !thing.under.is_empty() {
                let shown = self.fit_text(side, &thing.under);
                let tw = self.text_width(&shown);
                self.ink(
                    (cell.x + (cell.w - tw) * 0.5).round(),
                    cell.y + cell.h + s,
                    thing.under_colour,
                    &shown,
                );
            }
            // A script finds the thing by its words, as it found the row.
            let said = if !thing.said.is_empty() {
                thing.said.clone()
            } else if thing.under.is_empty() {
                thing.name.clone()
            } else {
                format!("{}  {}", thing.name, thing.under)
            };
            self.note(SeenKind::Row, &said, cell);
            let _ = thing_id;
        }
        // The empty slots up to the capacity: wells with nothing in them.
        for i in things.len()..capacity {
            let (row, col) = (i / cols, i % cols);
            if row < first_row || row >= first_row + rows_shown {
                continue;
            }
            let cell = Rect::new(
                r.x + col as f32 * (side + gap),
                r.y + (row - first_row) as f32 * (side + under + gap),
                side,
                side,
            );
            self.canvas.layer(LAYER_PLATES);
            if !self
                .canvas
                .frame(cell.x, cell.y, cell.w, cell.h, "slot", s, [1.0; 4])
            {
                self.canvas.rect(cell.x, cell.y, cell.w, cell.h, WELL);
                self.outline(cell, EDGE);
            }
            self.canvas.layer(LAYER_INK);
        }
        if rows_all > rows_shown {
            first_row = self.scrollbar(&id, r, first_row, rows_all, rows_shown);
        }
        self.state.scrolls.insert(id.clone(), first_row);
        // The tooltip, after 150 ms over one thing (LOOK.md 2.4).
        if let Some(i) = hovered {
            let key = format!("{id}:{}", things[i].id);
            let since = match &self.state.hover {
                Some((k, t)) if *k == key => *t,
                _ => self.input.time,
            };
            self.hovered = Some(key.clone());
            self.state.hover = Some((key, since));
            if self.input.time - since >= 0.15 && self.state.drag.as_ref().is_none_or(|d| !d.live) {
                let lines = tooltip(&things[i]);
                if !lines.is_empty() {
                    self.tooltip = Some((
                        Rect::new(self.input.cursor.0, self.input.cursor.1, 0.0, 0.0),
                        lines,
                    ));
                }
            }
        }
        if event == ListEvent::None && *selected != before {
            event = ListEvent::Picked;
        }
        event
    }

    /// A drop that ended on the grid or slot called `onto` this frame, from a slot of
    /// `from`: the thing's id. Asked after the grids were drawn.
    pub fn dropped(&self, onto: &str) -> Option<(String, i64)> {
        let d = self.state.drag_ended.as_ref()?;
        if !d.live {
            return None;
        }
        match &self.dropped {
            Some((target, from)) if target == onto => Some((from.clone(), d.id)),
            _ => None,
        }
    }

    /// A single slot that takes a drop (the weapon or armour slot of the equip panel,
    /// LOOK.md 4): drawn like a grid's cell with what is in it, named for scripts, and
    /// `Some(id)` when a dragged thing was let go on it this frame.
    pub fn drop_slot(
        &mut self,
        r: Rect,
        name: &str,
        thing: Option<&SlotThing>,
        tooltip: &dyn Fn(&SlotThing) -> TipLines,
    ) -> Option<(String, i64)> {
        let s = self.scale;
        let over = r.contains(self.input.cursor);
        let dragging = self.state.drag.as_ref().is_some_and(|d| d.live);
        // A press on what is in the slot may become a drag out of it (a worn thing dragged
        // into the grid comes off; Gemini's review).
        if self.input.pressed
            && over
            && let Some(t) = thing
        {
            self.state.focus = Some(format!("slot:{name}"));
            self.state.drag = Some(Drag {
                grid: name.to_string(),
                id: t.id,
                icon: t.icon.clone(),
                name: t.name.clone(),
                from: self.input.cursor,
                live: false,
            });
        }
        let piece = if over && dragging {
            "slot_hot"
        } else if thing.is_some() {
            "slot_worn"
        } else {
            "slot"
        };
        self.canvas.layer(LAYER_PLATES);
        if !self.canvas.frame(r.x, r.y, r.w, r.h, piece, s, [1.0; 4]) {
            self.canvas.rect(r.x, r.y, r.w, r.h, WELL);
            self.outline(r, if over && dragging { FOCUS } else { EDGE });
        }
        self.canvas.layer(LAYER_INK);
        match thing {
            Some(t) => {
                let inner = r.inset(2.0 * s);
                let drawn = t
                    .icon
                    .as_deref()
                    .is_some_and(|k| self.canvas.icon(inner.x, inner.y, inner.w, k, [1.0; 4]));
                if drawn && let Some(k) = &t.icon {
                    self.note(SeenKind::Image, k, inner);
                }
                if !drawn {
                    let initial: String = t.name.chars().take(2).collect();
                    let tw = self.canvas.width_in(self.title_face, s, &initial);
                    let (lh, _) = self.canvas.metrics(self.title_face);
                    self.canvas.text_in(
                        self.title_face,
                        (r.x + (r.w - tw) * 0.5).round(),
                        (r.y + (r.h - lh * s) * 0.5).round(),
                        s,
                        FAINT,
                        &initial,
                    );
                }
                self.note(SeenKind::Row, &format!("{name}: {}", t.name), r);
                if over {
                    let key = format!("slot:{name}");
                    let since = match &self.state.hover {
                        Some((k, t)) if *k == key => *t,
                        _ => self.input.time,
                    };
                    self.hovered = Some(key.clone());
                    self.state.hover = Some((key, since));
                    if self.input.time - since >= 0.15 && !dragging {
                        let lines = tooltip(t);
                        if !lines.is_empty() {
                            self.tooltip = Some((
                                Rect::new(self.input.cursor.0, self.input.cursor.1, 0.0, 0.0),
                                lines,
                            ));
                        }
                    }
                }
            }
            None => {
                // What goes here, whole: in small print when the word is wider than
                // the slot.
                let room = r.w - 4.0 * s;
                let print = if self.text_width(name) <= room {
                    s
                } else {
                    (s * 0.5).max(1.0)
                };
                let mut shown = name.to_string();
                while self.canvas.width_in(self.face, print, &shown) > room && shown.pop().is_some()
                {
                }
                let tw = self.canvas.width_in(self.face, print, &shown);
                self.canvas.text_in(
                    self.face,
                    (r.x + (r.w - tw) * 0.5).round(),
                    (r.y + (r.h - self.ascent_dots * print) * 0.5).round(),
                    print,
                    OFF,
                    &shown,
                );
                self.note(SeenKind::Row, &format!("{name}: nothing"), r);
            }
        }
        let d = self.state.drag_ended.as_ref()?;
        if d.live && over {
            Some((d.grid.clone(), d.id))
        } else {
            None
        }
    }

    /// A portrait by icon key in its frame, `side` square; the initial of `name` when the
    /// atlas lacks it.
    #[allow(dead_code)]
    pub fn portrait(&mut self, r: Rect, icon: Option<&str>, name: &str) {
        let s = self.scale;
        self.canvas.layer(LAYER_PLATES);
        if !self
            .canvas
            .frame(r.x, r.y, r.w, r.h, "portrait_frame", s, [1.0; 4])
        {
            self.canvas.rect(r.x, r.y, r.w, r.h, WELL);
            self.outline(r, EDGE);
        }
        self.canvas.layer(LAYER_INK);
        let inner = r.inset(6.0 * s);
        let drawn = icon.is_some_and(|k| {
            self.canvas
                .icon(inner.x, inner.y, inner.w.min(inner.h), k, [1.0; 4])
        });
        if drawn && let Some(k) = icon {
            self.note(SeenKind::Image, k, inner);
        }
        if !drawn {
            let initial: String = name.chars().take(1).collect();
            let tw = self.canvas.width_in(self.title_face, s, &initial);
            let (lh, _) = self.canvas.metrics(self.title_face);
            self.canvas.text_in(
                self.title_face,
                (r.x + (r.w - tw) * 0.5).round(),
                (r.y + (r.h - lh * s) * 0.5).round(),
                s,
                FAINT,
                &initial,
            );
        }
    }

    /// A bar in its frame, filled to `frac`, with `text` inside it.
    #[allow(dead_code)]
    pub fn bar(&mut self, r: Rect, frac: f32, fill: [f32; 4], text: &str) {
        let s = self.scale;
        self.canvas.layer(LAYER_PLATES);
        let framed = self
            .canvas
            .frame(r.x, r.y, r.w, r.h, "bar_frame", s, [1.0; 4]);
        let inner = if framed { r.inset(3.0 * s) } else { r.inset(s) };
        if !framed {
            self.canvas.rect(r.x, r.y, r.w, r.h, [0.0, 0.0, 0.0, 0.55]);
        }
        let w = (inner.w * frac.clamp(0.0, 1.0)).round();
        if w > 0.0
            && !self
                .canvas
                .image(inner.x, inner.y, w, inner.h, "bar_fill", fill)
        {
            self.canvas.rect(inner.x, inner.y, w, inner.h, fill);
        }
        self.canvas.layer(LAYER_INK);
        if !text.is_empty() {
            let tw = self.text_width(text);
            self.ink(
                (r.x + (r.w - tw) * 0.5).round(),
                (r.y + (r.h - self.ascent()) * 0.5).round(),
                TEXT,
                text,
            );
            self.note(SeenKind::Label, text, r);
        }
    }

    /// A rectangle a body is drawn into by the app (LOOK.md 5): a dark well now, the body
    /// over it on layer 1; a drag across it turns the body.
    pub fn paperdoll(&mut self, r: Rect) {
        self.paperdoll_in(r, true);
    }

    /// The same body in the open: no well, drawn over what is behind the screen (the
    /// map behind the selector, CLIENT.md 4.2).
    pub fn paperdoll_open(&mut self, r: Rect) {
        self.paperdoll_in(r, false);
    }

    fn paperdoll_in(&mut self, r: Rect, well: bool) {
        let s = self.scale;
        let id = "paperdoll";
        let (_, held, _) = self.clicked(id, r);
        // A drag across the body turns it: one full turn per `TURN_DOTS` units dragged.
        // Not on the frame of the press itself, whose last cursor is wherever the
        // pointer (or the last finger) was before.
        if held && self.input.down && !self.input.pressed {
            self.state.paperdoll_turn +=
                (self.input.cursor.0 - self.input.last_cursor.0) / (TURN_DOTS * s);
        }
        if well {
            self.canvas.layer(LAYER_PLATES);
            if !self.canvas.frame(r.x, r.y, r.w, r.h, "well", s, [1.0; 4]) {
                self.canvas.rect(r.x, r.y, r.w, r.h, WELL);
                self.outline(r, EDGE);
            }
            self.canvas.layer(LAYER_INK);
        }
        self.state.paperdolls.push(Paperdoll {
            rect: r.inset(2.0 * s),
            turn: self.state.paperdoll_turn,
            open: !well,
        });
        self.note(SeenKind::Label, "paperdoll", r);
    }

    /// End the frame: Tab moves the focus through what was drawn, and what was drawn is
    /// kept for scripts and tests.
    pub fn end(mut self) {
        // The drag: live once the press has moved four dots; drawn over everything.
        let mut shown_drag: Option<Drag> = None;
        if let Some(d) = &mut self.state.drag {
            if !self.input.down {
                self.state.drag = None;
            } else {
                let moved =
                    (self.input.cursor.0 - d.from.0).abs() + (self.input.cursor.1 - d.from.1).abs();
                if moved >= 4.0 * self.scale {
                    d.live = true;
                }
                if d.live {
                    shown_drag = Some(d.clone());
                }
            }
        }
        if let Some(d) = shown_drag {
            let side = 32.0 * self.scale;
            let (x, y) = (
                self.input.cursor.0 - side * 0.5,
                self.input.cursor.1 - side * 0.5,
            );
            self.canvas.layer(LAYER_OVER);
            let drawn = d
                .icon
                .as_deref()
                .is_some_and(|k| self.canvas.icon(x, y, side, k, [1.0, 1.0, 1.0, 0.85]));
            if !drawn {
                self.canvas.rect(x, y, side, side, PICKED);
                self.canvas.text_in(
                    self.face,
                    x + 2.0 * self.scale,
                    y + 2.0 * self.scale,
                    self.scale,
                    TEXT,
                    &d.name,
                );
            }
            self.note(
                SeenKind::Label,
                &format!("dragging {}", d.name),
                Rect::new(x, y, side, side),
            );
        }
        if self.hovered.is_none() {
            self.state.hover = None;
        }
        // The tooltip, beside the pointer and inside the frame.
        if let Some((at, lines)) = self.tooltip.take() {
            let s = self.scale;
            let pad = 6.0 * s;
            let line = self.line();
            let w = lines
                .iter()
                .map(|(t, _)| self.text_width(t))
                .fold(0.0, f32::max)
                + 2.0 * pad;
            let h = line * lines.len() as f32 + 2.0 * pad;
            let (fw, fh) = self.size();
            let x = (at.x + 16.0 * s).min(fw - w).max(0.0).round();
            let y = (at.y + 16.0 * s).min(fh - h).max(0.0).round();
            self.canvas.layer(LAYER_OVER);
            if !self.canvas.frame(x, y, w, h, "tooltip", s, [1.0; 4]) {
                self.canvas.rect(x, y, w, h, [0.04, 0.03, 0.03, 0.94]);
                self.outline(Rect::new(x, y, w, h), EDGE);
            }
            for (i, (t, c)) in lines.iter().enumerate() {
                self.canvas
                    .text_in(self.face, x + pad, y + pad + line * i as f32, s, *c, t);
            }
            let whole: Vec<String> = lines.iter().map(|(t, _)| t.clone()).collect();
            self.note(
                SeenKind::Label,
                &format!("tooltip: {}", whole.join(" / ")),
                Rect::new(x, y, w, h),
            );
        }
        self.canvas.layer(LAYER_INK);
        self.state.drag_ended = None;
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
        self.state.cut_cells = std::mem::take(&mut self.cut);
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

/// Text broken at spaces into lines no wider than `w` pixels as `measure` says; a word
/// wider than a line is cut where it stops fitting.
pub fn wrap_measured(text: &str, w: f32, measure: impl Fn(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let mut word = word.to_string();
            loop {
                let with = if line.is_empty() {
                    word.clone()
                } else {
                    format!("{line} {word}")
                };
                if measure(&with) <= w {
                    line = with;
                    break;
                }
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                    continue;
                }
                // A word wider than a line: what fits, and the rest goes on.
                let mut head = String::new();
                for c in word.chars() {
                    head.push(c);
                    if measure(&head) > w && head.chars().count() > 1 {
                        head.pop();
                        break;
                    }
                }
                let rest: String = word.chars().skip(head.chars().count()).collect();
                lines.push(head);
                if rest.is_empty() {
                    break;
                }
                word = rest;
            }
        }
        lines.push(line);
    }
    lines
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
        tidy_in(canvas, state, need, false);
    }

    /// As `tidy`; `whole` for a screen laid out across the frame (the selector,
    /// CLIENT.md 4.2): then the frame is its panel and the height rule is the frame's.
    pub fn tidy_in(canvas: &Recorder, state: &UiState, need: f32, whole: bool) {
        let scale = scale_for(canvas.size, need, 0);
        let (w, h) = canvas.size;
        let panel = if whole {
            Rect::new(0.0, 0.0, w, h)
        } else {
            canvas.rects.first().expect("a panel was drawn").0
        };
        assert!(
            whole || panel.h <= PANEL_HIGH * scale + 0.5,
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
    fn a_drag_across_the_paperdoll_turns_it_by_how_far_it_moved() {
        let mut canvas = Recorder::new(1920.0, 1080.0);
        let mut st = UiState::default();
        let r = Rect::new(600.0, 100.0, 700.0, 800.0);
        let mut input = UiInput {
            cursor: (900.0, 500.0),
            last_cursor: (0.0, 0.0),
            pressed: true,
            down: true,
            ..Default::default()
        };
        let s = scale_for(canvas.size(), 100.0, 0);
        let show = |canvas: &mut Recorder, st: &mut UiState, input: &UiInput| {
            let mut ui = Ui::begin(canvas, st, input, "equip", 100.0);
            ui.paperdoll(r);
            ui.end();
        };
        show(&mut canvas, &mut st, &input);
        assert_eq!(st.paperdoll_turn, 0.0, "the press itself turns nothing");
        input.pressed = false;
        input.last_cursor = input.cursor;
        input.cursor = (900.0 + TURN_DOTS * s * 0.5, 500.0);
        show(&mut canvas, &mut st, &input);
        assert!(
            (st.paperdoll_turn - 0.5).abs() < 1e-5,
            "half a turn: {}",
            st.paperdoll_turn
        );
        // The frame after, with the pointer still: no more turning.
        input.last_cursor = input.cursor;
        show(&mut canvas, &mut st, &input);
        assert!((st.paperdoll_turn - 0.5).abs() < 1e-5);
        input.down = false;
        input.released = true;
        input.cursor = (100.0, 100.0);
        show(&mut canvas, &mut st, &input);
        assert!(
            (st.paperdoll_turn - 0.5).abs() < 1e-5,
            "let go: the body stays"
        );
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
        // A phone (2.625 at 2340 by 1080 lines) asks for 3 and gets it in fullscreen; a
        // tablet at 2 gets 2; a desktop's 1 is still 2, a finger being wide.
        assert_eq!(touch_scale(2.625), 3);
        assert_eq!(touch_scale(2.0), 2);
        assert_eq!(touch_scale(1.0), 2);
        assert_eq!(touch_scale(4.5), 4);
        assert_eq!(scale_for((2340.0, 1080.0), 340.0, touch_scale(2.625)), 3.0);
        assert_eq!(scale_for((2340.0, 980.0), 340.0, touch_scale(2.625)), 2.0);
        assert_eq!(scale_for((640.0, 360.0), 300.0, 0), 1.0);
        assert_eq!(scale_for((1280.0, 720.0), 300.0, 0), 2.0);
        assert_eq!(scale_for((1920.0, 1080.0), 300.0, 0), 2.0);
        assert_eq!(scale_for((3840.0, 2160.0), 300.0, 0), 4.0);
        // Too low for the tallest panel (340 units since the inventory grew its grid and
        // equip panel, LOOK.md 4) at the scale its height would give: one less.
        assert_eq!(scale_for((960.0, 540.0), 300.0, 0), 1.0);
        assert_eq!(scale_for((1024.0, 600.0), 300.0, 0), 1.0);
        assert_eq!(scale_for((1024.0, 760.0), 300.0, 0), 2.0);
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
