//! People together (PARTY.md 8): who is here and who is of the party, a trade between two,
//! and the tavern. What a party is, who asked what, and the trade itself are the hub's and
//! the zone's word: these screens work nothing out. Asking somebody into the party,
//! answering, leaving and asking for a trade are said to the zone; the trade's window and
//! the tavern are asked of the hub.
//!
//! The rules of the inventory's screens hold here (ITEMS.md 6): what is picked is picked
//! by what it is, and nothing takes a vanished thing's place; nothing is said that was not
//! said to this client. One list moves by itself, the other's side of a trade: it is the
//! only one that must, and everything on it that the player has not agreed to is marked
//! until they agree.

use gm_hub_proto::player::{
    HireRow, PlayerEcon, PlayerEconReply, PlayerRequest, PlayerResponse, TavernRow,
};
use gm_hub_proto::protocol::{
    CharacterId, HubError, ItemSummary, PLACE_NONE, SessionId, TRADE_CANCELLED, TRADE_COMMITTED,
    TRADE_OPEN, TradeOffer,
};
use gm_net::control::FromClient;
use web_time::{Duration, Instant};

use crate::bag::{coin_fields, coin_parts, coin_row, copper_of, headline, keep, name};
use crate::font::ADVANCE;
use crate::front::PANEL_UNITS;
use crate::hub::{Answer, HubApi, Pending, RpcError};
use crate::ui::{self, Canvas, Column, Key, NONE, Rect, RowMark, Ui};

/// The zone takes one request about the party from a player in a second.
const ZONE_GAP: Duration = Duration::from_millis(1100);
/// An invitation waits a minute, a request to trade half of one (PARTY.md 11).
const INVITE_LIFE: Duration = Duration::from_secs(60);
const TRADE_ASK_LIFE: Duration = Duration::from_secs(gm_net::control::TRADE_ASK_SECS);
/// The trade window asks for the trade this often, and a view older than `STALE` arms
/// nothing; an offer must have been on this screen for `LOOK` before it can be accepted.
const POLL: Duration = Duration::from_secs(1);
const STALE: Duration = Duration::from_secs(2);
const LOOK: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    People,
    Trade,
    Tavern,
}

impl Page {
    /// The name a UI script waits for (CLIENT.md 9).
    pub fn name(self) -> &'static str {
        match self {
            Page::People => "people",
            Page::Trade => "trade",
            Page::Tavern => "tavern",
        }
    }
}

/// What the app must do after a frame of these screens.
#[derive(Clone, Debug, PartialEq)]
pub enum PeopleAction {
    None,
    Close,
    /// Said to the zone (PARTY.md 4).
    Zone(FromClient),
    /// Open the chat line at a whisper to this name.
    Whisper(String),
}

/// Somebody asked something of this player.
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub from: String,
    /// A request to trade, by this body; otherwise an invitation to a party.
    pub trade: Option<u32>,
    pub at: Instant,
}

/// What the zone has said of other people, kept while a zone is played.
#[derive(Default)]
pub struct Social {
    /// The party as the hub has it: its members by name, the leader first; nobody: none.
    pub party: Vec<String>,
    pub asks: Vec<Ask>,
    /// When the zone was last asked something about the party or a trade: it takes one a
    /// second, whichever page (or the chat line) the asking came from.
    pub asked: Option<Instant>,
}

/// Names are told apart as the hub tells them apart.
fn same(a: &str, b: &str) -> bool {
    gm_hub_proto::names::skeleton(a) == gm_hub_proto::names::skeleton(b)
}

impl Social {
    /// `true`: it is new (the same one asked again is the one that waits already).
    fn asked(&mut self, from: String, trade: Option<u32>, now: Instant) -> bool {
        self.forget(now);
        let known = self
            .asks
            .iter_mut()
            .find(|a| same(&a.from, &from) && a.trade.is_some() == trade.is_some());
        match known {
            Some(ask) => {
                // Asked again: the hub's new one begins its time anew.
                ask.trade = trade;
                ask.at = now;
                false
            }
            None => {
                self.asks.push(Ask {
                    from,
                    trade,
                    at: now,
                });
                true
            }
        }
    }

    pub fn invited(&mut self, from: String, now: Instant) -> bool {
        self.asked(from, None, now)
    }

    pub fn trade_asked(&mut self, body: u32, from: String, now: Instant) -> bool {
        self.asked(from, Some(body), now)
    }

    /// What nobody answered in its time is gone.
    pub fn forget(&mut self, now: Instant) {
        self.asks.retain(|a| {
            let life = if a.trade.is_some() {
                TRADE_ASK_LIFE
            } else {
                INVITE_LIFE
            };
            now.saturating_duration_since(a.at) < life
        });
    }

    fn answered(&mut self, from: &str, trade: bool) {
        self.asks
            .retain(|a| !(same(&a.from, from) && a.trade.is_some() == trade));
    }

    fn in_party(&self, who: &str) -> bool {
        self.party.iter().any(|m| same(m, who))
    }

    fn leads(&self, who: &str) -> bool {
        self.party.first().is_some_and(|l| same(l, who))
    }
}

/// Somebody whose body is in this zone.
#[derive(Clone, Debug, PartialEq)]
pub struct Here {
    pub body: u32,
    pub name: String,
    /// Within reach for a trade (the rule the zone decides with).
    pub near: bool,
}

/// What the hub was asked.
#[derive(Clone, Debug, PartialEq)]
enum What {
    View,
    Inventory,
    /// A change of the trade; what to say when it is done.
    Change(&'static str),
    /// An accept of this version, and the other's offer as it was shown then.
    Accept(TradeOffer),
    Tavern,
    Hires,
    Listed,
    /// A hire, a listing, a dismissal; what to say when it is done.
    Tavernly(&'static str),
}

/// A trade as the hub last showed it.
#[derive(Clone, Debug, PartialEq)]
struct Shown {
    state: u8,
    version: i32,
    wait_ms: u32,
    together: bool,
    mine: TradeOffer,
    theirs: TradeOffer,
}

struct Trade {
    id: i64,
    with: String,
    shown: Option<Shown>,
    /// When the last view came, and when one was last asked for.
    seen_at: Option<Instant>,
    asked_at: Option<Instant>,
    /// The version on the screen, and since when.
    version_since: Option<(i32, Instant)>,
    /// The other's offer as this player last accepted it: nothing, until they have.
    agreed: TradeOffer,
    /// This player has accepted at least once: before that everything the other offers
    /// is new, and "changed" is no word for it.
    agreed_once: bool,
    /// The other's offer as it was when Accept last armed (looked at for three seconds):
    /// what differs from it is "changed", not merely "new", and the lines say so.
    looked: Option<(i32, TradeOffer)>,
    /// The hub's last word is about a change this player made, whose version is still to
    /// be seen: that version does not clear it (the one after it does).
    own_word: bool,
    inventory: Option<Vec<ItemSummary>>,
    /// Picked in: this player's offer, the other's, the inventory.
    picked: [usize; 3],
    /// The list a row was last picked in: what the line under the lists tells of.
    told: usize,
    coin: [String; 3],
    /// How it ended, when it has.
    over: Option<&'static str>,
}

#[derive(Default)]
struct Tavern {
    list: Option<Vec<TavernRow>>,
    hires: Option<Vec<HireRow>>,
    /// The price this character is listed at (`Some(0)`: it is not).
    listed: Option<i64>,
    picked: usize,
    hired: usize,
    price: [String; 3],
}

pub struct People {
    pub page: Page,
    session: SessionId,
    character: CharacterId,
    /// The rows of the people page, in the places they keep while it is up.
    rows: Vec<String>,
    picked: usize,
    trade: Option<Trade>,
    tavern: Tavern,
    asks: Vec<(What, Pending<Answer>)>,
    notice: String,
    bad: bool,
    now: Instant,
}

fn nothing_offered() -> TradeOffer {
    TradeOffer {
        coin: 0,
        accepted: false,
        items: Vec::new(),
    }
}

/// A refusal in a person's words.
fn words(e: &RpcError) -> String {
    match e {
        RpcError::Refused(HubError::Invalid(why)) => why.clone(),
        RpcError::Refused(HubError::Cooldown) => {
            "the offer has just changed: look at it first".to_string()
        }
        RpcError::Refused(HubError::Insufficient) => "not enough coin".to_string(),
        RpcError::Refused(HubError::Full) => "there is no room for it".to_string(),
        RpcError::Refused(HubError::Busy) => "the hub is busy: try again".to_string(),
        RpcError::Refused(HubError::NotFound) => "it is not there any more".to_string(),
        RpcError::Refused(HubError::Unauthorized) => {
            "that is not yours to do from here".to_string()
        }
        other => other.to_string(),
    }
}

/// How long until a unix time, in words.
fn until(ends_at: u64, unix: u64) -> String {
    let secs = ends_at.saturating_sub(unix);
    match secs {
        0 => "ending".to_string(),
        s if s < 3600 => format!("{} min", (s / 60).max(1)),
        s => format!("{} h {} min", s / 3600, s % 3600 / 60),
    }
}

impl People {
    fn new(page: Page, session: SessionId, character: CharacterId, now: Instant) -> People {
        People {
            page,
            session,
            character,
            rows: Vec::new(),
            picked: NONE,
            trade: None,
            tavern: Tavern::default(),
            asks: Vec::new(),
            notice: String::new(),
            bad: false,
            now,
        }
    }

    /// The people of this zone and of the party.
    pub fn here(session: SessionId, character: CharacterId, now: Instant) -> People {
        People::new(Page::People, session, character, now)
    }

    /// Whether this is the window of that trade.
    pub fn trading(&self, trade: i64) -> bool {
        self.page == Page::Trade && self.trade.as_ref().is_some_and(|t| t.id == trade)
    }

    /// The window of a trade the zone said the hub opened.
    pub fn trade(
        hub: &dyn HubApi,
        session: SessionId,
        character: CharacterId,
        trade: i64,
        with: String,
        now: Instant,
    ) -> People {
        let mut p = People::new(Page::Trade, session, character, now);
        p.trade = Some(Trade {
            id: trade,
            with,
            shown: None,
            seen_at: None,
            asked_at: None,
            version_since: None,
            agreed: nothing_offered(),
            agreed_once: false,
            looked: None,
            own_word: false,
            inventory: None,
            picked: [NONE; 3],
            told: 2,
            coin: Default::default(),
            over: None,
        });
        p.ask(hub, What::Inventory, PlayerEcon::Inventory);
        p
    }

    fn ask(&mut self, hub: &dyn HubApi, what: What, op: PlayerEcon) {
        let req = PlayerRequest::Econ {
            session: self.session,
            character: self.character,
            op,
        };
        self.asks.push((what, hub.call(req)));
    }

    fn say(&mut self, text: impl Into<String>, bad: bool) {
        self.notice = text.into();
        self.bad = bad;
    }

    /// Something other than a look at the trade is being waited for.
    fn busy(&self) -> bool {
        self.asks.iter().any(|(what, _)| *what != What::View)
    }

    /// One frame. `ui` was begun for `self.page.name()`. `hub`: there is one to ask.
    /// `me`: the player's own name. `here`: the other people of the zone. `word`: what the
    /// zone last said to this player, a moment ago (its answer to this page's buttons).
    /// `unix`: the wall clock, in seconds.
    #[allow(clippy::too_many_arguments)]
    pub fn frame<C: Canvas>(
        &mut self,
        ui: &mut Ui<'_, C>,
        hub: Option<&dyn HubApi>,
        me: &str,
        social: &mut Social,
        here: &[Here],
        word: Option<&str>,
        now: Instant,
        unix: u64,
    ) -> PeopleAction {
        self.now = now;
        social.forget(now);
        let action = match (self.page, hub) {
            (Page::People, _) => self.people_page(ui, hub, me, social, here, word),
            (Page::Trade, Some(hub)) => self.trade_page(ui, hub),
            (Page::Tavern, Some(hub)) => self.tavern_page(ui, hub, unix),
            (_, None) => PeopleAction::Close,
        };
        // Answers are taken when the frame has been drawn: a page changes between two
        // frames, never inside one.
        if let Some(hub) = hub {
            self.answers(hub);
        }
        action
    }

    /// The two lines under the lists: what is being waited for, or the last thing said.
    fn notice_lines<C: Canvas>(&self, ui: &mut Ui<'_, C>, r: Rect) {
        let (text, color) = if self.busy() {
            ("asking the hub", ui::FAINT)
        } else if self.bad {
            (self.notice.as_str(), ui::WARN)
        } else {
            (self.notice.as_str(), ui::TEXT)
        };
        if !text.is_empty() {
            ui.paragraph(r, color, text);
        }
    }

    // ---------- the people of the zone and of the party ----------

    fn people_page<C: Canvas>(
        &mut self,
        ui: &mut Ui<'_, C>,
        hub: Option<&dyn HubApi>,
        me: &str,
        social: &mut Social,
        here: &[Here],
        word: Option<&str>,
    ) -> PeopleAction {
        // The rows keep their places while the page is up: whoever is new is added at the
        // end, and the row of whoever left stays. Nothing slides under the pointer.
        let mut wanted: Vec<&str> = social
            .party
            .iter()
            .map(String::as_str)
            .filter(|m| !same(m, me))
            .collect();
        wanted.extend(social.asks.iter().map(|a| a.from.as_str()));
        let mut others: Vec<&str> = here.iter().map(|h| h.name.as_str()).collect();
        others.sort_unstable();
        wanted.extend(others);
        for who in wanted {
            if !self.rows.iter().any(|r| r == who) {
                self.rows.push(who.to_string());
            }
        }
        let body = |who: &str| here.iter().find(|h| h.name == who);
        let asks = |who: &str, trade: bool| {
            social
                .asks
                .iter()
                .find(|a| a.from == who && a.trade.is_some() == trade)
        };
        let rows: Vec<Vec<String>> = self
            .rows
            .iter()
            .map(|who| {
                let mut notes: Vec<&str> = Vec::new();
                if social.leads(who) {
                    notes.push("leads");
                } else if social.in_party(who) {
                    notes.push("party");
                }
                if asks(who, false).is_some() {
                    notes.push("invites you");
                }
                if asks(who, true).is_some() {
                    notes.push("asks to trade");
                }
                if body(who).is_none() {
                    // Of the party and not here; asking from another zone; or was here
                    // while this page has been up, and left.
                    notes.push(if social.in_party(who) {
                        "away"
                    } else if asks(who, false).is_some() {
                        "elsewhere"
                    } else {
                        "gone"
                    });
                }
                vec![who.clone(), notes.join(", ")]
            })
            .collect();

        let s = ui.scale;
        let gap = 5.0 * s;
        let line = ui.line();
        let h = ui.button_height();
        let list = ui.list_height(10);
        let inner = line + gap + list + gap + 2.0 * line + gap + h + gap + h;
        let panel = Rect::centred(ui.size(), PANEL_UNITS * s, ui.panel_height(inner, true));
        let inner = ui.panel(panel, "people");
        let mut col = Column::new(inner, gap);
        let top = col.take(line);
        let party = if social.party.is_empty() {
            "you are in no party".to_string()
        } else {
            format!("your party: {}", social.party.join(", "))
        };
        ui.label(top.x, top.y, top.w, ui::FAINT, &party);
        ui.focus_default("list", "people");
        ui.list(
            col.take(list),
            "people",
            &[0.0, 0.36],
            &rows,
            &mut self.picked,
        );
        // What the zone answered to the last button is said here too: the page may lie
        // over the lines of the chat.
        let said = col.take(2.0 * line);
        match word {
            Some(word) if self.notice.is_empty() => {
                ui.paragraph(said, ui::TEXT, word);
            }
            _ => self.notice_lines(ui, said),
        }

        let picked = self.rows.get(self.picked).cloned();
        let who = picked.as_deref();
        let in_party = !social.party.is_empty();
        let i_lead = social.leads(me);
        // The zone takes one of these a second: a button is off for that long.
        let free = social
            .asked
            .is_none_or(|at| self.now.saturating_duration_since(at) > ZONE_GAP);
        let present = who.and_then(body);
        let member = who.is_some_and(|w| social.in_party(w));
        let invites = who.is_some_and(|w| asks(w, false).is_some());
        let may_invite = present.is_some() && !member && (!in_party || i_lead);

        let first = ui.buttons(
            col.take(h),
            &["Invite", "Join", "Decline", "Remove", "Leave"],
        );
        let second = ui.buttons(col.take(h), &["Trade", "Whisper", "Tavern", "Close"]);
        let mut say: Option<FromClient> = None;
        if ui.button_if(first[0], "Invite", free && may_invite)
            && let Some(who) = who
        {
            say = Some(FromClient::PartyInvite {
                name: who.to_string(),
            });
        }
        for (at, label, join) in [(first[1], "Join", true), (first[2], "Decline", false)] {
            if ui.button_if(at, label, free && invites)
                && let Some(who) = who
            {
                say = Some(FromClient::PartyAnswer {
                    from: who.to_string(),
                    join,
                });
                // (A join is answered by the party itself, which takes every
                // invitation with it; one the zone refused is still there to answer.)
                if !join {
                    social.answered(who, false);
                }
            }
        }
        if ui.button_if(first[3], "Remove", free && member && i_lead)
            && let Some(who) = who
        {
            say = Some(FromClient::PartyRemove {
                name: who.to_string(),
            });
        }
        if ui.button_if(first[4], "Leave", free && in_party) {
            say = Some(FromClient::PartyLeave);
        }
        // A trade is asked of somebody who stands near; asking back opens it.
        let near = present.filter(|h| h.near);
        if ui.button_if(second[0], "Trade", free && near.is_some() && hub.is_some())
            && let Some(near) = near
        {
            say = Some(FromClient::TradeAsk { with: near.body });
            social.answered(&near.name, true);
        }
        let mut action = PeopleAction::None;
        if ui.button_if(second[1], "Whisper", who.is_some())
            && let Some(who) = who
        {
            // (A name is one word after `/w`: what is between its letters is left out.)
            action = PeopleAction::Whisper(who.replace(' ', ""));
        }
        if ui.button_if(second[2], "Tavern", hub.is_some())
            && let Some(hub) = hub
        {
            self.page = Page::Tavern;
            self.notice.clear();
            self.tavern = Tavern::default();
            self.read_tavern(hub);
        }
        if ui.button(second[3], "Close") || ui.key(Key::Escape) {
            return PeopleAction::Close;
        }
        if let Some(say) = say {
            social.asked = Some(self.now);
            self.notice.clear();
            return PeopleAction::Zone(say);
        }
        action
    }

    // ---------- a trade ----------

    fn trade_page<C: Canvas>(&mut self, ui: &mut Ui<'_, C>, hub: &dyn HubApi) -> PeopleAction {
        let now = self.now;
        let busy = self.busy();
        let Some(t) = &mut self.trade else {
            return PeopleAction::Close;
        };
        // Seen by asking (PARTY.md 6): once a second while the window is up.
        let polling = self.asks.iter().any(|(what, _)| *what == What::View);
        let due = t
            .asked_at
            .is_none_or(|at| now.saturating_duration_since(at) >= POLL);
        let (id, over) = (t.id, t.over);
        if over.is_none() && !polling && due {
            t.asked_at = Some(now);
            self.ask(hub, What::View, PlayerEcon::TradeView { trade: id });
        }
        let Some(t) = &mut self.trade else {
            return PeopleAction::Close;
        };

        let s = ui.scale;
        let gap = 5.0 * s;
        let line = ui.line();
        let h = ui.button_height();
        // The two offers one above the other, each as wide as the panel: a row has room
        // for the longest name, what the thing does and its mark. The other's is the one
        // that is read, and gets the rows.
        let mine_h = ui.list_height(2);
        let theirs_h = ui.list_height(4);
        let carried = ui.list_height(3);
        let inner = line
            + gap
            + mine_h
            + gap
            + line
            + gap
            + theirs_h
            + gap
            + carried
            + gap
            + ui.field_height()
            + gap
            + 2.0 * line
            + gap
            + h;
        let panel = Rect::centred(ui.size(), PANEL_UNITS * s, ui.panel_height(inner, true));
        let title = format!("trade with {}", t.with);
        let inner = ui.panel(panel, &title);
        let mut col = Column::new(inner, gap);

        let (mine, theirs) = match &t.shown {
            Some(shown) => (shown.mine.clone(), shown.theirs.clone()),
            None => (nothing_offered(), nothing_offered()),
        };
        // A header says whose offer it is and whether they have accepted it as it stands.
        let header = |ui: &mut Ui<'_, C>, r: Rect, whose: &str, accepted: bool| {
            ui.label(r.x, r.y, r.w, ui::FAINT, whose);
            if accepted {
                let w = ui.text_width("accepted");
                ui.label(r.x + r.w - w, r.y, w, ui::TEXT, "accepted");
            }
        };
        // What a row says of a thing: its name, what it does and (for gear) how much of
        // it is left; the coin is a row like any other.
        let said_of = |i: &ItemSummary| -> String {
            let left = i.what.rsplit_once(", ").map(|(_, rest)| rest);
            match (i.does.first(), left) {
                (Some(does), Some(left)) if i.place != PLACE_NONE => format!("{does}, {left}"),
                _ => headline(i).to_string(),
            }
        };
        let coin_row_of = |coin: i64| vec!["coin".to_string(), coin_row(coin, 27)];

        header(ui, col.take(line), "you give", mine.accepted);
        let mut mine_rows: Vec<Vec<String>> = mine
            .items
            .iter()
            .map(|i| vec![name(i), said_of(i)])
            .collect();
        mine_rows.push(coin_row_of(mine.coin));
        let before = t.picked;
        ui.list(
            col.take(mine_h),
            "yours",
            &[0.0, 0.24],
            &mine_rows,
            &mut t.picked[0],
        );

        // The other's offer, and what of it this player has not agreed to: what was not
        // there when they last accepted is marked `new`, or `changed` when it was not
        // there either when Accept last armed; what was there and is gone is shown
        // struck, until they accept again. (Said in a word as well as in a colour: a
        // colour alone is not seen by all.)
        let looked = t
            .looked
            .as_ref()
            .filter(|(_, _)| t.shown.is_some())
            .map(|(_, offer)| offer);
        let word_for = |in_agreed: bool, in_looked: Option<bool>| -> (&'static str, RowMark) {
            match (in_agreed, in_looked) {
                (true, _) => ("", RowMark::Plain),
                (false, Some(false)) => ("changed", RowMark::Marked),
                (false, _) => ("new", RowMark::Marked),
            }
        };
        let gives = format!("{} gives", t.with);
        header(ui, col.take(line), &gives, theirs.accepted);
        let mut their_rows: Vec<Vec<String>> = Vec::new();
        let mut marks: Vec<RowMark> = Vec::new();
        for item in &theirs.items {
            let known = t.agreed.items.iter().any(|a| a.id == item.id);
            let seen = looked.map(|l| l.items.iter().any(|a| a.id == item.id));
            let (word, mark) = word_for(known, seen);
            their_rows.push(vec![name(item), said_of(item), word.to_string()]);
            marks.push(mark);
        }
        {
            let known = theirs.coin == t.agreed.coin;
            let seen = looked.map(|l| l.coin == theirs.coin);
            let (word, mark) = word_for(known, seen);
            let mut row = coin_row_of(theirs.coin);
            row.push(word.to_string());
            their_rows.push(row);
            marks.push(mark);
        }
        // Gone since it was agreed to, or since it was looked at: struck.
        let mut gone: Vec<&ItemSummary> = Vec::new();
        for was in t
            .agreed
            .items
            .iter()
            .chain(looked.into_iter().flat_map(|l| l.items.iter()))
        {
            if !theirs.items.iter().any(|i| i.id == was.id) && !gone.iter().any(|g| g.id == was.id)
            {
                gone.push(was);
            }
        }
        for item in &gone {
            their_rows.push(vec![name(item), said_of(item), "taken back".to_string()]);
            marks.push(RowMark::Struck);
        }
        ui.list_marked(
            col.take(theirs_h),
            "theirs",
            &[0.0, 0.24, 0.78],
            &their_rows,
            &marks,
            &mut t.picked[1],
        );

        // What is carried, to offer from.
        let items: Vec<ItemSummary> = t.inventory.clone().unwrap_or_default();
        let carried_rows: Vec<Vec<String>> = items
            .iter()
            .map(|i| {
                let note = if i.worn {
                    "worn"
                } else if mine.items.iter().any(|o| o.id == i.id) {
                    "offered"
                } else {
                    ""
                };
                vec![name(i), said_of(i), note.to_string()]
            })
            .collect();
        ui.focus_default("list", "carried");
        ui.list(
            col.take(carried),
            "carried",
            &[0.0, 0.24, 0.84],
            &carried_rows,
            &mut t.picked[2],
        );
        for (list, was) in before.iter().enumerate() {
            if t.picked[list] != *was && t.picked[list] != NONE {
                t.told = list;
            }
        }
        let picked = t.picked;

        let fields = col.take(ui.field_height());
        // The three fields, and at the end of their row the button that sends them.
        let wide = ui.text_width("Set coin") + 12.0 * s;
        let set = Rect::new(fields.x + fields.w - wide, fields.y + fields.h - h, wide, h);
        let room = Rect::new(fields.x, fields.y, fields.w - wide - gap, fields.h);
        coin_fields(ui, room, gap, &mut t.coin);
        let coin = copper_of(&t.coin);

        // What is said: how it ended; that the other is gone; the hub's last word (about
        // this version); that they changed the offer since it was looked at; how long
        // until it can be accepted.
        let said = col.take(2.0 * line);
        let unagreed = marks.iter().any(|m| *m != RowMark::Plain);
        let changed_since_looked = marks.contains(&RowMark::Struck)
            || their_rows
                .iter()
                .any(|r| r.last().is_some_and(|w| w == "changed"));
        let looked_for = t
            .shown
            .as_ref()
            .zip(t.version_since)
            .filter(|(shown, (version, _))| shown.version == *version)
            .map(|(_, (_, since))| now.saturating_duration_since(since));
        let fresh = t
            .seen_at
            .is_some_and(|at| now.saturating_duration_since(at) < STALE);
        let open = t.shown.as_ref().is_some_and(|v| v.state == TRADE_OPEN) && t.over.is_none();
        let waits = t.shown.as_ref().map_or(0, |v| v.wait_ms);
        let left_to_look = LOOK.saturating_sub(looked_for.unwrap_or_default());
        let wait = Duration::from_millis(waits as u64).max(left_to_look);
        let something =
            !mine.items.is_empty() || !theirs.items.is_empty() || mine.coin > 0 || theirs.coin > 0;
        let with = t.with.clone();
        let away = t.shown.as_ref().is_some_and(|v| !v.together);
        let (over, mine_accepted, version) =
            (t.over, mine.accepted, t.shown.as_ref().map(|v| v.version));
        let agreed_once = t.agreed_once;
        let may = open && !busy;
        let armed = may && fresh && something && wait.is_zero() && !mine_accepted;
        if armed
            && let Some(version) = version
            && t.looked.as_ref().is_none_or(|(v, _)| *v != version)
        {
            // Looked at, for as long as the rule asks: what changes from here is said to
            // have changed.
            t.looked = Some((version, theirs.clone()));
        }
        if let Some(over) = over {
            ui.paragraph(said, ui::TEXT, over);
        } else if away {
            let gone = format!("{with} is not here any more");
            ui.paragraph(said, ui::WARN, &gone);
        } else if busy || !self.notice.is_empty() {
            // (The hub's last word is about this version: a new version clears it.)
            self.notice_lines(ui, said);
        } else if changed_since_looked || (unagreed && agreed_once) {
            let changed = format!("{with} changed the offer: look at what is marked");
            ui.paragraph(said, ui::WARN, &changed);
        } else if open && something && !wait.is_zero() && !mine_accepted {
            let secs = wait.as_secs_f32().ceil() as u32;
            let text = format!("look at the offer: it can be accepted in {secs} s");
            ui.paragraph(said, ui::FAINT, &text);
        } else if mine_accepted {
            let text = format!("you accepted: waiting for {with}");
            ui.paragraph(said, ui::FAINT, &text);
        }

        if over.is_some() {
            let row = ui.buttons(col.take(h), &["Close"]);
            if ui.button(row[0], "Close") || ui.key(Key::Escape) {
                return PeopleAction::Close;
            }
            return PeopleAction::None;
        }
        let row = ui.buttons(col.take(h), &["Offer", "Take back", "Accept", "Cancel"]);
        let loose = items
            .get(picked[2])
            .filter(|i| !i.worn && !mine.items.iter().any(|o| o.id == i.id))
            .map(|i| i.id);
        let offered = mine.items.get(picked[0]).map(|i| i.id);
        let mut op: Option<(What, PlayerEcon)> = None;
        if ui.button_if(row[0], "Offer", may && loose.is_some())
            && let Some(item) = loose
        {
            let change = PlayerEcon::TradeOffer { trade: id, item };
            op = Some((What::Change("offered"), change));
        }
        if ui.button_if(row[1], "Take back", may && offered.is_some())
            && let Some(item) = offered
        {
            let change = PlayerEcon::TradeRetract { trade: id, item };
            op = Some((What::Change("taken back"), change));
        }
        if ui.button_if(set, "Set coin", may && coin.is_some())
            && let Some(coin) = coin
        {
            let change = PlayerEcon::TradeCoin { trade: id, coin };
            op = Some((What::Change("the coin is set"), change));
        }
        if ui.button_if(row[2], "Accept", armed)
            && let Some(version) = version
        {
            let accept = PlayerEcon::TradeAccept { trade: id, version };
            op = Some((What::Accept(theirs.clone()), accept));
        }
        if ui.button(row[3], "Cancel") || ui.key(Key::Escape) {
            // Said and not waited for: the window is gone either way.
            self.ask(hub, What::Change(""), PlayerEcon::TradeCancel { trade: id });
            return PeopleAction::Close;
        }
        if let Some((what, op)) = op {
            self.notice.clear();
            self.ask(hub, what, op);
        }
        PeopleAction::None
    }

    // ---------- the tavern ----------

    fn read_tavern(&mut self, hub: &dyn HubApi) {
        self.ask(hub, What::Tavern, PlayerEcon::Tavern);
        self.ask(hub, What::Hires, PlayerEcon::Hires);
        self.ask(hub, What::Listed, PlayerEcon::HireListed);
    }

    fn tavern_page<C: Canvas>(
        &mut self,
        ui: &mut Ui<'_, C>,
        hub: &dyn HubApi,
        unix: u64,
    ) -> PeopleAction {
        let s = ui.scale;
        let gap = 5.0 * s;
        let line = ui.line();
        let h = ui.button_height();
        let list = ui.list_height(4);
        let hires = ui.list_height(2);
        let inner = 2.0 * line
            + gap
            + list
            + gap
            + 2.0 * line
            + gap
            + hires
            + gap
            + line
            + gap
            + ui.field_height()
            + gap
            + 2.0 * line
            + gap
            + h;
        let panel = Rect::centred(ui.size(), PANEL_UNITS * s, ui.panel_height(inner, true));
        let inner = ui.panel(panel, "tavern");
        let mut col = Column::new(inner, gap);
        // What ECONOMY.md 11 rules, said before anybody pays.
        ui.paragraph(
            col.take(2.0 * line),
            ui::FAINT,
            "A hire is for twelve hours. Its owner may take it back at any time: nothing is refunded.",
        );
        let t = &mut self.tavern;
        let for_hire: Vec<TavernRow> = t.list.clone().unwrap_or_default();
        let rows: Vec<Vec<String>> = for_hire
            .iter()
            .map(|r| {
                let hired = match r.hires {
                    0 => String::new(),
                    n => format!("hired {n}x"),
                };
                vec![r.name.clone(), r.role.clone(), coin_row(r.price, 12), hired]
            })
            .collect();
        ui.focus_default("list", "for hire");
        ui.list(
            col.take(list),
            "for hire",
            &[0.0, 0.38, 0.52, 0.8],
            &rows,
            &mut t.picked,
        );
        let picked = for_hire.get(t.picked).cloned();
        // The picked one in full: what it is in the hub's words, and the whole price a
        // hire is made at.
        let cost = col.take(2.0 * line);
        match (&picked, t.list.is_some(), for_hire.is_empty()) {
            (Some(row), _, _) => {
                let what = format!("{}, {}", row.what, row.role);
                ui.label(cost.x, cost.y, cost.w, ui::TEXT, &what);
                // (The longest name and the dearest price fit the line together; the
                // label is bounded so that a cut would be recorded, not drawn over.)
                let label = format!("{} for", row.name);
                let parts = coin_parts(row.price);
                let price_w: f32 = parts.iter().map(|(t, _)| ui.text_width(t)).sum::<f32>()
                    + (parts.len() as f32) * ADVANCE * s;
                let room = cost.w - price_w - 2.0 * ADVANCE * s;
                ui.label(cost.x, cost.y + line, room.max(1.0), ui::FAINT, &label);
                let x = cost.x + ui.text_width(&label).min(room) + 2.0 * ADVANCE * s;
                ui.spans(x, cost.y + line, &parts);
            }
            (None, true, true) => ui.label(cost.x, cost.y, cost.w, ui::FAINT, "nobody is for hire"),
            (None, true, false) => ui.label(cost.x, cost.y, cost.w, ui::FAINT, "pick one to hire"),
            (None, false, _) => {}
        }
        let mine: Vec<HireRow> = t.hires.clone().unwrap_or_default();
        let hired_rows: Vec<Vec<String>> = mine
            .iter()
            .map(|r| vec![r.name.clone(), r.role.clone(), until(r.ends_at, unix)])
            .collect();
        ui.list(
            col.take(hires),
            "hires",
            &[0.0, 0.42, 0.62],
            &hired_rows,
            &mut t.hired,
        );
        let hired = mine.get(t.hired).map(|r| r.hire);
        let listing = col.take(line);
        match t.listed {
            Some(0) => ui.label(
                listing.x,
                listing.y,
                listing.w,
                ui::FAINT,
                "not for hire: name a price to list this character",
            ),
            Some(price) => {
                let label = "this character is for hire at";
                let parts = coin_parts(price);
                let price_w: f32 = parts.iter().map(|(t, _)| ui.text_width(t)).sum::<f32>()
                    + (parts.len() as f32) * ADVANCE * s;
                let room = listing.w - price_w - 2.0 * ADVANCE * s;
                ui.label(listing.x, listing.y, room.max(1.0), ui::FAINT, label);
                let x = listing.x + ui.text_width(label).min(room) + 2.0 * ADVANCE * s;
                ui.spans(x, listing.y, &parts);
            }
            None => {}
        }
        let fields = col.take(ui.field_height());
        coin_fields(ui, fields, gap, &mut t.price);
        let price = copper_of(&t.price).filter(|p| *p > 0);
        let listed = t.listed;
        let known = t.list.is_some();
        let ready = !self.busy() && known;
        self.notice_lines(ui, col.take(2.0 * line));
        let row = ui.buttons(
            col.take(h),
            &["Hire", "Dismiss", "List", "Withdraw", "Back"],
        );
        let mut op: Option<(&'static str, PlayerEcon)> = None;
        if ui.button_if(row[0], "Hire", ready && picked.is_some())
            && let Some(row) = &picked
        {
            // At the price shown: the hub refuses when it is another now.
            let hire = PlayerEcon::Hire {
                avatar: row.character,
                price: row.price,
            };
            op = Some((
                "hired: it joins your squad at the next zone you enter",
                hire,
            ));
        }
        if ui.button_if(row[1], "Dismiss", ready && hired.is_some())
            && let Some(hire) = hired
        {
            op = Some(("sent away", PlayerEcon::Dismiss { hire }));
        }
        if ui.button_if(row[2], "List", ready && price.is_some())
            && let Some(price) = price
        {
            op = Some((
                "listed: it serves while you are away",
                PlayerEcon::HireList { price },
            ));
        }
        if ui.button_if(row[3], "Withdraw", ready && listed.is_some_and(|p| p > 0)) {
            op = Some(("not for hire any more", PlayerEcon::HireUnlist));
        }
        if ui.button(row[4], "Back") || ui.key(Key::Escape) {
            self.page = Page::People;
            self.notice.clear();
            return PeopleAction::None;
        }
        if let Some((done, op)) = op {
            self.notice.clear();
            self.ask(hub, What::Tavernly(done), op);
        }
        PeopleAction::None
    }

    // ---------- answers ----------

    /// Take the answers that have come.
    fn answers(&mut self, hub: &dyn HubApi) {
        let now = self.now;
        let mut i = 0;
        while i < self.asks.len() {
            let Some(answer) = self.asks[i].1.take() else {
                i += 1;
                continue;
            };
            let (what, _) = self.asks.remove(i);
            match (what, answer) {
                (
                    What::View,
                    Ok(PlayerResponse::Econ(PlayerEconReply::TradeView {
                        state,
                        version,
                        wait_ms,
                        with,
                        together,
                        mine,
                        theirs,
                    })),
                ) => {
                    let Some(t) = &mut self.trade else { continue };
                    t.seen_at = Some(now);
                    t.with = with;
                    // A version is looked at from the moment it is first on the screen;
                    // what the hub last answered was about the version before, unless it
                    // answered this player's own change, which made this version.
                    if t.version_since.is_none_or(|(v, _)| v != version) {
                        t.version_since = Some((version, now));
                        if t.shown.is_some() && !std::mem::take(&mut t.own_word) {
                            self.notice.clear();
                        }
                    }
                    // The same things stay picked, by what they are.
                    let was = |list: &[ItemSummary], at: usize| list.get(at).map(|i| i.id);
                    if let Some(old) = &t.shown {
                        let (a, b) = (
                            was(&old.mine.items, t.picked[0]),
                            was(&old.theirs.items, t.picked[1]),
                        );
                        keep(&mine.items, &mut t.picked[0], a, |i| i.id);
                        keep(&theirs.items, &mut t.picked[1], b, |i| i.id);
                    }
                    match state {
                        TRADE_COMMITTED => t.over = Some("the trade is done"),
                        TRADE_CANCELLED => t.over = Some("the trade was called off"),
                        _ => {}
                    }
                    t.shown = Some(Shown {
                        state,
                        version,
                        wait_ms,
                        together,
                        mine,
                        theirs,
                    });
                }
                (
                    What::View,
                    Err(RpcError::Refused(HubError::NotFound | HubError::Unauthorized)),
                ) => {
                    if let Some(t) = &mut self.trade {
                        t.over = Some("the trade was called off");
                    }
                }
                // A look that failed is asked again in a second; what is on the screen
                // goes stale meanwhile, and a stale view arms nothing.
                (What::View, _) => {}
                (
                    What::Inventory,
                    Ok(PlayerResponse::Econ(PlayerEconReply::Holder { items, .. })),
                ) => {
                    if let Some(t) = &mut self.trade {
                        let was = t
                            .inventory
                            .as_ref()
                            .and_then(|l| l.get(t.picked[2]))
                            .map(|i| i.id);
                        keep(&items, &mut t.picked[2], was, |i| i.id);
                        t.inventory = Some(items);
                    }
                }
                (What::Inventory, _) => {}
                (What::Change(done), Ok(_)) => {
                    self.say(done, false);
                    if let Some(t) = &mut self.trade {
                        t.own_word = true;
                    }
                    self.look_again(hub);
                }
                (
                    What::Accept(theirs),
                    Ok(PlayerResponse::Econ(PlayerEconReply::Trade { committed })),
                ) => {
                    if let Some(t) = &mut self.trade {
                        // This is what was agreed to: what differs from it later is marked.
                        t.agreed = theirs;
                        t.agreed_once = true;
                        if committed {
                            t.over = Some("the trade is done");
                        }
                    }
                    self.look_again(hub);
                }
                (What::Change(_) | What::Accept(_), Err(e)) => {
                    self.say(words(&e), true);
                    self.look_again(hub);
                }
                (What::Accept(_), Ok(_)) => self.say("the hub answered something else", true),
                (What::Tavern, Ok(PlayerResponse::Econ(PlayerEconReply::Tavern(list)))) => {
                    let t = &mut self.tavern;
                    let was = t
                        .list
                        .as_ref()
                        .and_then(|l| l.get(t.picked))
                        .map(|r| r.character);
                    keep(&list, &mut t.picked, was, |r| r.character);
                    if t.list.is_none() {
                        t.picked = NONE;
                    }
                    t.list = Some(list);
                }
                (What::Hires, Ok(PlayerResponse::Econ(PlayerEconReply::Hires(hires)))) => {
                    let t = &mut self.tavern;
                    let was = t
                        .hires
                        .as_ref()
                        .and_then(|l| l.get(t.hired))
                        .map(|r| r.hire);
                    keep(&hires, &mut t.hired, was, |r| r.hire);
                    if t.hires.is_none() {
                        t.hired = NONE;
                    }
                    t.hires = Some(hires);
                }
                (What::Listed, Ok(PlayerResponse::Econ(PlayerEconReply::Id(price)))) => {
                    self.tavern.listed = Some(price);
                }
                (What::Tavern | What::Hires | What::Listed, Err(e)) => self.say(words(&e), true),
                (What::Tavern | What::Hires | What::Listed, Ok(_)) => {
                    self.say("the hub answered something else", true)
                }
                (What::Tavernly(done), Ok(_)) => {
                    self.say(done, false);
                    self.tavern.price = Default::default();
                    self.read_tavern(hub);
                }
                (What::Tavernly(_), Err(e)) => {
                    self.say(words(&e), true);
                    self.read_tavern(hub);
                }
            }
        }
    }

    /// After something the player did in a trade: what it looks like now, at once.
    fn look_again(&mut self, hub: &dyn HubApi) {
        let Some(t) = &mut self.trade else { return };
        if t.over.is_some() {
            return;
        }
        let id = t.id;
        t.asked_at = Some(self.now);
        if !self.asks.iter().any(|(what, _)| *what == What::View) {
            self.ask(hub, What::View, PlayerEcon::TradeView { trade: id });
        }
        self.ask(hub, What::Inventory, PlayerEcon::Inventory);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gm_hub_proto::protocol::{PLACE_NONE, PLACE_WEAPON};

    use super::*;
    use crate::ui::tests::{Recorder, SIZES, tidy};
    use crate::ui::{UiInput, UiState};

    const S: SessionId = SessionId([7; 16]);
    const ME: CharacterId = 42;

    fn item(id: i64, template: &str, does: &str) -> ItemSummary {
        let whole = !template.is_empty();
        ItemSummary {
            id,
            template: if whole { template } else { "component" }.to_string(),
            components: vec![("core".into(), "core/iron".into())],
            place: if whole { PLACE_WEAPON } else { PLACE_NONE },
            edge: [0; 8],
            worn: false,
            what: if whole {
                "a weapon, 250 of 250".to_string()
            } else {
                "a core, for crafting".to_string()
            },
            does: if does.is_empty() {
                Vec::new()
            } else {
                vec![does.to_string()]
            },
        }
    }

    /// A trade and a tavern as a hub would keep them, moved by the test's hand.
    #[derive(Default)]
    struct Desk {
        state: u8,
        version: i32,
        wait_ms: u32,
        together: bool,
        mine: Vec<ItemSummary>,
        my_coin: i64,
        i_accepted: bool,
        theirs: Vec<ItemSummary>,
        their_coin: i64,
        they_accepted: bool,
        carried: Vec<ItemSummary>,
        for_hire: Vec<TavernRow>,
        hires: Vec<HireRow>,
        listed: i64,
        /// Whom the trade is with (Bojan, when nobody is named).
        with: Option<String>,
        /// What a look at the trade, and what anything else, is refused with.
        deaf: Option<HubError>,
        refuse: Option<HubError>,
        asked: Vec<PlayerEcon>,
    }

    struct Hub(RefCell<Desk>);

    impl Hub {
        /// The other side changes its offer: both accepts are cleared, the version moves
        /// and the three seconds begin again (ECONOMY.md 6).
        fn they_offer(&self, items: Vec<ItemSummary>, coin: i64) {
            let mut d = self.0.borrow_mut();
            d.theirs = items;
            d.their_coin = coin;
            d.version += 1;
            d.wait_ms = 3000;
            d.i_accepted = false;
            d.they_accepted = false;
        }

        fn looks(&self) -> usize {
            let d = self.0.borrow();
            d.asked
                .iter()
                .filter(|op| matches!(op, PlayerEcon::TradeView { .. }))
                .count()
        }

        fn last(&self) -> PlayerEcon {
            let d = self.0.borrow();
            d.asked
                .iter()
                .rev()
                .find(|op| {
                    !matches!(
                        op,
                        PlayerEcon::TradeView { .. }
                            | PlayerEcon::Inventory
                            | PlayerEcon::Tavern
                            | PlayerEcon::Hires
                            | PlayerEcon::HireListed
                    )
                })
                .cloned()
                .expect("something was asked")
        }
    }

    impl HubApi for Hub {
        fn call(&self, req: PlayerRequest) -> Pending<Answer> {
            let PlayerRequest::Econ {
                session: S,
                character: ME,
                op,
            } = req
            else {
                panic!("not what these screens ask: {req:?}");
            };
            let mut d = self.0.borrow_mut();
            d.asked.push(op.clone());
            let look = matches!(op, PlayerEcon::TradeView { .. });
            let refused = if look {
                d.deaf.clone()
            } else {
                d.refuse.clone()
            };
            let touched = |d: &mut Desk| {
                d.version += 1;
                d.wait_ms = 3000;
                d.i_accepted = false;
                d.they_accepted = false;
            };
            let reply = match (refused, op) {
                (Some(e), _) => Err(e),
                (None, PlayerEcon::TradeView { .. }) => Ok(PlayerEconReply::TradeView {
                    state: d.state,
                    version: d.version,
                    wait_ms: d.wait_ms,
                    with: d.with.clone().unwrap_or_else(|| "Bojan".into()),
                    together: d.together,
                    mine: TradeOffer {
                        coin: d.my_coin,
                        accepted: d.i_accepted,
                        items: d.mine.clone(),
                    },
                    theirs: TradeOffer {
                        coin: d.their_coin,
                        accepted: d.they_accepted,
                        items: d.theirs.clone(),
                    },
                }),
                (None, PlayerEcon::Inventory) => Ok(PlayerEconReply::Holder {
                    coin: 500,
                    items: d.carried.clone(),
                }),
                (None, PlayerEcon::TradeOffer { item, .. }) => {
                    let it = d.carried.iter().find(|i| i.id == item).cloned();
                    match it {
                        Some(it) => {
                            d.mine.push(it);
                            touched(&mut d);
                            Ok(PlayerEconReply::Done)
                        }
                        None => Err(HubError::Unauthorized),
                    }
                }
                (None, PlayerEcon::TradeRetract { item, .. }) => {
                    d.mine.retain(|i| i.id != item);
                    touched(&mut d);
                    Ok(PlayerEconReply::Done)
                }
                (None, PlayerEcon::TradeCoin { coin, .. }) => {
                    d.my_coin = coin;
                    touched(&mut d);
                    Ok(PlayerEconReply::Done)
                }
                (None, PlayerEcon::TradeAccept { version, .. }) => {
                    if version != d.version {
                        Err(HubError::Invalid("the offer changed".into()))
                    } else if d.wait_ms > 0 {
                        Err(HubError::Cooldown)
                    } else {
                        d.i_accepted = true;
                        let committed = d.they_accepted;
                        if committed {
                            d.state = TRADE_COMMITTED;
                        }
                        Ok(PlayerEconReply::Trade { committed })
                    }
                }
                (None, PlayerEcon::TradeCancel { .. }) => {
                    d.state = TRADE_CANCELLED;
                    Ok(PlayerEconReply::Done)
                }
                (None, PlayerEcon::Tavern) => Ok(PlayerEconReply::Tavern(d.for_hire.clone())),
                (None, PlayerEcon::Hires) => Ok(PlayerEconReply::Hires(d.hires.clone())),
                (None, PlayerEcon::HireListed) => Ok(PlayerEconReply::Id(d.listed)),
                (None, PlayerEcon::Hire { avatar, price }) => {
                    let row = d.for_hire.iter().find(|r| r.character == avatar).cloned();
                    match row {
                        Some(row) if row.price == price => {
                            d.hires.push(HireRow {
                                hire: 900 + avatar,
                                name: row.name,
                                what: row.what,
                                role: row.role,
                                ends_at: 1_000 + 12 * 3600,
                            });
                            Ok(PlayerEconReply::Id(900 + avatar))
                        }
                        Some(_) => Err(HubError::Invalid("the price changed: look again".into())),
                        None => Err(HubError::NotFound),
                    }
                }
                (None, PlayerEcon::Dismiss { hire }) => {
                    d.hires.retain(|h| h.hire != hire);
                    Ok(PlayerEconReply::Done)
                }
                (None, PlayerEcon::HireList { price }) => {
                    d.listed = price;
                    Ok(PlayerEconReply::Done)
                }
                (None, PlayerEcon::HireUnlist) => {
                    d.listed = 0;
                    Ok(PlayerEconReply::Done)
                }
                (None, other) => panic!("not what these screens ask: {other:?}"),
            };
            Pending::ready(reply.map(PlayerResponse::Econ).map_err(RpcError::Refused))
        }
    }

    /// A test's own clock, screen, and what the zone has said of people.
    struct Run {
        st: UiState,
        t: Instant,
        social: Social,
        here: Vec<Here>,
        /// What the zone last said.
        word: Option<String>,
        size: (f32, f32),
        canvas: Recorder,
    }

    impl Run {
        fn new() -> Run {
            Run {
                st: UiState::default(),
                t: Instant::now(),
                social: Social::default(),
                here: Vec::new(),
                word: None,
                size: (1280.0, 720.0),
                canvas: Recorder::new(1280.0, 720.0),
            }
        }

        fn arrives(&mut self, body: u32, name: &str, near: bool) {
            self.here.push(Here {
                body,
                name: name.into(),
                near,
            });
        }

        fn frame(&mut self, p: &mut People, input: &UiInput, hub: Option<&Hub>) -> PeopleAction {
            self.canvas = Recorder::new(self.size.0, self.size.1);
            let mut ui = Ui::begin(
                &mut self.canvas,
                &mut self.st,
                input,
                p.page.name(),
                PANEL_UNITS,
            );
            let hub = hub.map(|h| h as &dyn HubApi);
            let action = p.frame(
                &mut ui,
                hub,
                "Ana",
                &mut self.social,
                &self.here,
                self.word.as_deref(),
                self.t,
                1_000,
            );
            ui.end();
            action
        }

        /// The screen once the answers that have come are on it.
        fn look(&mut self, p: &mut People, hub: Option<&Hub>) {
            let idle = UiInput::default();
            self.frame(p, &idle, hub);
            self.frame(p, &idle, hub);
        }

        /// Time passes, a frame every half second (the window asks every second).
        fn pass(&mut self, p: &mut People, hub: Option<&Hub>, secs: f32) {
            let steps = (secs * 2.0).round() as u32;
            for _ in 0..steps {
                self.t += Duration::from_millis(500);
                self.look(p, hub);
            }
        }

        fn click(&mut self, p: &mut People, text: &str, hub: Option<&Hub>) -> PeopleAction {
            self.look(p, hub);
            let at = self
                .st
                .find(text)
                .unwrap_or_else(|| panic!("nothing to click says {text:?}: {:?}", self.said()))
                .rect
                .centre();
            let press = UiInput {
                cursor: at,
                pressed: true,
                down: true,
                ..Default::default()
            };
            self.frame(p, &press, hub);
            let release = UiInput {
                cursor: at,
                released: true,
                ..Default::default()
            };
            let action = self.frame(p, &release, hub);
            self.look(p, hub);
            action
        }

        fn typed(&mut self, p: &mut People, text: &str, hub: Option<&Hub>) {
            let input = UiInput {
                text: text.into(),
                ..Default::default()
            };
            self.frame(p, &input, hub);
        }

        fn shows(&self, text: &str) -> bool {
            self.st.shows(text)
        }

        fn offers(&self, text: &str) -> bool {
            self.st.find(text).is_some()
        }

        fn said(&self) -> Vec<&str> {
            self.st.seen.iter().map(|s| s.text.as_str()).collect()
        }
    }

    #[test]
    fn the_page_of_people_says_who_is_here_and_asks_the_zone() {
        let mut run = Run::new();
        run.arrives(5, "Bojan", true);
        run.arrives(6, "Cvita", false);
        run.arrives(7, "De Vil", true);
        let mut p = People::here(S, ME, run.t);
        // No hub is asked anything on this page: the zone is.
        run.look(&mut p, None);
        assert!(run.shows("you are in no party"));
        assert!(run.shows("Bojan") && run.shows("Cvita") && run.shows("De Vil"));
        // Nothing is picked: nothing acts but what needs no row.
        for off in [
            "Invite", "Join", "Decline", "Remove", "Leave", "Trade", "Whisper", "Tavern",
        ] {
            assert!(!run.offers(off), "{off} with nothing picked");
        }
        // A row is picked: somebody here can be asked into a party, and whispered to;
        // a trade needs a hub and somebody near.
        run.click(&mut p, "Bojan", None);
        assert!(run.offers("Invite") && run.offers("Whisper") && !run.offers("Trade"));
        assert_eq!(
            run.click(&mut p, "Invite", None),
            PeopleAction::Zone(FromClient::PartyInvite {
                name: "Bojan".into()
            })
        );
        // The zone takes one a second: the button is off for that long. What the zone
        // answers is said on the page as well as in the chat's lines under it.
        assert!(!run.offers("Invite"));
        run.word = Some("Bojan was asked to join".into());
        run.look(&mut p, None);
        assert!(run.shows("Bojan was asked to join"));
        run.word = None;
        run.pass(&mut p, None, 1.5);
        assert!(run.offers("Invite"));
        // A whisper to a name with a space in it is written as one word.
        run.click(&mut p, "De Vil", None);
        assert_eq!(
            run.click(&mut p, "Whisper", None),
            PeopleAction::Whisper("DeVil".into())
        );

        // The party is the zone's word: the leader first.
        run.social.party = vec!["Ana".into(), "Bojan".into(), "Ema".into()];
        run.look(&mut p, None);
        assert!(run.shows("your party: Ana, Bojan, Ema"));
        assert!(run.shows("Bojan  party") && run.shows("Ema  party, away"));
        // The leader removes; anybody leaves; a member is not invited again.
        run.click(&mut p, "Bojan", None);
        assert!(!run.offers("Invite") && run.offers("Remove") && run.offers("Leave"));
        assert_eq!(
            run.click(&mut p, "Remove", None),
            PeopleAction::Zone(FromClient::PartyRemove {
                name: "Bojan".into()
            })
        );
        run.pass(&mut p, None, 1.5);
        assert_eq!(
            run.click(&mut p, "Leave", None),
            PeopleAction::Zone(FromClient::PartyLeave)
        );
        // Somebody else leads: no inviting, no removing.
        run.social.party = vec!["Bojan".into(), "Ana".into()];
        run.pass(&mut p, None, 1.5);
        assert!(run.shows("Bojan  leads"));
        run.click(&mut p, "Cvita", None);
        assert!(!run.offers("Invite") && !run.offers("Remove") && run.offers("Leave"));

        // Who asked what is on its row, and the buttons answer it.
        run.social.party.clear();
        assert!(run.social.invited("Cvita".into(), run.t));
        assert!(
            !run.social.invited("cvita".into(), run.t),
            "the same one, once"
        );
        assert!(run.social.trade_asked(5, "Bojan".into(), run.t));
        run.look(&mut p, None);
        assert!(run.shows("Cvita  invites you") && run.shows("Bojan  asks to trade"));
        assert!(run.offers("Join") && run.offers("Decline"));
        assert_eq!(
            run.click(&mut p, "Join", None),
            PeopleAction::Zone(FromClient::PartyAnswer {
                from: "Cvita".into(),
                join: true
            })
        );
        // The invitation stands until the party itself answers it (the zone may have
        // refused the request); the button is off for the zone's second meanwhile.
        assert!(run.shows("invites you"));
        assert!(!run.offers("Join"));
        run.social.party = vec!["Cvita".into(), "Ana".into()];
        run.social.asks.retain(|a| a.trade.is_some());
        run.pass(&mut p, None, 1.5);
        assert!(!run.shows("invites you"), "in the party: answered");
        run.social.party.clear();
        // Declining takes the invitation off the page at once.
        assert!(run.social.invited("Cvita".into(), run.t));
        run.pass(&mut p, None, 1.5);
        run.click(&mut p, "Cvita", None);
        assert_eq!(
            run.click(&mut p, "Decline", None),
            PeopleAction::Zone(FromClient::PartyAnswer {
                from: "Cvita".into(),
                join: false
            })
        );
        assert!(!run.shows("invites you"), "declined");
        // A trade: with a hub, with somebody near. Asking back is the same request.
        let hub = Hub(RefCell::default());
        run.pass(&mut p, Some(&hub), 1.5);
        assert!(!run.offers("Trade"), "Cvita is across the square");
        run.click(&mut p, "Bojan", Some(&hub));
        assert_eq!(
            run.click(&mut p, "Trade", Some(&hub)),
            PeopleAction::Zone(FromClient::TradeAsk { with: 5 })
        );
        assert!(
            hub.0.borrow().asked.is_empty(),
            "the zone is asked, not the hub"
        );

        // The rows keep their places while the page is up: whoever leaves stays as a row
        // that says so, whoever comes is added at the end, and what was picked stays
        // picked by its name.
        run.click(&mut p, "Cvita", Some(&hub));
        let place = |run: &Run, name: &str| run.st.find(name).unwrap().rect.y;
        let (bojan, cvita) = (place(&run, "Bojan"), place(&run, "Cvita"));
        run.here.remove(0);
        run.arrives(9, "Aldric", true);
        run.pass(&mut p, Some(&hub), 1.5);
        assert!(run.shows("Bojan  gone"));
        assert_eq!((place(&run, "Bojan"), place(&run, "Cvita")), (bojan, cvita));
        assert!(place(&run, "Aldric") > cvita, "added at the end");
        assert!(run.offers("Invite"), "Cvita is still picked");
        // An invitation and a request to trade lapse by themselves.
        run.social.invited("De Vil".into(), run.t);
        run.social.trade_asked(7, "De Vil".into(), run.t);
        run.pass(&mut p, Some(&hub), 31.0);
        assert!(run.shows("De Vil  invites you") && !run.shows("asks to trade"));
        run.pass(&mut p, Some(&hub), 30.0);
        assert!(!run.shows("invites you"));
        // Escape closes it.
        let escape = UiInput {
            keys: vec![Key::Escape],
            ..Default::default()
        };
        assert_eq!(run.frame(&mut p, &escape, Some(&hub)), PeopleAction::Close);
        assert!(run.st.clipped.is_empty(), "{:?}", run.st.clipped);
    }

    #[test]
    fn a_trade_marks_what_was_not_agreed_to_and_arms_only_what_was_looked_at() {
        let hub = Hub(RefCell::new(Desk {
            together: true,
            carried: vec![item(1, "sword", "slash +2.0%"), item(2, "", "")],
            ..Desk::default()
        }));
        let mut run = Run::new();
        let mut p = People::trade(&hub, S, ME, 7, "Bojan".into(), run.t);
        run.look(&mut p, Some(&hub));
        assert!(run.shows("trade with Bojan"));
        assert!(run.shows("you give") && run.shows("Bojan gives"));
        // A row says what a thing does and, of gear, how much of it is left.
        assert!(run.shows("sword  slash +2.0%, 250 of 250"));
        assert!(run.shows("iron  a core, for crafting"));
        // The coin is a row of each offer.
        assert!(run.shows("coin  0 c"));
        // It is asked for once a second while the window is up, not every frame.
        let before = hub.looks();
        run.pass(&mut p, Some(&hub), 4.0);
        assert!(
            (3..=5).contains(&(hub.looks() - before)),
            "{}",
            hub.looks() - before
        );
        // Nothing is offered on either side: there is nothing to accept.
        assert!(!run.offers("Accept") && run.offers("Cancel"));

        // The other offers the best sword. It is new, and says so; it can be accepted
        // only when the hub's three seconds are over AND it has been on this screen
        // for three.
        hub.they_offer(vec![item(8, "sword", "slash +11.0%")], 0);
        run.pass(&mut p, Some(&hub), 1.0);
        assert!(run.shows("sword  slash +11.0%, 250 of 250  new"));
        assert!(!run.offers("Accept"));
        assert!(
            run.shows("look at the offer: it can be accepted in"),
            "{:?}",
            run.said()
        );
        hub.0.borrow_mut().wait_ms = 0;
        run.pass(&mut p, Some(&hub), 1.0);
        assert!(!run.offers("Accept"), "the hub is ready, the eye is not");
        run.pass(&mut p, Some(&hub), 2.0);
        assert!(run.offers("Accept"));
        // The quick swap before anything was agreed to: the lesser sword in the place
        // of the one that was looked at is not merely new, it is a change, and the one
        // that was looked at is still shown, struck.
        hub.they_offer(vec![item(9, "sword", "slash +2.0%")], 0);
        run.pass(&mut p, Some(&hub), 1.0);
        assert!(
            run.shows("sword  slash +2.0%, 250 of 250  changed"),
            "{:?}",
            run.said()
        );
        assert!(run.shows("sword  slash +11.0%, 250 of 250  taken back"));
        assert!(run.shows("Bojan changed the offer: look at what is marked"));
        assert!(!run.offers("Accept"));
        hub.they_offer(vec![item(8, "sword", "slash +11.0%")], 0);
        hub.0.borrow_mut().wait_ms = 0;
        // (Seen at the next look, within a second; three seconds on the screen from then.)
        run.pass(&mut p, Some(&hub), 4.5);
        assert!(run.offers("Accept"), "{:?}", run.said());
        // This player's own offer: an item and coin, by the hub's requests.
        run.click(&mut p, "iron", Some(&hub));
        run.click(&mut p, "Offer", Some(&hub));
        assert_eq!(hub.last(), PlayerEcon::TradeOffer { trade: 7, item: 2 });
        assert!(run.shows("iron  a core, for crafting  offered"));
        run.click(&mut p, "silver", Some(&hub));
        run.typed(&mut p, "50", Some(&hub));
        run.click(&mut p, "Set coin", Some(&hub));
        assert_eq!(
            hub.last(),
            PlayerEcon::TradeCoin {
                trade: 7,
                coin: 5000
            }
        );
        // The hub's word on this player's own change stays through the version it made;
        // the other's next change clears it (the word was about the offer before).
        run.pass(&mut p, Some(&hub), 1.5);
        assert!(run.shows("the coin is set"), "{:?}", run.said());
        hub.they_offer(vec![item(8, "sword", "slash +11.0%")], 0);
        run.pass(&mut p, Some(&hub), 1.5);
        assert!(!run.shows("the coin is set"), "{:?}", run.said());
        // Its own change began the three seconds again, at the hub and on the screen.
        assert!(!run.offers("Accept"));
        hub.0.borrow_mut().wait_ms = 0;
        run.pass(&mut p, Some(&hub), 3.5);
        run.click(&mut p, "Accept", Some(&hub));
        assert!(matches!(
            hub.last(),
            PlayerEcon::TradeAccept { trade: 7, .. }
        ));
        assert!(run.shows("you accepted: waiting for Bojan"));
        assert!(!run.shows("new"), "what was accepted is agreed to");

        // The swap after an accept: the other takes the sword back and puts a lesser one
        // of the same name in its place. Both accepts are gone; the new one is marked
        // and the old one is still shown, struck, with a line that says what happened.
        hub.they_offer(vec![item(9, "sword", "slash +2.0%")], 0);
        run.pass(&mut p, Some(&hub), 1.0);
        assert!(run.shows("sword  slash +2.0%, 250 of 250  changed"));
        assert!(run.shows("sword  slash +11.0%, 250 of 250  taken back"));
        assert!(run.shows("Bojan changed the offer: look at what is marked"));
        assert!(!run.offers("Accept"));
        // The marks do not fade with the three seconds: they stay until this player
        // accepts again, however long that is. (Looked at for three seconds, the change
        // is "new" rather than "changed": not agreed to, and no longer a surprise.)
        hub.0.borrow_mut().wait_ms = 0;
        run.pass(&mut p, Some(&hub), 20.0);
        assert!(run.offers("Accept"));
        assert!(
            run.shows("sword  slash +2.0%, 250 of 250  new"),
            "{:?}",
            run.said()
        );
        assert!(run.shows("sword  slash +11.0%, 250 of 250  taken back"));
        assert!(run.shows("Bojan changed the offer"));
        // The coin too: a row, marked like the rest.
        hub.they_offer(vec![item(9, "sword", "slash +2.0%")], 300);
        hub.0.borrow_mut().wait_ms = 0;
        run.pass(&mut p, Some(&hub), 1.0);
        assert!(run.shows("coin  3 s  changed"), "{:?}", run.said());
        run.pass(&mut p, Some(&hub), 3.5);
        assert!(run.shows("coin  3 s  new"), "{:?}", run.said());
        run.click(&mut p, "Accept", Some(&hub));
        assert!(!run.shows("new") && !run.shows("taken back") && !run.shows("changed"));
        assert!(run.shows("coin  3 s"));

        // A view that is old arms nothing: the hub does not answer, and two seconds
        // later the button is off.
        hub.they_offer(vec![item(9, "sword", "slash +2.0%")], 310);
        hub.0.borrow_mut().wait_ms = 0;
        run.pass(&mut p, Some(&hub), 4.0);
        assert!(run.offers("Accept"));
        hub.0.borrow_mut().deaf = Some(HubError::Busy);
        run.pass(&mut p, Some(&hub), 3.0);
        assert!(!run.offers("Accept"), "nothing acts on a view that is old");
        hub.0.borrow_mut().deaf = None;
        run.pass(&mut p, Some(&hub), 1.5);
        assert!(run.offers("Accept"));
        // A refusal is said in words.
        hub.0.borrow_mut().refuse = Some(HubError::Insufficient);
        run.click(&mut p, "Accept", Some(&hub));
        assert!(run.shows("not enough coin"));
        hub.0.borrow_mut().refuse = None;
        // The other is elsewhere: said, over the last refusal.
        hub.0.borrow_mut().together = false;
        run.click(&mut p, "sword", Some(&hub));
        run.pass(&mut p, Some(&hub), 1.5);
        assert!(run.shows("Bojan is not here any more"), "{:?}", run.said());

        // The other accepts, then this player: done, and only Close is left.
        {
            let mut d = hub.0.borrow_mut();
            d.together = true;
            d.they_accepted = true;
        }
        run.pass(&mut p, Some(&hub), 1.5);
        run.click(&mut p, "Accept", Some(&hub));
        assert!(run.shows("the trade is done"), "{:?}", run.said());
        assert!(!run.offers("Accept") && !run.offers("Cancel") && run.offers("Close"));
        assert_eq!(run.click(&mut p, "Close", Some(&hub)), PeopleAction::Close);

        // Cancel says so to the hub and closes; a trade called off by the hub says so.
        let hub = Hub(RefCell::new(Desk {
            together: true,
            ..Desk::default()
        }));
        let mut p = People::trade(&hub, S, ME, 8, "Bojan".into(), run.t);
        assert_eq!(run.click(&mut p, "Cancel", Some(&hub)), PeopleAction::Close);
        assert_eq!(hub.last(), PlayerEcon::TradeCancel { trade: 8 });
        let mut p = People::trade(&hub, S, ME, 8, "Bojan".into(), run.t);
        run.pass(&mut p, Some(&hub), 1.5);
        assert!(run.shows("the trade was called off") && run.offers("Close"));
    }

    #[test]
    fn the_tavern_hires_at_the_price_shown_and_lists_this_character() {
        let row = |character, name: &str, what: &str, role: &str, price, hires| TavernRow {
            character,
            name: name.into(),
            what: what.into(),
            role: role.into(),
            price,
            hires,
        };
        let hub = Hub(RefCell::new(Desk {
            for_hire: vec![
                row(
                    11,
                    "Aldric",
                    "ironclad: colossus in plate",
                    "tank",
                    12_000,
                    0,
                ),
                row(12, "Mirela", "mender: caster in cloth", "heal", 9_500, 4),
            ],
            ..Desk::default()
        }));
        let mut run = Run::new();
        run.arrives(5, "Bojan", true);
        let mut p = People::here(S, ME, run.t);
        run.click(&mut p, "Tavern", Some(&hub));
        assert_eq!(p.page, Page::Tavern);
        // What the rule is, before anybody pays; and the list, in the hub's words.
        assert!(run.shows("may take it back at any time") && run.shows("nothing is refunded"));
        assert!(run.shows("Aldric  tank  1 g 20 s"));
        assert!(run.shows("Mirela  heal  95 s  hired 4x"));
        assert!(run.shows("not for hire: name a price to list this character"));
        assert!(!run.offers("Hire") && !run.offers("Dismiss") && !run.offers("Withdraw"));
        // Hired at the price that was shown: the request names it.
        run.click(&mut p, "Aldric", Some(&hub));
        assert!(run.shows("ironclad: colossus in plate, tank") && run.shows("Aldric for"));
        run.click(&mut p, "Hire", Some(&hub));
        assert_eq!(
            hub.last(),
            PlayerEcon::Hire {
                avatar: 11,
                price: 12_000
            }
        );
        assert!(run.shows("hired: it joins your squad at the next zone you enter"));
        assert!(run.shows("Aldric  tank  12 h 0 min"));
        // The price is another by the time of the click: nothing is hired, and it is said.
        hub.0.borrow_mut().for_hire[1].price = 99_000;
        run.click(&mut p, "Mirela", Some(&hub));
        // (The screen still shows the price it was told.)
        hub.0.borrow_mut().for_hire[1].price = 120_000;
        run.click(&mut p, "Hire", Some(&hub));
        assert!(
            run.shows("the price changed: look again"),
            "{:?}",
            run.said()
        );
        // (The request named the price on the screen, not the one at the hub.)
        assert!(
            hub.0.borrow().asked.iter().any(|op| matches!(
                op,
                PlayerEcon::Hire {
                    avatar: 12,
                    price: 9_500
                }
            )),
            "{:?}",
            hub.0.borrow().asked
        );
        assert_eq!(hub.0.borrow().hires.len(), 1);
        // A hire is sent away.
        run.click(&mut p, "Aldric  tank  12 h 0 min", Some(&hub));
        run.click(&mut p, "Dismiss", Some(&hub));
        assert_eq!(hub.last(), PlayerEcon::Dismiss { hire: 911 });
        assert!(run.shows("sent away"));
        // This character is listed at a price named in three fields, and taken off.
        run.click(&mut p, "gold", Some(&hub));
        run.typed(&mut p, "2", Some(&hub));
        run.click(&mut p, "List", Some(&hub));
        assert_eq!(hub.last(), PlayerEcon::HireList { price: 20_000 });
        assert!(run.shows("this character is for hire at"));
        run.click(&mut p, "Withdraw", Some(&hub));
        assert_eq!(hub.last(), PlayerEcon::HireUnlist);
        assert!(run.shows("not for hire: name a price to list this character"));
        assert!(run.st.clipped.is_empty(), "{:?}", run.st.clipped);
        // Back is the page of people again.
        run.click(&mut p, "Back", Some(&hub));
        assert_eq!(p.page, Page::People);
    }

    #[test]
    fn every_page_is_whole_at_every_size() {
        // The longest words the content and the hub can put in a row: a name of 24
        // bytes, the longest material, the longest things an item is said to do, the
        // dearest price there is.
        let long = "Žanamarija Škrinjarić";
        let longest_name = "Abcdefghijklmnopqrstuvwx";
        let dearest = 1_000_000_000_000;
        let mut thunderstone = item(2, "", "");
        thunderstone.components = vec![("catalyst".into(), "catalyst/thunderstone".into())];
        thunderstone.what = "a catalyst, for crafting".into();
        let mut cuirass = item(8, "cuirass", "physical -12.5%");
        cuirass.place = gm_hub_proto::protocol::PLACE_ARMOUR;
        cuirass.what = "an armour, 250 of 250".into();
        for size in SIZES {
            let hub = Hub(RefCell::new(Desk {
                together: true,
                carried: vec![item(1, "crossbow", "pierce +11.0%"), thunderstone.clone()],
                theirs: vec![cuirass.clone(), thunderstone.clone()],
                their_coin: dearest,
                my_coin: dearest,
                with: Some(longest_name.into()),
                for_hire: vec![TavernRow {
                    character: 11,
                    name: longest_name.into(),
                    what: "frostweaver: infiltrator in leather".into(),
                    role: "support".into(),
                    price: dearest,
                    hires: 12,
                }],
                hires: vec![HireRow {
                    hire: 3,
                    name: longest_name.into(),
                    what: "frostweaver: infiltrator in leather".into(),
                    role: "support".into(),
                    ends_at: 1_000 + 11 * 3600 + 59 * 60,
                }],
                listed: dearest,
                ..Desk::default()
            }));
            let mut run = Run::new();
            run.size = size;
            run.arrives(5, long, true);
            run.arrives(6, "Bojan", true);
            run.social.party = vec![long.into(), "Ana".into(), "Bojan".into()];
            run.social.invited("Bojan".into(), run.t);
            run.social.trade_asked(6, "Bojan".into(), run.t);
            let mut pages = vec![
                People::here(S, ME, run.t),
                People::trade(&hub, S, ME, 7, long.into(), run.t),
            ];
            let mut tavern = People::here(S, ME, run.t);
            run.click(&mut tavern, "Tavern", Some(&hub));
            pages.push(tavern);
            for p in &mut pages {
                let row = match p.page {
                    Page::People => "Bojan",
                    Page::Trade => "crossbow",
                    Page::Tavern => longest_name,
                };
                run.click(p, row, Some(&hub));
                run.pass(p, Some(&hub), 1.5);
                if p.page == Page::Trade {
                    // The other's offer with its marks: the coin, looked at and changed.
                    assert!(run.shows("coin  100,000,000 g  new"), "{:?}", run.said());
                    assert!(run.shows("cuirass  physical -12.5%, 250 of 250  new"));
                    assert!(run.shows("thunderstone  a catalyst, for crafting  new"));
                    assert!(run.shows(&format!("trade with {longest_name}")));
                }
                if p.page == Page::Tavern {
                    assert!(
                        run.shows(&format!("{longest_name} for")),
                        "{:?}",
                        run.said()
                    );
                }
                tidy(&run.canvas, &run.st, PANEL_UNITS);
                assert!(
                    run.st.clipped.is_empty(),
                    "{} at {size:?}: cut {:?}",
                    p.page.name(),
                    run.st.clipped
                );
                // A price and what somebody is are never cut; a long name in a narrow
                // list may be (the line under the list has it whole).
                let cut: Vec<&String> = run
                    .st
                    .cut_cells
                    .iter()
                    .filter(|c| !c.contains("Žana") && c.as_str() != longest_name)
                    .collect();
                assert!(cut.is_empty(), "{} at {size:?}: cut {cut:?}", p.page.name());
            }
        }
    }
}
