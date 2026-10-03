//! What a person sees while playing when the game itself is not what they are looking at
//! (CLIENT.md 4.4, 4.5, 5): the game menu and what it opens, and the chat.

use std::collections::VecDeque;

use gm_hub_proto::player::{PlayerRequest, PlayerResponse};
use gm_hub_proto::protocol::{SessionId, ZoneSummary};
use web_time::Instant;

use crate::hub::{Answer, HubApi, Pending};
use crate::settings::{SENSITIVITY_RANGE, Settings};
use crate::ui::{self, Canvas, Column, Field, Key, ListEvent, Rect, Ui};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Main,
    Travel,
    Settings,
    Keys,
}

impl Page {
    /// The name a UI script waits for (CLIENT.md 9).
    pub fn name(self) -> &'static str {
        match self {
            Page::Main => "menu",
            Page::Travel => "travel",
            Page::Settings => "settings",
            Page::Keys => "keys",
        }
    }
}

/// What the app must do after a frame of the menu.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    None,
    /// Close the menu and play on.
    Resume,
    /// Close the menu and open the inventory (ITEMS.md 6).
    Inventory,
    /// Close the menu and open the page of people (PARTY.md 8).
    People,
    Travel(String),
    /// Say goodbye to the zone and show the characters.
    Leave,
    Quit,
    /// A setting changed: apply it now.
    Changed,
}

/// What the menu may offer: a client that joined a zone directly has no hub to travel
/// through and no characters to go back to.
#[derive(Clone, Copy, Debug)]
pub struct Offers {
    pub inventory: bool,
    pub people: bool,
    pub travel: bool,
    pub leave: bool,
    pub fullscreen: bool,
    /// Not in a browser: its tab is closed by the browser.
    pub quit: bool,
}

pub struct GameMenu {
    pub page: Page,
    zones: Vec<ZoneSummary>,
    picked: usize,
    wait: Option<Pending<Answer>>,
    notice: String,
}

impl Default for GameMenu {
    fn default() -> GameMenu {
        GameMenu {
            page: Page::Main,
            zones: Vec::new(),
            picked: 0,
            wait: None,
            notice: String::new(),
        }
    }
}

impl GameMenu {
    /// One frame of the menu. `ui` was begun for `self.page.name()`. `here` is the zone
    /// being played, which is not somewhere to travel to.
    pub fn frame<C: Canvas>(
        &mut self,
        ui: &mut Ui<'_, C>,
        hub: Option<(&dyn HubApi, SessionId)>,
        offers: Offers,
        here: &str,
        settings: &mut Settings,
    ) -> MenuAction {
        match self.page {
            Page::Main => self.main(ui, hub, offers),
            Page::Travel => self.travel(ui, here),
            Page::Settings => self.settings(ui, offers, settings),
            Page::Keys => self.keys(ui),
        }
    }

    fn main<C: Canvas>(
        &mut self,
        ui: &mut Ui<'_, C>,
        hub: Option<(&dyn HubApi, SessionId)>,
        offers: Offers,
    ) -> MenuAction {
        let s = ui.scale;
        let gap = 5.0 * s;
        let h = ui.button_height();
        let inner = 8.0 * (h + gap) + 2.0 * ui.line();
        let panel = Rect::centred(ui.size(), 160.0 * s, ui.panel_height(inner, true));
        let inner = ui.panel(panel, "menu");
        let mut col = Column::new(inner, gap);
        if ui.button(col.take(h), "Resume") || ui.key(Key::Escape) {
            return MenuAction::Resume;
        }
        if ui.button_if(col.take(h), "Inventory", offers.inventory) {
            return MenuAction::Inventory;
        }
        if ui.button_if(col.take(h), "People", offers.people) {
            return MenuAction::People;
        }
        if ui.button_if(col.take(h), "Travel", offers.travel && hub.is_some())
            && let Some((hub, session)) = hub
        {
            self.page = Page::Travel;
            self.notice.clear();
            self.wait = Some(hub.call(PlayerRequest::ListZones { session }));
        }
        if ui.button(col.take(h), "Settings") {
            self.page = Page::Settings;
        }
        if ui.button(col.take(h), "Keys") {
            self.page = Page::Keys;
        }
        if ui.button_if(col.take(h), "Leave", offers.leave) {
            return MenuAction::Leave;
        }
        if ui.button_if(col.take(h), "Quit", offers.quit) {
            return MenuAction::Quit;
        }
        // There is no pause in a shared world.
        ui.paragraph(
            col.rest(),
            ui::FAINT,
            "The world goes on while this is open.",
        );
        MenuAction::None
    }

    /// What the keys do: nothing else tells a new player.
    fn keys<C: Canvas>(&mut self, ui: &mut Ui<'_, C>) -> MenuAction {
        const KEYS: &[(&str, &str)] = &[
            ("W A S D", "walk"),
            ("Space", "jump"),
            ("mouse", "look around"),
            ("left button", "the primary attack"),
            ("right button", "the secondary"),
            ("C or Ctrl", "guard"),
            ("1 2 3 4", "the actives (Shift is 1)"),
            ("V", "first or third person"),
            ("Tab", "the tactical view and the squad"),
            ("Enter", "say something"),
            ("F9", "report the player you look at"),
            ("I", "the inventory"),
            ("P", "people, the party, a trade"),
            ("E", "look at the stall you stand at"),
            ("B and N", "open, close a stall on a tile"),
            ("Escape", "this menu"),
        ];
        let s = ui.scale;
        let gap = 6.0 * s;
        let line = ui.line();
        let inner = KEYS.len() as f32 * line + gap + ui.button_height();
        let panel = Rect::centred(ui.size(), 290.0 * s, ui.panel_height(inner, true));
        let inner = ui.panel(panel, "keys");
        let mut col = Column::new(inner, 0.0);
        for (key, does) in KEYS {
            let row = col.take(line);
            ui.label(row.x, row.y, row.w * 0.3, ui::TEXT, key);
            ui.label(row.x + row.w * 0.32, row.y, row.w * 0.68, ui::FAINT, does);
        }
        let row = col.take(gap + ui.button_height());
        let back = Rect::new(row.x, row.y + gap, row.w, ui.button_height());
        let row = ui.buttons(back, &["Back"]);
        if ui.button(row[0], "Back") || ui.key(Key::Escape) {
            self.page = Page::Main;
        }
        MenuAction::None
    }

    fn travel<C: Canvas>(&mut self, ui: &mut Ui<'_, C>, here: &str) -> MenuAction {
        if let Some(answer) = self.wait.as_ref().and_then(|w| w.take()) {
            self.wait = None;
            match answer {
                Ok(PlayerResponse::Zones(zones)) => {
                    self.zones = zones;
                    self.picked = self.zones.iter().position(|z| z.id != here).unwrap_or(0);
                }
                // (Not printed whole: the words for every answer of the hub's would be
                // carried by every browser for this one line.)
                Ok(_) => self.notice = "the hub answered something else".into(),
                Err(e) => self.notice = e.to_string(),
            }
        }
        let s = ui.scale;
        let gap = 6.0 * s;
        let list = ui.list_height(6);
        let inner = list + gap + ui.line() + gap + ui.button_height();
        let panel = Rect::centred(ui.size(), 260.0 * s, ui.panel_height(inner, true));
        let inner = ui.panel(panel, "travel");
        let mut col = Column::new(inner, gap);
        // A zone that cannot be gone to says why: this one, a full one, one a browser
        // cannot reach. (What a zone asks of a character, the zone itself answers.)
        let closed = |z: &ZoneSummary| {
            if z.id == here {
                Some("here")
            } else if z.players >= z.max_players {
                Some("full")
            } else if cfg!(target_arch = "wasm32") && !z.web {
                Some("not from a browser")
            } else {
                None
            }
        };
        let rows: Vec<Vec<String>> = self
            .zones
            .iter()
            .map(|z| {
                vec![
                    z.id.clone(),
                    z.map.clone(),
                    match closed(z) {
                        Some(why) => why.to_string(),
                        None => format!("{} playing", z.players),
                    },
                ]
            })
            .collect();
        ui.focus_default("list", "zones");
        let event = ui.list(
            col.take(list),
            "zones",
            &[0.0, 0.36, 0.64],
            &rows,
            &mut self.picked,
        );
        let notice = col.take(ui.line());
        if self.wait.is_some() {
            ui.label(notice.x, notice.y, notice.w, ui::FAINT, "asking the hub");
        } else if !self.notice.is_empty() {
            ui.label(notice.x, notice.y, notice.w, ui::WARN, &self.notice);
        }
        let row = ui.buttons(col.take(ui.button_height()), &["Go", "Back"]);
        let target = self.zones.get(self.picked).filter(|z| closed(z).is_none());
        let mut go = ui.button_if(row[0], "Go", target.is_some());
        go |= target.is_some() && event == ListEvent::Activated;
        if ui.button(row[1], "Back") || ui.key(Key::Escape) {
            self.page = Page::Main;
        }
        // Enter that no button took is the page's own: Go.
        go |= target.is_some() && ui.key(Key::Enter);
        match (go, target) {
            (true, Some(zone)) => MenuAction::Travel(zone.id.clone()),
            _ => MenuAction::None,
        }
    }

    fn settings<C: Canvas>(
        &mut self,
        ui: &mut Ui<'_, C>,
        offers: Offers,
        settings: &mut Settings,
    ) -> MenuAction {
        let s = ui.scale;
        let gap = 6.0 * s;
        let h = ui.button_height();
        let boxes = if offers.fullscreen { 4.0 } else { 3.0 };
        let inner = 3.0 * (ui.field_height() + gap) + boxes * (h + gap) + h;
        let panel = Rect::centred(ui.size(), 240.0 * s, ui.panel_height(inner, true));
        let inner = ui.panel(panel, "settings");
        let mut col = Column::new(inner, gap);
        let mut changed = ui.slider(
            col.take(ui.field_height()),
            "mouse sensitivity",
            &mut settings.sensitivity,
            SENSITIVITY_RANGE,
        );
        // The sound (SOUND.md 3.2): how loud, and whether at all.
        let mut volume = settings.volume.min(100) as f32;
        if ui.slider(
            col.take(ui.field_height()),
            "volume",
            &mut volume,
            (0.0, 100.0),
        ) {
            settings.volume = volume.round() as u8;
            changed = true;
        }
        changed |= ui.checkbox(col.take(h), "no sound", &mut settings.mute);
        // The size of everything drawn here: what the window gives, or one of four.
        let mut size = settings.ui_scale.min(4) as usize;
        if ui.choice(
            col.take(ui.field_height()),
            "size of text",
            &["by the window", "1", "2", "3", "4"],
            &mut size,
        ) {
            settings.ui_scale = size as u8;
            changed = true;
        }
        changed |= ui.checkbox(
            col.take(h),
            "invert the mouse's up and down",
            &mut settings.invert,
        );
        changed |= ui.checkbox(
            col.take(h),
            "start in third person",
            &mut settings.third_person,
        );
        if offers.fullscreen {
            changed |= ui.checkbox(col.take(h), "fullscreen", &mut settings.fullscreen);
        }
        let row = ui.buttons(col.take(h), &["Back"]);
        if ui.button(row[0], "Back") || ui.key(Key::Escape) {
            self.page = Page::Main;
        }
        if changed {
            MenuAction::Changed
        } else {
            MenuAction::None
        }
    }
}

/// What the screen for no hub says: the settings file a hub is named in, and what is
/// wrong with the one that was named, if one was.
#[derive(Clone, Debug)]
pub struct Title {
    pub settings: String,
    pub why: Option<String>,
}

/// The screen of a client that a person started and that has no hub to talk to
/// (CLIENT.md 2): it says why, where one is named, and offers the offline walk.
/// `Some(true)`: walk; `Some(false)`: quit.
pub fn title<C: Canvas>(ui: &mut Ui<'_, C>, title: &Title) -> Option<bool> {
    let s = ui.scale;
    let line = ui.line();
    let why = title
        .why
        .as_deref()
        .unwrap_or("No hub is named, so there is nobody to play with yet.");
    let how = format!(
        "A hub is named in {} (hub = \"address:port\", hub_cert = \"its certificate file\"), or with --hub.",
        title.settings
    );
    // As tall as what it has to say: the paths are as long as they are.
    let width = 280.0 * s;
    let fit = ui.fit(width - 16.0 * s).max(1);
    let (top, rows) = (
        ui::wrap(why, fit).len() as f32,
        ui::wrap(&how, fit).len() as f32,
    );
    let gap = 6.0 * s;
    let inner = (top * line + gap) + (rows * line + gap) + ui.button_height();
    let panel = Rect::centred(ui.size(), width, ui.panel_height(inner, true));
    let inner = ui.panel(panel, "gamengine");
    let mut col = Column::new(inner, gap);
    ui.paragraph(col.take(top * line), ui::TEXT, why);
    ui.paragraph(col.take(rows * line), ui::FAINT, &how);
    let row = ui.buttons(
        col.take(ui.button_height()),
        &["Walk around offline", "Quit"],
    );
    if ui.button(row[0], "Walk around offline") {
        return Some(true);
    }
    if ui.button(row[1], "Quit") {
        return Some(false);
    }
    None
}

/// How long a chat line stays on the screen, how many are shown, and how many while the
/// line is open (CLIENT.md 5).
const CHAT_SECS: f32 = 12.0;
const CHAT_SHOWN: usize = 8;
const CHAT_SHOWN_OPEN: usize = 30;
const CHAT_KEPT: usize = 60;
/// Rows one message may take on the screen.
const CHAT_ROWS: usize = 4;

/// Where a line of the log came from (PARTY.md 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Voice {
    /// The zone itself: a refusal, a notice.
    Zone,
    /// Somebody, to everybody in the zone.
    Say,
    /// A member of the party, to the party.
    Party,
    /// Somebody, to this player alone.
    Whisper,
    /// This player's own whisper, as it went out (the name is whom it went to).
    Whispered,
}

/// A colour for the party's lines and one for whispers.
const PARTY_INK: [f32; 4] = [0.55, 0.85, 0.60, 1.0];
const WHISPER_INK: [f32; 4] = [0.80, 0.65, 0.95, 1.0];

/// What the line that was typed asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Said {
    /// To everybody in the zone.
    Say(String),
    Party(String),
    Whisper {
        to: String,
        text: String,
    },
    Invite(String),
    Leave,
}

/// The chat: what was said, and the line being typed.
#[derive(Default)]
pub struct Chat {
    pub open: bool,
    line: String,
    /// When, whose voice, who (for the zone's own lines nobody), what.
    log: VecDeque<(Instant, Voice, String, String)>,
    /// Who whispered last: whom `/r` answers.
    reply_to: Option<String>,
    /// The line was written by a page or a rewrite, not typed: the caret goes to its end.
    written: bool,
}

fn ignores(ignored: &[String], who: &str) -> bool {
    use gm_hub_proto::names::skeleton;
    ignored.iter().any(|i| skeleton(i) == skeleton(who))
}

impl Chat {
    fn keep(&mut self, voice: Voice, who: String, text: String) {
        self.log.push_back((Instant::now(), voice, who, text));
        while self.log.len() > CHAT_KEPT {
            self.log.pop_front();
        }
    }

    /// A line arrived. Lines of players on the `ignored` list are not kept. Whether the
    /// line was kept (what is not shown is not heard either, SOUND.md 3).
    pub fn heard(&mut self, who: Option<String>, text: String, ignored: &[String]) -> bool {
        match who {
            Some(who) if ignores(ignored, &who) => false,
            Some(who) => {
                self.keep(Voice::Say, who, text);
                true
            }
            None => {
                self.keep(Voice::Zone, String::new(), text);
                true
            }
        }
    }

    /// A line that came through the hub (PARTY.md 5): the party's, a whisper, or this
    /// player's own whisper as it went out. Somebody who is not heard is not heard here.
    /// Whether the line was kept.
    pub fn heard_on(
        &mut self,
        channel: u8,
        from: String,
        text: String,
        ignored: &[String],
    ) -> bool {
        use gm_net::control::{CHANNEL_PARTY, CHANNEL_WHISPER, CHANNEL_WHISPERED};
        let voice = match channel {
            CHANNEL_PARTY => Voice::Party,
            CHANNEL_WHISPER => Voice::Whisper,
            CHANNEL_WHISPERED => Voice::Whispered,
            _ => return false,
        };
        if voice != Voice::Whispered && ignores(ignored, &from) {
            return false;
        }
        if voice == Voice::Whisper {
            self.reply_to = Some(from.clone());
        }
        self.keep(voice, from, text);
        true
    }

    /// The zone's own last line, when it is not older than `within`: a page whose button
    /// the line answers shows it too (the page may lie over the log).
    pub fn zone_said(&self, within: std::time::Duration) -> Option<&str> {
        self.log
            .iter()
            .rev()
            .find(|l| l.1 == Voice::Zone)
            .filter(|l| l.0.elapsed() < within)
            .map(|l| l.3.as_str())
    }

    /// Open the line with something already on it (a whisper to somebody picked on a
    /// page).
    pub fn open_with(&mut self, line: String) {
        self.open = true;
        self.line = line;
        self.written = true;
    }

    /// Everything of this character's (a leave, another character): nothing of it is
    /// kept for the next.
    pub fn clear(&mut self) {
        self.open = false;
        self.line.clear();
        self.log.clear();
        self.reply_to = None;
    }

    /// Close the line without sending it (what Escape does).
    pub fn drop_line(&mut self) {
        self.open = false;
        self.line.clear();
    }

    /// What a line that begins with `/` asks for. `/ignore NAME` and `/unignore NAME` are
    /// the client's own (speech cannot be reported, ANTICHEAT.md 5: not hearing somebody
    /// is what there is); the rest are said to the zone (PARTY.md 5). A name after `/w`
    /// is one word: a name with a space in it is written without.
    fn command(&mut self, rest: &str, ignored: &mut Vec<String>) -> Option<Said> {
        use gm_hub_proto::names::{character_name, skeleton};
        let (word, rest) = rest.split_once(' ').unwrap_or((rest, ""));
        let rest = rest.trim();
        let line = |text: &str| gm_net::control::valid_chat(text);
        let said = match (word, rest.is_empty()) {
            ("p", false) => return line(rest).map(Said::Party),
            ("w", false) => {
                let (to, text) = rest.split_once(' ').unwrap_or((rest, ""));
                match (character_name(to), line(text)) {
                    (Ok(to), Some(text)) => return Some(Said::Whisper { to, text }),
                    (Err(_), _) => format!("nobody is called {to}"),
                    (_, None) => "a whisper is /w NAME and what to say".to_string(),
                }
            }
            ("r", false) => match (&self.reply_to, line(rest)) {
                (Some(to), Some(text)) => {
                    // (Names are told apart without what is between their letters.)
                    let to = to.replace(' ', "");
                    return Some(Said::Whisper { to, text });
                }
                (None, _) => "nobody has whispered to you".to_string(),
                (_, None) => return None,
            },
            ("leave", true) => return Some(Said::Leave),
            ("invite" | "ignore" | "unignore", false) if character_name(rest).is_err() => {
                format!("nobody is called {rest}")
            }
            ("invite", false) => return Some(Said::Invite(rest.to_string())),
            ("ignore", false) => {
                if !ignores(ignored, rest) {
                    ignored.push(rest.to_string());
                }
                format!("not hearing {rest} any more")
            }
            ("unignore", false) => {
                ignored.retain(|i| skeleton(i) != skeleton(rest));
                format!("hearing {rest} again")
            }
            ("ignore", true) if ignored.is_empty() => "nobody is ignored".to_string(),
            ("ignore", true) => format!("ignored: {}", ignored.join(", ")),
            _ => {
                "/p TEXT, /w NAME TEXT, /r TEXT, /invite NAME, /leave, /ignore NAME, /unignore NAME"
                    .to_string()
            }
        };
        self.keep(Voice::Zone, String::new(), said);
        None
    }

    /// One frame of the chat, bottom left above the own bars: the log, and the line when it
    /// is open. Returns what the line that was sent asks for.
    pub fn frame<C: Canvas>(
        &mut self,
        ui: &mut Ui<'_, C>,
        ignored: &mut Vec<String>,
    ) -> Option<Said> {
        let s = ui.scale;
        let (w, h) = ui.size();
        let line = ui.line();
        let width = (w * 0.45).min(120.0 * crate::font::ADVANCE * s);
        // Above the three bars of the HUD and the command stance line.
        let bottom = h - 16.0 - line * 5.0;
        let show = if self.open {
            CHAT_SHOWN_OPEN
        } else {
            CHAT_SHOWN
        };
        let fit = ui.fit(width).max(8);
        let mut lines: Vec<(String, [f32; 4])> = Vec::new();
        for (at, voice, who, text) in self.log.iter().rev() {
            if !self.open && at.elapsed().as_secs_f32() > CHAT_SECS {
                break;
            }
            // The zone's own lines, the party's and the whispers are marked with what no
            // name can begin with (a star, a bracket), and drawn in colours of their own:
            // nothing somebody says aloud can look like one of them.
            let (said, ink) = match voice {
                Voice::Zone => (format!("* {text}"), ui::WARN),
                Voice::Say => (format!("{who}: {text}"), ui::TEXT),
                Voice::Party => (format!("[party] {who}: {text}"), PARTY_INK),
                Voice::Whisper => (format!("[whisper] {who}: {text}"), WHISPER_INK),
                Voice::Whispered => (format!("[to {who}] {text}"), WHISPER_INK),
            };
            // A long line wraps (runs of spaces are one space): at most `CHAT_ROWS` rows,
            // and every row after the first is indented, so that nothing a person types
            // can begin a row the way somebody else's line would.
            let mut rows = ui::wrap(&said, fit.saturating_sub(2).max(6));
            if rows.len() > CHAT_ROWS {
                rows.truncate(CHAT_ROWS);
                if let Some(last) = rows.last_mut() {
                    last.push_str("...");
                }
            }
            // The newest at the bottom, each message's rows in order.
            for (i, part) in rows.into_iter().enumerate().rev() {
                let part = if i == 0 { part } else { format!("  {part}") };
                lines.push((part, ink));
            }
            if lines.len() >= show {
                break;
            }
        }
        lines.truncate(show);
        let field_h = if self.open {
            ui.button_height() + 4.0 * s
        } else {
            0.0
        };
        for (i, (text, ink)) in lines.iter().enumerate() {
            let y = bottom - field_h - line * (i as f32 + 1.0);
            if y < 0.0 {
                break;
            }
            let tw = ui.text_width(text);
            ui.canvas
                .rect(14.0, y - 2.0 * s, tw + 4.0 * s, line, ui::PLATE);
            ui.label(16.0, y, 0.0, *ink, text);
        }
        if !self.open {
            return None;
        }
        // The line: a field without its label row.
        let label = ui.line();
        let field = Rect::new(
            16.0,
            bottom - field_h - label + 2.0 * s,
            width,
            field_h + label - 4.0 * s,
        );
        ui.canvas.rect(
            field.x - 2.0,
            field.y + label - 2.0 * s,
            field.w + 4.0,
            field.h - label + 4.0 * s,
            ui::PLATE,
        );
        ui.focus("field", "");
        if std::mem::take(&mut self.written) {
            ui.caret_to_end("");
        }
        ui.text_field(
            field,
            "",
            &mut self.line,
            Field::text(gm_net::control::MAX_CHAT_CHARS),
        );
        // `/r ` becomes `/w NAME ` as it is typed: whom the answer goes to is on the
        // screen before it is sent, whatever whisper arrives meanwhile.
        if self.line == "/r "
            && let Some(to) = &self.reply_to
        {
            self.line = format!("/w {} ", to.replace(' ', ""));
            self.written = true;
        }
        if ui.key(Key::Escape) {
            self.drop_line();
            return None;
        }
        if ui.key(Key::Enter) {
            self.open = false;
            let said = std::mem::take(&mut self.line);
            return match said.trim().strip_prefix('/') {
                Some(rest) => self.command(rest, ignored),
                None => gm_net::control::valid_chat(&said).map(Said::Say),
            };
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::net::SocketAddr;

    use super::*;
    use crate::ui::tests::Recorder;
    use crate::ui::{UiInput, UiState};

    struct Zones(RefCell<Vec<PlayerRequest>>);

    impl HubApi for Zones {
        fn call(&self, req: PlayerRequest) -> Pending<Answer> {
            self.0.borrow_mut().push(req);
            let zone = |id: &str, map: &str, players| ZoneSummary {
                id: id.into(),
                map: map.into(),
                players,
                max_players: 64,
                web: true,
                min_trust: 0,
                requires: Vec::new(),
                addr: "127.0.0.1:1".parse::<SocketAddr>().unwrap(),
                cert_hash: 0,
                up_secs: 60,
            };
            Pending::ready(Ok(PlayerResponse::Zones(vec![
                zone("arena", "arena", 7),
                zone("town", "town", 3),
            ])))
        }
    }

    const S: SessionId = SessionId([9; 16]);
    const ALL: Offers = Offers {
        inventory: true,
        people: true,
        travel: true,
        leave: true,
        fullscreen: true,
        quit: true,
    };

    fn frame(
        menu: &mut GameMenu,
        state: &mut UiState,
        input: &UiInput,
        hub: Option<&dyn HubApi>,
        offers: Offers,
        settings: &mut Settings,
    ) -> MenuAction {
        let mut canvas = Recorder::new(1280.0, 720.0);
        let mut ui = Ui::begin(&mut canvas, state, input, menu.page.name(), 240.0);
        let action = menu.frame(&mut ui, hub.map(|h| (h, S)), offers, "town", settings);
        ui.end();
        action
    }

    fn click(
        menu: &mut GameMenu,
        state: &mut UiState,
        text: &str,
        hub: Option<&dyn HubApi>,
        offers: Offers,
        settings: &mut Settings,
    ) -> MenuAction {
        // The screen as it stands now (the last frame may have ended early on an action).
        frame(menu, state, &UiInput::default(), hub, offers, settings);
        let at = state
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
        frame(menu, state, &press, hub, offers, settings);
        let release = UiInput {
            cursor: at,
            released: true,
            ..Default::default()
        };
        frame(menu, state, &release, hub, offers, settings)
    }

    #[test]
    fn the_menu_resumes_travels_leaves_and_quits() {
        let hub = Zones(RefCell::new(Vec::new()));
        let (mut menu, mut st, mut set) =
            (GameMenu::default(), UiState::default(), Settings::default());
        let idle = UiInput::default();
        frame(&mut menu, &mut st, &idle, Some(&hub), ALL, &mut set);
        assert!(st.shows("The world goes on while this is open."));
        assert_eq!(
            click(&mut menu, &mut st, "Resume", Some(&hub), ALL, &mut set),
            MenuAction::Resume
        );
        assert_eq!(
            click(&mut menu, &mut st, "Inventory", Some(&hub), ALL, &mut set),
            MenuAction::Inventory
        );
        assert_eq!(
            click(&mut menu, &mut st, "Leave", Some(&hub), ALL, &mut set),
            MenuAction::Leave
        );
        assert_eq!(
            click(&mut menu, &mut st, "Quit", Some(&hub), ALL, &mut set),
            MenuAction::Quit
        );
        // Escape is the way back out of the menu.
        let escape = UiInput {
            keys: vec![Key::Escape],
            ..Default::default()
        };
        assert_eq!(
            frame(&mut menu, &mut st, &escape, Some(&hub), ALL, &mut set),
            MenuAction::Resume
        );

        // Travel asks the hub which zones are up; the one being played cannot be gone to.
        click(&mut menu, &mut st, "Travel", Some(&hub), ALL, &mut set);
        assert_eq!(menu.page, Page::Travel);
        assert!(matches!(
            hub.0.borrow()[..],
            [PlayerRequest::ListZones { session: S }]
        ));
        frame(&mut menu, &mut st, &idle, Some(&hub), ALL, &mut set);
        assert!(st.shows("arena  arena  7 playing") && st.shows("town  town  here"));
        assert_eq!(
            click(&mut menu, &mut st, "Go", Some(&hub), ALL, &mut set),
            MenuAction::Travel("arena".into()),
            "the first zone that is not this one is picked"
        );
        click(&mut menu, &mut st, "town  town", Some(&hub), ALL, &mut set);
        assert!(st.find("Go").is_none(), "here is not somewhere to go");
        assert_eq!(
            frame(&mut menu, &mut st, &escape, Some(&hub), ALL, &mut set),
            MenuAction::None
        );
        assert_eq!(menu.page, Page::Main);
    }

    #[test]
    fn a_double_click_that_opened_the_travel_page_does_not_travel() {
        let hub = Zones(RefCell::new(Vec::new()));
        let (mut menu, mut st, mut set) =
            (GameMenu::default(), UiState::default(), Settings::default());
        let idle = UiInput::default();
        frame(&mut menu, &mut st, &idle, Some(&hub), ALL, &mut set);
        // Two clicks in quick succession on the Travel button: the first opens the page,
        // and whatever is under the pointer by then gets the second. Where the button
        // was, or on any row of the list that came up (a row is activated by two presses
        // on that row, not by one on a button and one on the row): nothing travels.
        let at = st.find("Travel").unwrap().rect.centre();
        click(&mut menu, &mut st, "Travel", Some(&hub), ALL, &mut set);
        assert_eq!(menu.page, Page::Travel);
        frame(&mut menu, &mut st, &idle, Some(&hub), ALL, &mut set);
        let rows: Vec<(f32, f32)> = st
            .seen
            .iter()
            .filter(|s| s.kind == crate::ui::SeenKind::Row)
            .map(|s| s.rect.centre())
            .collect();
        assert!(!rows.is_empty(), "the page came up with its list");
        for at in std::iter::once(at).chain(rows.into_iter().take(1)) {
            let second = UiInput {
                cursor: at,
                pressed: true,
                down: true,
                double: true,
                ..Default::default()
            };
            assert_eq!(
                frame(&mut menu, &mut st, &second, Some(&hub), ALL, &mut set),
                MenuAction::None
            );
            assert_eq!(menu.page, Page::Travel);
        }
        // With the keyboard on Back, Enter goes back and nowhere else.
        let tab = UiInput {
            keys: vec![Key::Tab],
            ..Default::default()
        };
        for _ in 0..6 {
            if st.focus() == Some("button:Back") {
                break;
            }
            frame(&mut menu, &mut st, &tab, Some(&hub), ALL, &mut set);
        }
        assert_eq!(st.focus(), Some("button:Back"));
        let enter = UiInput {
            keys: vec![Key::Enter],
            ..Default::default()
        };
        assert_eq!(
            frame(&mut menu, &mut st, &enter, Some(&hub), ALL, &mut set),
            MenuAction::None
        );
        assert_eq!(menu.page, Page::Main);
    }

    #[test]
    fn a_client_that_joined_a_zone_directly_is_offered_less() {
        let (mut menu, mut st, mut set) =
            (GameMenu::default(), UiState::default(), Settings::default());
        let none = Offers {
            inventory: false,
            people: false,
            travel: false,
            leave: false,
            fullscreen: false,
            quit: true,
        };
        frame(
            &mut menu,
            &mut st,
            &UiInput::default(),
            None,
            none,
            &mut set,
        );
        assert!(st.find("Travel").is_none() && st.find("Leave").is_none());
        assert!(st.find("Inventory").is_none(), "no hub: nothing to ask");
        assert!(st.find("Resume").is_some() && st.find("Quit").is_some());
        // The settings change what they say they change, at once.
        click(&mut menu, &mut st, "Settings", None, none, &mut set);
        assert_eq!(menu.page, Page::Settings);
        frame(
            &mut menu,
            &mut st,
            &UiInput::default(),
            None,
            none,
            &mut set,
        );
        assert!(!st.shows("fullscreen"));
        let a = click(
            &mut menu,
            &mut st,
            "invert the mouse's up and down",
            None,
            none,
            &mut set,
        );
        assert_eq!(a, MenuAction::Changed);
        assert!(set.invert);
        // The size of text is one of five, and stays what was clicked.
        let a = click(&mut menu, &mut st, "size of text 3", None, none, &mut set);
        assert_eq!((a, set.ui_scale), (MenuAction::Changed, 3));
        let a = click(
            &mut menu,
            &mut st,
            "size of text by the window",
            None,
            none,
            &mut set,
        );
        assert_eq!((a, set.ui_scale), (MenuAction::Changed, 0));
        click(&mut menu, &mut st, "Back", None, none, &mut set);
        assert_eq!(menu.page, Page::Main);
    }

    #[test]
    fn every_page_is_whole_at_every_size() {
        let hub = Zones(RefCell::new(Vec::new()));
        for size in crate::ui::tests::SIZES {
            let (mut menu, mut st, mut set) =
                (GameMenu::default(), UiState::default(), Settings::default());
            for page in [Page::Main, Page::Travel, Page::Settings, Page::Keys] {
                menu.page = page;
                if page == Page::Travel {
                    menu.wait = Some(hub.call(PlayerRequest::ListZones { session: S }));
                }
                // Twice: the first frame of the travel page takes the hub's answer.
                for _ in 0..2 {
                    let mut canvas = Recorder::new(size.0, size.1);
                    let input = UiInput::default();
                    let mut ui = Ui::begin(&mut canvas, &mut st, &input, page.name(), 300.0);
                    menu.frame(&mut ui, Some((&hub, S)), ALL, "town", &mut set);
                    ui.end();
                    crate::ui::tests::tidy(&canvas, &st, 300.0);
                    // Nothing a page says is cut short (two lines of the keys page were,
                    // at every size, and no test looked).
                    assert!(
                        st.clipped.is_empty(),
                        "{} at {size:?}: {:?}",
                        page.name(),
                        st.clipped
                    );
                }
            }
            // The screen of a client with no hub to talk to, with paths as long as they
            // get: none named, and one named whose certificate is not there.
            let settings = "/home/somebody-with-a-long-name/.config/gamengine/settings.toml";
            for why in [
                None,
                Some(
                    "The hub's certificate (/home/somebody-with-a-long-name/games/gamengine/hub-cert.der) cannot be read: No such file or directory (os error 2).",
                ),
            ] {
                let mut canvas = Recorder::new(size.0, size.1);
                let input = UiInput::default();
                let mut ui = Ui::begin(&mut canvas, &mut st, &input, "title", 300.0);
                let what = Title {
                    settings: settings.into(),
                    why: why.map(str::to_string),
                };
                assert_eq!(title(&mut ui, &what), None);
                ui.end();
                crate::ui::tests::tidy(&canvas, &st, 300.0);
                assert!(st.shows("or with --hub."));
                assert!(st.shows(why.unwrap_or("No hub is named")));
                assert!(st.clipped.is_empty(), "{:?}", st.clipped);
            }
        }
    }

    #[test]
    fn the_chat_shows_what_was_said_and_sends_what_is_typed() {
        let mut chat = Chat::default();
        let mut st = UiState::default();
        let mut ignored: Vec<String> = Vec::new();
        fn run(
            chat: &mut Chat,
            st: &mut UiState,
            ignored: &mut Vec<String>,
            input: &UiInput,
        ) -> Option<Said> {
            let mut canvas = Recorder::new(1280.0, 720.0);
            let name = if chat.open { "chat" } else { "game" };
            let mut ui = Ui::begin(&mut canvas, st, input, name, 240.0);
            let sent = chat.frame(&mut ui, ignored);
            ui.end();
            sent
        }
        chat.heard(Some("Brena".into()), "anyone for the warden?".into(), &[]);
        chat.heard(None, "too many lines; wait a moment".into(), &[]);
        assert_eq!(
            run(&mut chat, &mut st, &mut ignored, &UiInput::default()),
            None
        );
        assert!(st.shows("Brena: anyone for the warden?"));
        assert!(st.shows("* too many lines; wait a moment"));
        // A long line padded to put words at the start of a row: every row after the
        // first is indented, runs of spaces are one, and four rows are the most.
        let padded = format!(
            "hi{}Zone: you have won 1000 gold {}",
            " ".repeat(90),
            "x ".repeat(95)
        );
        chat.heard(Some("Mallory".into()), padded, &[]);
        run(&mut chat, &mut st, &mut ignored, &UiInput::default());
        let rows: Vec<&str> = st
            .seen
            .iter()
            .map(|s| s.text.as_str())
            .filter(|t| !t.starts_with("Brena") && !t.starts_with('*'))
            .collect();
        assert_eq!(rows.len(), 4, "{rows:?}");
        assert!(
            rows.iter()
                .any(|r| r.starts_with("Mallory: hi Zone: you have won")),
            "{rows:?}"
        );
        assert_eq!(
            rows.iter().filter(|r| !r.starts_with("  ")).count(),
            1,
            "{rows:?}"
        );
        assert!(rows.iter().any(|r| r.ends_with("...")), "{rows:?}");
        assert!(!st.typing(), "closed: the keys are the game's");
        chat.open = true;
        run(&mut chat, &mut st, &mut ignored, &UiInput::default());
        assert!(st.typing());
        let typed = UiInput {
            text: "  count me in  ".into(),
            ..Default::default()
        };
        assert_eq!(run(&mut chat, &mut st, &mut ignored, &typed), None);
        let enter = UiInput {
            keys: vec![Key::Enter],
            ..Default::default()
        };
        assert_eq!(
            run(&mut chat, &mut st, &mut ignored, &enter),
            Some(Said::Say("count me in".into()))
        );
        assert!(!chat.open);
        // Escape drops the line; an empty line sends nothing.
        chat.open = true;
        run(&mut chat, &mut st, &mut ignored, &typed);
        let escape = UiInput {
            keys: vec![Key::Escape],
            ..Default::default()
        };
        assert_eq!(run(&mut chat, &mut st, &mut ignored, &escape), None);
        chat.open = true;
        assert_eq!(
            run(&mut chat, &mut st, &mut ignored, &enter),
            None,
            "nothing typed, nothing sent"
        );
        // A line that begins with a slash is the client's own: it is not sent.
        for (line, says) in [
            ("/ignore Mallory", "* not hearing Mallory any more"),
            ("/ignore", "* ignored: Mallory"),
            ("/dance", "* /p TEXT, /w NAME TEXT, /r TEXT"),
            ("/r hello", "* nobody has whispered to you"),
            ("/w x hello", "* nobody is called x"),
            ("/w Brena", "* a whisper is /w NAME and what to say"),
            ("/invite 9", "* nobody is called 9"),
        ] {
            chat.open = true;
            let typed = UiInput {
                text: line.into(),
                ..Default::default()
            };
            run(&mut chat, &mut st, &mut ignored, &typed);
            assert_eq!(run(&mut chat, &mut st, &mut ignored, &enter), None);
            run(&mut chat, &mut st, &mut ignored, &UiInput::default());
            assert!(st.shows(says), "{says:?} not shown");
        }
        assert_eq!(ignored, ["Mallory"]);
        // Somebody ignored is not heard, however the name is dressed up.
        chat.heard(Some("MaIIory".into()), "buy gold".into(), &ignored);
        chat.heard(Some("Brena".into()), "still here".into(), &ignored);
        let said: Vec<&str> = chat.log.iter().map(|l| l.3.as_str()).collect();
        assert!(said.contains(&"still here") && !said.contains(&"buy gold"));
        // --- Channels (PARTY.md 5). What is typed after a command goes where it says.
        for (line, sent) in [
            ("/p  ready? ", Said::Party("ready?".into())),
            (
                "/w Brena are you there",
                Said::Whisper {
                    to: "Brena".into(),
                    text: "are you there".into(),
                },
            ),
            ("/invite De Vil", Said::Invite("De Vil".into())),
            ("/leave", Said::Leave),
        ] {
            chat.open = true;
            let typed = UiInput {
                text: line.into(),
                ..Default::default()
            };
            run(&mut chat, &mut st, &mut ignored, &typed);
            assert_eq!(
                run(&mut chat, &mut st, &mut ignored, &enter),
                Some(sent),
                "{line}"
            );
        }
        // A party's line, a whisper and one's own whisper are each marked with what no
        // name can begin with: somebody called "Brena whispers" says nothing that looks
        // like a whisper of Brena's.
        use gm_net::control::{CHANNEL_PARTY, CHANNEL_WHISPER, CHANNEL_WHISPERED};
        chat.heard_on(CHANNEL_PARTY, "Brena".into(), "pull".into(), &ignored);
        chat.heard_on(CHANNEL_WHISPER, "De Vil".into(), "psst".into(), &ignored);
        chat.heard_on(CHANNEL_WHISPERED, "Brena".into(), "soon".into(), &ignored);
        chat.heard_on(
            CHANNEL_WHISPER,
            "Mallory".into(),
            "buy gold".into(),
            &ignored,
        );
        chat.heard_on(CHANNEL_PARTY, "MaIIory".into(), "buy gold".into(), &ignored);
        chat.heard(Some("Brena whispers".into()), "psst".into(), &ignored);
        chat.open = true;
        run(&mut chat, &mut st, &mut ignored, &UiInput::default());
        for says in [
            "[party] Brena: pull",
            "[whisper] De Vil: psst",
            "[to Brena] soon",
            "Brena whispers: psst",
        ] {
            assert!(st.shows(says), "{says:?} not shown");
        }
        assert!(
            !st.shows("buy gold"),
            "not heard is not heard on any channel"
        );
        // `/r` answers whoever whispered last and is heard; a name with a space in it is
        // written without (names are told apart by their letters).
        let typed = UiInput {
            text: "/r on my way".into(),
            ..Default::default()
        };
        run(&mut chat, &mut st, &mut ignored, &typed);
        assert_eq!(
            run(&mut chat, &mut st, &mut ignored, &enter),
            Some(Said::Whisper {
                to: "DeVil".into(),
                text: "on my way".into()
            })
        );
        // Typed a key at a time, `/r ` becomes `/w DeVil ` on the screen as soon as it is
        // complete: a whisper that arrives meanwhile does not turn the answer elsewhere.
        chat.open_with(String::new());
        for c in ["/", "r", " "] {
            let typed = UiInput {
                text: c.into(),
                ..Default::default()
            };
            run(&mut chat, &mut st, &mut ignored, &typed);
        }
        assert_eq!(chat.line, "/w DeVil ");
        chat.heard_on(2, "Mallory".into(), "hey".into(), &ignored);
        let typed = UiInput {
            text: "no".into(),
            ..Default::default()
        };
        run(&mut chat, &mut st, &mut ignored, &typed);
        assert_eq!(
            run(&mut chat, &mut st, &mut ignored, &enter),
            Some(Said::Whisper {
                to: "DeVil".into(),
                text: "no".into()
            })
        );
        // Whoever whispered last is this character's business, not the next one's.
        chat.clear();
        chat.open_with("/r ".into());
        run(&mut chat, &mut st, &mut ignored, &UiInput::default());
        assert_eq!(chat.line, "/r ");
        // A page opens the line with a whisper begun.
        chat.open_with("/w Brena ".into());
        let typed = UiInput {
            text: "hello".into(),
            ..Default::default()
        };
        run(&mut chat, &mut st, &mut ignored, &typed);
        assert_eq!(
            run(&mut chat, &mut st, &mut ignored, &enter),
            Some(Said::Whisper {
                to: "Brena".into(),
                text: "hello".into()
            })
        );
    }
}
