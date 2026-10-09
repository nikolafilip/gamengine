//! The screens between starting the client and playing (CLIENT.md 4.1 to 4.3): log in, pick
//! or make a character, enter. One state machine asks the hub and waits for its answers; a
//! command line that already named the account and the character is the same machine with
//! nobody clicking (`Auto`), so a gate that enters by command line runs what a person
//! clicks through.

use gm_core::build::{Build, ContentPack};
use gm_core::vocab::ArchetypeFrame;
use gm_hub_proto::player::{PlayerRequest, PlayerResponse};
use gm_hub_proto::protocol::{
    BuildChoice, CharacterId, CharacterSummary, HubError, LocationSummary,
    MAX_CHARACTERS_PER_ACCOUNT, SessionId, ZoneTicket,
};
use web_time::{Duration, Instant};

use crate::hub::{Account, Answer, HubApi, Pending, RpcError};
use crate::ui::{self, Canvas, Column, Field, Key, ListEvent, Rect, Ui};

/// A zone takes a moment to put a character away after it left (its last save): an entry
/// the hub refuses as `Busy`, and a list that still shows the character in its zone, are
/// asked for again this often, this many times.
const RETRY_EVERY: Duration = Duration::from_millis(if cfg!(test) { 5 } else { 1000 });
const RETRIES: u32 = 20;

/// The widest panel of these screens, in units (pixels at scale 1).
pub const PANEL_UNITS: f32 = 340.0;

/// What the command line (or the page) already said: the screens are skipped as far as it
/// reaches.
#[derive(Clone, Debug, Default)]
pub struct Auto {
    pub email: String,
    pub password: String,
    pub register: bool,
    /// Empty: stop at the characters and let a person choose.
    pub character: String,
    /// The archetype of a character that does not exist yet.
    pub preset: Option<String>,
    /// Empty: where the character was, else the hub's start zone.
    pub zone: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Login,
    Characters,
    NewCharacter,
    Entering,
}

impl Screen {
    /// The name a UI script waits for (CLIENT.md 9).
    pub fn name(self) -> &'static str {
        match self {
            Screen::Login => "login",
            Screen::Characters => "characters",
            Screen::NewCharacter => "new character",
            Screen::Entering => "entering",
        }
    }
}

/// The request the machine is waiting on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Register,
    Login,
    Characters,
    Content,
    Create,
    Enter,
}

/// What the app must do after a frame of the screens.
#[derive(Debug, PartialEq)]
pub enum Action {
    None,
    /// The hub gave a ticket: connect to the zone as this character.
    Enter {
        ticket: Box<ZoneTicket>,
        name: String,
    },
    /// The session began or ended.
    Session(Option<Account>),
    /// The person gave up waiting for a zone: hang up on it.
    CancelEntering,
    Quit,
    /// On autopilot a refusal ends the program, as it always did.
    Failed(String),
}

pub struct Front {
    hub: Box<dyn HubApi>,
    pub screen: Screen,
    pub account: Option<Account>,
    auto: Option<Auto>,
    wait: Option<(Step, Pending<Answer>)>,
    /// A request to make again in a moment: when, which, how many more times after it.
    retry: Option<(Instant, Step, PlayerRequest, u32)>,
    /// What the screen says under its fields: the last refusal, in words.
    notice: String,
    /// What it says instead while the hub is being asked something a person waits for.
    busy: &'static str,
    // The login form.
    email: String,
    password: String,
    again: String,
    registering: bool,
    /// The passwords are shown as typed (a long one cannot be typed blind twice).
    show: bool,
    // The account's characters and the one picked.
    characters: Vec<CharacterSummary>,
    picked: usize,
    /// The character to pick once the list is here: the one played last.
    prefer: String,
    content: Option<ContentPack>,
    /// What each preset is, in the content's order.
    blurbs: Vec<String>,
    /// When the content was last asked for: it is asked again while it is missing.
    content_asked: Option<Instant>,
    // The new character's form.
    name: String,
    archetype: usize,
    /// The zone being entered, for the screen that says so.
    entering: String,
}

/// A ban's end as a day (UTC), or "for good" when it is further off than anybody plans.
pub fn day(unix: u64) -> String {
    const FOR_GOOD: u64 = 50 * 365 * 86_400;
    let now = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if unix > now + FOR_GOOD {
        return "for good".into();
    }
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = (unix / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("until {y:04}-{m:02}-{d:02}")
}

/// A refusal in words a person can act on.
fn words(step: Step, e: &RpcError) -> String {
    let RpcError::Refused(e) = e else {
        return e.to_string();
    };
    match (step, e) {
        (_, HubError::Credentials) => "wrong email or password".into(),
        (Step::Register, HubError::Taken) => "there is an account with this email already".into(),
        (Step::Create, HubError::Taken) => "that name is taken".into(),
        (Step::Register | Step::Login, HubError::Busy) => "too many attempts; wait a minute".into(),
        (_, HubError::Banned { until, reason }) => {
            format!("this account is banned {}: {reason}", day(*until))
        }
        (Step::Enter, HubError::NotFound) => "no zone is up to enter".into(),
        (Step::Enter, HubError::Busy) => {
            "the character is still in a zone; try again in a moment".into()
        }
        (Step::Enter, HubError::Full) => "that zone is full".into(),
        (Step::Create, HubError::Full) => "an account has room for ten characters".into(),
        (Step::Enter, HubError::Locked(what)) => format!("that zone asks for {what}"),
        (_, HubError::Unauthorized) => "the session ended; log in again".into(),
        (_, HubError::Internal) => "the hub had a problem; try again".into(),
        (_, HubError::Invalid(what)) => what.clone(),
        (_, other) => other.to_string(),
    }
}

pub fn frame_name(frame: ArchetypeFrame) -> &'static str {
    match frame {
        ArchetypeFrame::Colossus => "colossus",
        ArchetypeFrame::Striker => "striker",
        ArchetypeFrame::Caster => "caster",
        ArchetypeFrame::Infiltrator => "infiltrator",
    }
}

/// The preset a build is, by the content's name for it.
pub fn build_name(pack: Option<&ContentPack>, build: &Build) -> String {
    pack.and_then(|p| p.builds.iter().find(|b| &b.build == build))
        .map_or_else(|| "custom".to_string(), |b| b.name.clone())
}

fn played(seconds: u32) -> String {
    match seconds {
        0 => "new".into(),
        s if s < 3600 => format!("{} min", (s / 60).max(1)),
        s if s < 360_000 => format!("{} h {} m", s / 3600, s % 3600 / 60),
        s => format!("{} h", s / 3600),
    }
}

/// The abilities of a build by their content keys, as words.
fn abilities(pack: &ContentPack, b: &Build) -> String {
    let name = |i: u16| {
        pack.abilities
            .get(i as usize)
            .map_or_else(|| "?".to_string(), |a| a.key.replace('_', " "))
    };
    let mut all = vec![name(b.primary), name(b.secondary)];
    all.extend(b.guard.map(name));
    all.extend(b.actives.iter().map(|a| name(*a)));
    all.join(", ")
}

impl Front {
    /// The screens on `hub`. `email` fills the login form (the last one used); with `auto`
    /// the steps it names are taken at once.
    pub fn new(hub: Box<dyn HubApi>, email: String, prefer: String, auto: Option<Auto>) -> Front {
        let mut front = Front {
            hub,
            screen: Screen::Login,
            account: None,
            auto: None,
            wait: None,
            retry: None,
            notice: String::new(),
            busy: "",
            email,
            password: String::new(),
            again: String::new(),
            registering: false,
            show: false,
            characters: Vec::new(),
            picked: 0,
            prefer,
            content: None,
            blurbs: Vec::new(),
            content_asked: None,
            name: String::new(),
            archetype: 0,
            entering: String::new(),
        };
        if let Some(auto) = auto {
            front.email = auto.email.clone();
            front.password = auto.password.clone();
            let step = if auto.register {
                Step::Register
            } else {
                Step::Login
            };
            front.auto = Some(auto);
            front.sign_in(step);
        }
        front
    }

    /// Whether nobody is clicking: the command line named the steps.
    pub fn on_autopilot(&self) -> bool {
        self.auto.is_some()
    }

    /// The character picked, by name.
    #[cfg(test)]
    fn picked_name(&self) -> Option<&str> {
        self.characters.get(self.picked).map(|c| c.name.as_str())
    }

    fn send(&mut self, step: Step, req: PlayerRequest) {
        self.wait = Some((step, self.hub.call(req)));
    }

    /// The same request again in a moment, `left` more times after this one.
    fn again_later(&mut self, step: Step, req: PlayerRequest, left: u32) {
        self.retry = Some((Instant::now() + RETRY_EVERY, step, req, left));
    }

    /// The notice the login screen shows (a browser's page shows it by its own form).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn notice(&self) -> &str {
        &self.notice
    }

    /// Whether the hub is being asked something.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn busy(&self) -> bool {
        self.wait.is_some() || self.retry.is_some()
    }

    /// The page's form was sent (CLIENT.md 4.1): what the login screen's button does.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn page_login(&mut self, email: String, password: String, register: bool) {
        if self.screen != Screen::Login || self.busy() {
            return;
        }
        self.email = email;
        self.password = password;
        self.sign_in(if register {
            Step::Register
        } else {
            Step::Login
        });
    }

    /// A request a person waits for: the last refusal goes, the screen says it is asking.
    fn ask(&mut self, busy: &'static str) {
        self.notice.clear();
        self.busy = busy;
    }

    fn sign_in(&mut self, step: Step) {
        let (email, password) = (self.email.trim().to_string(), self.password.clone());
        // Nothing of an earlier session is asked again under this one.
        self.retry = None;
        self.ask("asking the hub");
        self.send(
            step,
            match step {
                Step::Register => PlayerRequest::Register { email, password },
                _ => PlayerRequest::Login { email, password },
            },
        );
    }

    /// The session requests are made on. (Asked without one, the hub refuses, and that
    /// refusal leads back to the login.)
    fn session(&self) -> SessionId {
        self.account
            .as_ref()
            .map_or(SessionId([0; 16]), |a| a.session)
    }

    fn enter(&mut self, character: CharacterId, zone: String) {
        self.ask("asking the hub for a way in");
        let session = self.session();
        let req = PlayerRequest::Enter {
            session,
            character,
            zone,
        };
        // A person's entry is tried again while the zone left is still saving.
        if self.auto.is_none() {
            self.retry = Some((Instant::now(), Step::Enter, req, RETRIES));
            self.retry_now();
        } else {
            self.send(Step::Enter, req);
        }
    }

    /// Make the retry that is due.
    fn retry_now(&mut self) {
        if self.wait.is_none()
            && let Some((at, step, req, left)) = self.retry.take()
        {
            if Instant::now() < at {
                self.retry = Some((at, step, req, left));
                return;
            }
            // Kept, one try shorter, until its answer says whether it is needed again.
            self.retry = Some((at, step, req.clone(), left));
            self.send(step, req);
        }
    }

    fn create(&mut self, name: String, preset: String) {
        self.ask("asking the hub");
        let session = self.session();
        self.send(
            Step::Create,
            PlayerRequest::CreateCharacter {
                session,
                name,
                build: BuildChoice::Preset(preset),
            },
        );
    }

    /// The zone would not have the character, or the connection to it ended: back to the
    /// characters, with the reason.
    pub fn back_to_characters(&mut self, why: &str) {
        self.notice = why.to_string();
        self.entering.clear();
        self.retry = None;
        if self.account.is_some() {
            self.screen = Screen::Characters;
            // Where the character is and how long it played have changed, once the zone
            // has saved it: the list is asked for until nobody is in a zone any more.
            let session = self.session();
            let req = PlayerRequest::Characters { session };
            self.retry = Some((Instant::now(), Step::Characters, req, RETRIES));
            self.retry_now();
        } else {
            self.screen = Screen::Login;
        }
    }

    /// A refusal: on autopilot it ends the program; a person reads it and tries again.
    fn refused(&mut self, step: Step, e: RpcError) -> Action {
        let text = words(step, &e);
        if self.auto.is_some() {
            return Action::Failed(text);
        }
        self.notice = text;
        if e == RpcError::Refused(HubError::Unauthorized) && self.account.take().is_some() {
            // The session ended: whatever else was asked or queued under it is void.
            self.wait = None;
            self.retry = None;
            self.busy = "";
            self.screen = Screen::Login;
            self.characters.clear();
            return Action::Session(None);
        }
        // (An answer that outlived its session changes no screen.)
        if step == Step::Enter && self.account.is_some() {
            self.screen = Screen::Characters;
        }
        // In a browser the page's form holds what was typed; here nothing keeps a
        // password that was refused.
        if cfg!(target_arch = "wasm32") && matches!(step, Step::Register | Step::Login) {
            self.password.clear();
            self.again.clear();
        }
        Action::None
    }

    /// The hub's answer to the request the machine waited on, if it has come.
    fn poll(&mut self) -> Action {
        self.retry_now();
        let Some((step, pending)) = &self.wait else {
            return Action::None;
        };
        let step = *step;
        let Some(answer) = pending.take() else {
            return Action::None;
        };
        self.wait = None;
        // What was asked may have to be asked again (see `RETRY_EVERY`).
        let retry = if self.retry.as_ref().is_some_and(|r| r.1 == step) {
            self.retry.take()
        } else {
            None
        };
        let again = match (&retry, &answer) {
            (Some((_, Step::Enter, _, left)), Err(RpcError::Refused(HubError::Busy))) => *left > 0,
            (Some((_, Step::Characters, _, left)), Ok(PlayerResponse::Characters(list))) => {
                *left > 0 && list.iter().any(|c| c.location != LocationSummary::Offline)
            }
            _ => false,
        };
        if again && let Some((_, step, req, left)) = retry {
            if step == Step::Enter {
                self.busy = "the zone it left is still putting the character away";
            }
            self.again_later(step, req, left - 1);
            if step == Step::Enter {
                return Action::None;
            }
        } else if !matches!(self.retry, Some((_, Step::Enter, _, _))) {
            // (An entry asked for while the list was on its way is still to be made.)
            self.busy = "";
        }
        match (step, answer) {
            // On autopilot `--register` means "make the account if it is not there".
            (Step::Register, Err(RpcError::Refused(HubError::Taken))) if self.auto.is_some() => {
                self.sign_in(Step::Login);
                Action::None
            }
            (Step::Register | Step::Login, Ok(PlayerResponse::Session { session, .. })) => {
                let account = Account {
                    session,
                    email: self.email.trim().to_string(),
                };
                // The password has done its work; nothing keeps it.
                self.password.clear();
                self.again.clear();
                if let Some(auto) = &mut self.auto {
                    auto.password.clear();
                }
                self.registering = false;
                self.account = Some(account.clone());
                self.retry = None;
                self.send(Step::Characters, PlayerRequest::Characters { session });
                Action::Session(Some(account))
            }
            (Step::Characters, Ok(PlayerResponse::Characters(list))) => {
                // The row that was picked stays picked when the list comes again; the
                // first list picks the character played last.
                let was = self.characters.get(self.picked).map(|c| c.id);
                self.characters = list;
                let at = |found: Option<usize>| found.filter(|_| was.is_some());
                self.picked = at(self.characters.iter().position(|c| Some(c.id) == was))
                    .or_else(|| self.characters.iter().position(|c| c.name == self.prefer))
                    .unwrap_or(0);
                let session = self.session();
                match self.auto.clone() {
                    Some(auto) if !auto.character.is_empty() => {
                        let found = self
                            .characters
                            .iter()
                            .find(|c| c.name == auto.character)
                            .map(|c| c.id);
                        match found {
                            Some(id) => self.enter(id, auto.zone),
                            None => match auto.preset {
                                Some(preset) => self.create(auto.character, preset),
                                None => {
                                    return Action::Failed(
                                        "no such character; name a build to create it".into(),
                                    );
                                }
                            },
                        }
                    }
                    _ => {
                        // From here a person chooses. The first list after the login
                        // decides the screen; a list that was asked for again changes none
                        // (the person may be naming a new character by then).
                        self.auto = None;
                        if self.screen == Screen::Login {
                            self.screen = if self.characters.is_empty() {
                                Screen::NewCharacter
                            } else {
                                Screen::Characters
                            };
                        }
                        let _ = session;
                        self.ask_content();
                    }
                }
                Action::None
            }
            (Step::Content, Ok(PlayerResponse::Content { pack, blurbs })) => {
                self.content = Some(pack);
                self.blurbs = blurbs;
                Action::None
            }
            (Step::Create, Ok(PlayerResponse::Character(c))) => {
                let (id, zone) = (c.id, self.auto.as_ref().map(|a| a.zone.clone()));
                self.characters.push(c);
                self.picked = self.characters.len() - 1;
                self.name.clear();
                match zone {
                    Some(zone) => self.enter(id, zone),
                    None => self.screen = Screen::Characters,
                }
                Action::None
            }
            (Step::Enter, Ok(PlayerResponse::Ticket(ticket))) => {
                self.entering = ticket.zone.clone();
                self.screen = Screen::Entering;
                self.prefer = self
                    .characters
                    .iter()
                    .find(|c| c.id == ticket.token.payload.character)
                    .map_or_else(String::new, |c| c.name.clone());
                Action::Enter {
                    name: self.prefer.clone(),
                    ticket: Box::new(ticket),
                }
            }
            (step, Ok(_)) => self.refused(
                step,
                RpcError::Other("the hub answered something else".into()),
            ),
            (step, Err(e)) => self.refused(step, e),
        }
    }

    /// One frame: the hub's answer if it came, then the screen (drawn in every frame: a
    /// click that falls into the frame of an answer is not lost). `ui` was begun for
    /// `self.screen.name()`. What the answer asks of the app, then what the screen does.
    pub fn frame<C: Canvas>(&mut self, ui: &mut Ui<'_, C>) -> [Action; 2] {
        let answered = self.poll();
        // Nobody is clicking: say what is being waited for and draw no form.
        if self.auto.is_some() && self.screen != Screen::Entering {
            let s = ui.scale;
            let panel = Rect::centred(ui.size(), 200.0 * s, ui.panel_height(ui.line(), true));
            let inner = ui.panel(panel, "gamengine");
            ui.label(inner.x, inner.y, 0.0, ui::FAINT, self.busy);
            return [answered, Action::None];
        }
        let clicked = match self.screen {
            Screen::Login => self.login(ui),
            Screen::Characters => self.characters(ui),
            Screen::NewCharacter => self.new_character(ui),
            Screen::Entering => self.entering(ui),
        };
        [answered, clicked]
    }

    /// A person is waiting for an answer: the buttons that would ask another are off.
    /// The list being asked for again behind the screen is nothing a person waits for: a
    /// button that went grey for the length of it would swallow the click that met it.
    fn waiting(&self) -> bool {
        let behind = match &self.wait {
            Some((Step::Characters, _)) => {
                matches!(self.retry, Some((_, Step::Characters, _, _)))
            }
            // The archetypes are fetched behind the characters' screen as well.
            Some((Step::Content, _)) => true,
            _ => false,
        };
        (self.wait.is_some() && !behind) || matches!(self.retry, Some((_, Step::Enter, _, _)))
    }

    /// Ask for the content while it is missing: once a second, when nothing else is being
    /// asked (the first request may have been lost, and without the archetypes nobody
    /// can make a character).
    fn ask_content(&mut self) {
        let due = self
            .content_asked
            .is_none_or(|at| at.elapsed() >= RETRY_EVERY);
        if self.content.is_none() && self.wait.is_none() && self.account.is_some() && due {
            self.content_asked = Some(Instant::now());
            let session = self.session();
            self.send(Step::Content, PlayerRequest::Content { session });
        }
    }

    /// The line under a screen's fields: what is being asked, else the last refusal.
    /// Drawn after the screen's buttons were handled, so that it is never a frame late.
    fn notice_line<C: Canvas>(&self, ui: &mut Ui<'_, C>, r: Rect) {
        if self.waiting() && !self.busy.is_empty() {
            ui.paragraph(r, ui::FAINT, self.busy);
        } else if !self.notice.is_empty() {
            ui.paragraph(r, ui::WARN, &self.notice);
        }
    }

    fn login<C: Canvas>(&mut self, ui: &mut Ui<'_, C>) -> Action {
        // In a browser the page's own form takes the email and the password, so that the
        // browser can fill and remember them (CLIENT.md 4.1): nothing is drawn here, and
        // the app hands over what the form sent (`page_login`).
        if cfg!(target_arch = "wasm32") {
            return Action::None;
        }
        let s = ui.scale;
        let gap = 6.0 * s;
        let (field, line, tick) = (ui.field_height(), ui.line(), ui.line() + 2.0 * s);
        // Two fields, the box, the notice and the buttons; a new account has a third
        // field and a word about the address.
        let mut inner =
            2.0 * (field + gap) + (tick + gap) + (2.0 * line + gap) + ui.button_height();
        if self.registering {
            inner += field + gap + 2.0 * line + gap;
        }
        let panel = Rect::centred(ui.size(), PANEL_UNITS * s, ui.panel_height(inner, true));
        let inner = ui.panel(
            panel,
            if self.registering {
                "gamengine: a new account"
            } else {
                "gamengine"
            },
        );
        let mut col = Column::new(inner, gap);
        ui.focus_default(
            "field",
            if self.email.is_empty() {
                "email"
            } else {
                "password"
            },
        );
        let email = Field {
            no_spaces: true,
            ..Field::text(254)
        };
        ui.text_field(col.take(ui.field_height()), "email", &mut self.email, email);
        // A password is whatever the keyboard gives, drawable or not.
        let secret = Field {
            secret: !self.show,
            any: true,
            max_bytes: 256,
            ..Field::text(256)
        };
        ui.text_field(
            col.take(ui.field_height()),
            "password",
            &mut self.password,
            secret,
        );
        if self.registering {
            ui.text_field(
                col.take(ui.field_height()),
                "password again",
                &mut self.again,
                secret,
            );
            // Nobody checks the address, and nothing can be sent to it.
            ui.paragraph(
                col.take(2.0 * ui.line()),
                ui::FAINT,
                "The address is only a name: no mail is sent to it, and a lost password stays lost.",
            );
        }
        ui.checkbox(col.take(tick), "show the password", &mut self.show);
        let notice = col.take(2.0 * line);
        let free = !self.waiting();
        let (go, other) = if self.registering {
            ("Create account", "Back")
        } else {
            ("Log in", "New account")
        };
        let row = ui.buttons(col.take(ui.button_height()), &[go, other, "Quit"]);
        let mut submit = ui.button_if(row[0], go, free);
        if ui.button_if(row[1], other, free) {
            self.registering = !self.registering;
            self.again.clear();
            self.notice.clear();
        }
        if ui.button(row[2], "Quit") {
            return Action::Quit;
        }
        submit |= free && ui.key(Key::Enter);
        if self.registering && ui.key(Key::Escape) {
            self.registering = false;
        }
        if submit {
            let email = self.email.trim();
            if !email.contains('@') || email.len() < 3 {
                self.notice = "an email address, please".into();
            } else if self.password.chars().count() < 8 {
                self.notice = "a password is 8 characters or more".into();
            } else if self.registering && self.password != self.again {
                self.notice = "the two passwords differ".into();
            } else {
                self.sign_in(if self.registering {
                    Step::Register
                } else {
                    Step::Login
                });
            }
        }
        self.notice_line(ui, notice);
        Action::None
    }

    fn characters<C: Canvas>(&mut self, ui: &mut Ui<'_, C>) -> Action {
        let s = ui.scale;
        let gap = 6.0 * s;
        let list = ui.list_height(6);
        let inner = list + gap + 2.0 * ui.line() + gap + ui.button_height();
        let panel = Rect::centred(ui.size(), PANEL_UNITS * s, ui.panel_height(inner, true));
        let title = format!("characters of {}", self.email.trim());
        let inner = ui.panel(panel, &title);
        let mut col = Column::new(inner, gap);
        let rows: Vec<Vec<String>> = self
            .characters
            .iter()
            .map(|c| {
                vec![
                    c.name.clone(),
                    build_name(self.content.as_ref(), &c.build),
                    c.last_zone.clone().unwrap_or_else(|| "new".into()),
                    played(c.play_seconds),
                ]
            })
            .collect();
        ui.focus_default("list", "characters");
        let event = ui.list(
            col.take(list),
            "characters",
            &[0.0, 0.42, 0.63, 0.82],
            &rows,
            &mut self.picked,
        );
        let notice = col.take(2.0 * ui.line());
        // A browser's tab is closed by the browser.
        let web = cfg!(target_arch = "wasm32");
        let labels: &[&str] = if web {
            &["Play", "New character", "Log out"]
        } else {
            &["Play", "New character", "Log out", "Quit"]
        };
        let row = ui.buttons(col.take(ui.button_height()), labels);
        let free = !self.waiting();
        let any = !self.characters.is_empty();
        let mut play = ui.button_if(row[0], "Play", free && any);
        play |= free && any && event == ListEvent::Activated;
        // An account has room for so many, and none can be deleted yet.
        let room = (self.characters.len() as i64) < MAX_CHARACTERS_PER_ACCOUNT;
        if ui.button_if(row[1], "New character", free && room) {
            self.screen = Screen::NewCharacter;
            self.notice.clear();
        }
        if ui.button_if(row[2], "Log out", free) {
            return self.log_out();
        }
        if !web && ui.button(row[3], "Quit") {
            return Action::Quit;
        }
        // Enter that no button took is the screen's own: Play.
        play |= free && any && ui.key(Key::Enter);
        self.ask_content();
        if play && let Some(c) = self.characters.get(self.picked) {
            let id = c.id;
            self.enter(id, String::new());
        }
        self.notice_line(ui, notice);
        Action::None
    }

    /// End the session here and at the hub, and show the login.
    fn log_out(&mut self) -> Action {
        let session = self.session();
        // Nobody waits for the answer: the session is gone here either way. Nor for
        // anything else that was being asked.
        let _ = self.hub.call(PlayerRequest::Logout { session });
        self.account = None;
        self.characters.clear();
        self.wait = None;
        self.retry = None;
        self.busy = "";
        self.screen = Screen::Login;
        self.notice.clear();
        Action::Session(None)
    }

    fn new_character<C: Canvas>(&mut self, ui: &mut Ui<'_, C>) -> Action {
        let s = ui.scale;
        let gap = 5.0 * s;
        let line = ui.line();
        let list = ui.list_height(6);
        // The name, the archetypes, six lines about the one picked, two of notice, the
        // buttons.
        let inner = (ui.field_height() + gap)
            + (line + gap)
            + (list + gap)
            + (6.0 * line + gap)
            + (2.0 * line + gap)
            + ui.button_height();
        let panel = Rect::centred(ui.size(), PANEL_UNITS * s, ui.panel_height(inner, true));
        let inner = ui.panel(panel, "a new character");
        let mut col = Column::new(inner, gap);
        ui.focus_default("field", "name");
        // The hub counts a name in bytes (24 of them).
        let name = Field {
            max_bytes: 24,
            ..Field::text(24)
        };
        ui.text_field(col.take(ui.field_height()), "name", &mut self.name, name);
        let head = col.take(line);
        ui.label(head.x, head.y, 0.0, ui::FAINT, "archetype");
        let presets = self.content.as_ref().map_or(&[][..], |p| &p.builds[..]);
        let rows: Vec<Vec<String>> = presets
            .iter()
            .map(|b| {
                vec![
                    b.name.clone(),
                    frame_name(b.build.frame).to_string(),
                    b.build.armour.name().to_string(),
                ]
            })
            .collect();
        ui.list(
            col.take(list),
            "archetypes",
            &[0.0, 0.42, 0.74],
            &rows,
            &mut self.archetype,
        );
        // What the picked archetype is, in the content's words, and its facts.
        let about = col.take(6.0 * line);
        match (self.content.as_ref(), presets.get(self.archetype)) {
            (Some(pack), Some(b)) => {
                let blurb = self.blurbs.get(self.archetype).map_or("", String::as_str);
                let used = ui.paragraph(
                    Rect::new(about.x, about.y, about.w, 4.0 * line),
                    ui::TEXT,
                    blurb,
                );
                let aspects: Vec<&str> = b.build.aspects.iter().map(|e| e.name()).collect();
                let facts = format!("{}: {}", aspects.join(" and "), abilities(pack, &b.build));
                ui.paragraph(
                    Rect::new(about.x, about.y + used, about.w, about.h - used),
                    ui::FAINT,
                    &facts,
                );
            }
            _ => ui.label(
                about.x,
                about.y,
                about.w,
                ui::FAINT,
                "asking the hub for the archetypes",
            ),
        }
        let notice = col.take(2.0 * line);
        let web = cfg!(target_arch = "wasm32");
        // An account with no character has nowhere to go back to but out.
        let empty = self.characters.is_empty();
        let back = if empty { "Log out" } else { "Back" };
        let labels: &[&str] = if web {
            &["Create", back]
        } else {
            &["Create", back, "Quit"]
        };
        let row = ui.buttons(col.take(ui.button_height()), labels);
        let free = !self.waiting();
        let ready = free && presets.get(self.archetype).is_some();
        let mut create = ui.button_if(row[0], "Create", ready);
        if ui.button_if(row[1], back, free) || (!empty && ui.key(Key::Escape)) {
            if empty {
                return self.log_out();
            }
            self.screen = Screen::Characters;
            self.notice.clear();
        }
        if !web && ui.button(row[2], "Quit") {
            return Action::Quit;
        }
        // Enter that no button took is the screen's own: Create.
        create |= ready && ui.key(Key::Enter);
        let preset = presets.get(self.archetype).map(|p| p.name.clone());
        self.ask_content();
        if create && let Some(preset) = preset {
            // The hub's own rule, said before it is asked.
            match gm_hub_proto::names::character_name(&self.name) {
                Ok(name) => self.create(name, preset),
                Err(why) => self.notice = why.to_string(),
            }
        }
        self.notice_line(ui, notice);
        Action::None
    }

    fn entering<C: Canvas>(&mut self, ui: &mut Ui<'_, C>) -> Action {
        let s = ui.scale;
        let gap = 6.0 * s;
        let inner = ui.line() + gap + ui.button_height();
        let panel = Rect::centred(ui.size(), 200.0 * s, ui.panel_height(inner, true));
        let inner = ui.panel(panel, &format!("entering {}", self.entering));
        let mut col = Column::new(inner, gap);
        let line = col.take(ui.line());
        ui.label(line.x, line.y, line.w, ui::FAINT, "waiting for the zone");
        let row = ui.buttons(col.take(ui.button_height()), &["Cancel"]);
        if ui.button(row[0], "Cancel") || ui.key(Key::Escape) {
            // Whoever cancels is a person: from here they choose.
            self.auto = None;
            self.back_to_characters("");
            return Action::CancelEntering;
        }
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use gm_core::sim::test_content;
    use gm_core::tick::TickRate;
    use gm_hub_proto::protocol::{LocationSummary, SessionToken, TokenPayload};

    use super::*;
    use crate::ui::tests::Recorder;
    use crate::ui::{UiInput, UiState};

    /// A hub that answers what the test told it to, and remembers what it was asked.
    #[derive(Clone, Default)]
    struct Script {
        answers: Rc<RefCell<VecDeque<Answer>>>,
        asked: Rc<RefCell<Vec<PlayerRequest>>>,
        /// While set, what is asked is not answered: the answers wait in `held` for the
        /// test to give them, as a hub's do on a slow line.
        slow: Rc<std::cell::Cell<bool>>,
        held: Rc<RefCell<VecDeque<crate::hub::Filler<Answer>>>>,
    }

    impl Script {
        fn then(&self, answer: Answer) -> &Script {
            self.answers.borrow_mut().push_back(answer);
            self
        }
        fn asked(&self) -> Vec<PlayerRequest> {
            self.asked.borrow().clone()
        }
        /// The oldest request still waiting gets its answer.
        fn answer(&self, answer: Answer) {
            let waiting = self.held.borrow_mut().pop_front();
            waiting.expect("a request is waiting").fill(answer);
        }
    }

    impl HubApi for Script {
        fn call(&self, req: PlayerRequest) -> Pending<Answer> {
            self.asked.borrow_mut().push(req);
            if self.slow.get() {
                let (pending, filler) = Pending::new();
                self.held.borrow_mut().push_back(filler);
                return pending;
            }
            let answer = self
                .answers
                .borrow_mut()
                .pop_front()
                .unwrap_or(Err(RpcError::Other("the hub does not answer".into())));
            Pending::ready(answer)
        }
    }

    fn character(id: i64, name: &str, zone: Option<&str>) -> CharacterSummary {
        let pack = test_content::pack(TickRate::COMBAT);
        CharacterSummary {
            id,
            name: name.into(),
            build: pack.builds[1].build.clone(),
            location: LocationSummary::Offline,
            last_zone: zone.map(str::to_string),
            play_seconds: 4000,
            model: None,
        }
    }

    fn ticket(zone: &str, character: i64) -> ZoneTicket {
        ZoneTicket {
            zone: zone.into(),
            addr: "127.0.0.1:4433".parse().unwrap(),
            cert_der: Vec::new(),
            web: None,
            token: SessionToken {
                payload: TokenPayload {
                    account: 1,
                    character,
                    zone: zone.into(),
                    issued_at: 0,
                    expires_at: 0,
                    nonce: [0; 16],
                },
                signature: [0; 64],
            },
        }
    }

    /// The session the scripted hub hands out.
    const S: SessionId = SessionId([7; 16]);

    struct Run {
        front: Front,
        state: UiState,
        hub: Script,
        /// The frame's size, and what the last frame drew.
        size: (f32, f32),
        drawn: Recorder,
    }

    impl Run {
        fn new(auto: Option<Auto>, answers: Vec<Answer>) -> Run {
            let hub = Script::default();
            for a in answers {
                hub.then(a);
            }
            Run {
                front: Front::new(Box::new(hub.clone()), String::new(), String::new(), auto),
                state: UiState::default(),
                hub,
                size: (1280.0, 720.0),
                drawn: Recorder::default(),
            }
        }

        fn frame(&mut self, input: &UiInput) -> Action {
            let mut canvas = Recorder::new(self.size.0, self.size.1);
            let mut ui = Ui::begin(
                &mut canvas,
                &mut self.state,
                input,
                self.front.screen.name(),
                PANEL_UNITS,
            );
            let actions = self.front.frame(&mut ui);
            ui.end();
            self.drawn = canvas;
            // What an answer asked for, or what the screen did: the tests here never
            // have both in one frame.
            match actions {
                [Action::None, clicked] => clicked,
                [answered, Action::None] => answered,
                both => panic!("two actions in one frame: {both:?}"),
            }
        }

        /// The screen as it stands is whole (see `ui::tests::tidy`).
        fn tidy(&mut self) {
            if self.front.wait.is_none() {
                self.frame(&UiInput::default());
            }
            crate::ui::tests::tidy(&self.drawn, &self.state, PANEL_UNITS);
        }

        /// Frames until the machine has nothing more to pick up; the actions on the way.
        fn settle(&mut self) -> Vec<Action> {
            let mut actions = Vec::new();
            for _ in 0..8 {
                match self.frame(&UiInput::default()) {
                    Action::None if self.front.wait.is_none() => break,
                    Action::None => {}
                    a => actions.push(a),
                }
            }
            actions
        }

        fn click(&mut self, text: &str) -> Action {
            let at = self
                .state
                .find(text)
                .unwrap_or_else(|| panic!("nothing on the screen says {text:?}: {:?}", self.said()))
                .rect
                .centre();
            let press = UiInput {
                cursor: at,
                pressed: true,
                down: true,
                ..Default::default()
            };
            let a = self.frame(&press);
            assert_eq!(a, Action::None);
            let action = self.frame(&UiInput {
                cursor: at,
                released: true,
                ..Default::default()
            });
            // What the click changed is on the screen a frame later; an answer that is
            // waited for is left for `settle`.
            if action == Action::None && self.front.wait.is_none() {
                assert_eq!(self.frame(&UiInput::default()), Action::None);
            }
            action
        }

        fn field(&mut self, label: &str, text: &str) {
            let a = self.click(label);
            assert_eq!(a, Action::None);
            self.frame(&UiInput {
                text: text.into(),
                ..Default::default()
            });
        }

        fn said(&self) -> Vec<String> {
            self.state.seen.iter().map(|s| s.text.clone()).collect()
        }

        fn says(&self, text: &str) -> bool {
            self.state.shows(text)
        }
    }

    fn session() -> Answer {
        Ok(PlayerResponse::Session {
            session: S,
            account: 1,
        })
    }

    fn refused(e: HubError) -> Answer {
        Err(RpcError::Refused(e))
    }

    fn content() -> Answer {
        let pack = test_content::pack(TickRate::COMBAT);
        let mut blurbs = vec![String::new(); pack.builds.len()];
        blurbs[0] = "A wall of plate behind a shield.".into();
        blurbs[2] = "Ice from a distance.".into();
        Ok(PlayerResponse::Content { pack, blurbs })
    }

    #[test]
    fn a_person_logs_in_picks_a_character_and_gets_a_ticket() {
        let mut run = Run::new(
            None,
            vec![
                session(),
                Ok(PlayerResponse::Characters(vec![
                    character(5, "Aldric", Some("arena")),
                    character(6, "Brena", None),
                ])),
                content(),
                Ok(PlayerResponse::Ticket(ticket("arena", 6))),
            ],
        );
        run.frame(&UiInput::default());
        assert_eq!(run.front.screen, Screen::Login);
        run.field("email", "someone@example.com");
        run.field("password", "a long password");
        assert_eq!(run.click("Log in"), Action::None);
        let actions = run.settle();
        assert_eq!(
            actions,
            [Action::Session(Some(Account {
                session: S,
                email: "someone@example.com".into()
            }))]
        );
        assert_eq!(run.front.screen, Screen::Characters);
        assert!(run.front.password.is_empty(), "the password is not kept");
        // The list says who they are, what they are, where and for how long.
        assert!(
            run.says("Aldric  blade  arena  1 h 6 m"),
            "{:?}",
            run.said()
        );
        assert!(run.says("Brena  blade  new  1 h 6 m"));
        // The second one, by a click; Play asks for no zone: the hub knows where it was.
        run.click("Brena");
        assert_eq!(run.click("Play"), Action::None);
        let actions = run.settle();
        assert!(
            matches!(&actions[..], [Action::Enter { name, ticket }] if name == "Brena" && ticket.zone == "arena"),
            "{actions:?}"
        );
        assert_eq!(run.front.screen, Screen::Entering);
        assert!(run.says("entering arena"));
        let asked = run.hub.asked();
        assert!(matches!(
            &asked[..],
            [
                PlayerRequest::Login { email, password },
                PlayerRequest::Characters { session: S },
                PlayerRequest::Content { session: S },
                PlayerRequest::Enter { session: S, character: 6, zone },
            ] if email == "someone@example.com" && password == "a long password" && zone.is_empty()
        ));
    }

    #[test]
    fn what_the_form_can_tell_is_said_before_the_hub_is_asked() {
        let mut run = Run::new(None, vec![]);
        run.frame(&UiInput::default());
        run.click("Log in");
        assert!(run.says("an email address, please"));
        run.field("email", "someone@example.com");
        run.field("password", "short");
        run.click("Log in");
        assert!(run.says("a password is 8 characters or more"));
        run.click("New account");
        assert!(run.says("password again"), "{:?}", run.said());
        run.field("password", "er and longer");
        run.field("password again", "something else");
        run.click("Create account");
        assert!(run.says("the two passwords differ"));
        assert!(run.hub.asked().is_empty(), "nothing was sent");
        run.click("Back");
        assert!(!run.says("password again"));
    }

    #[test]
    fn every_refusal_of_the_login_is_said_in_words() {
        let cases: Vec<(Answer, &str)> = vec![
            (refused(HubError::Credentials), "wrong email or password"),
            (refused(HubError::Busy), "too many attempts; wait a minute"),
            (
                refused(HubError::Banned {
                    until: 1_793_664_000,
                    reason: "aim assistance, replay 4711".into(),
                }),
                "this account is banned until 2026-11-03: aim assistance, replay 4711",
            ),
            (
                Err(RpcError::Other("the hub does not answer".into())),
                "the hub does not answer",
            ),
            (
                refused(HubError::Internal),
                "the hub had a problem; try again",
            ),
        ];
        for (answer, says) in cases {
            let mut run = Run::new(None, vec![answer]);
            run.frame(&UiInput::default());
            run.field("email", "someone@example.com");
            run.field("password", "a long password");
            run.click("Log in");
            assert_eq!(run.settle(), []);
            assert_eq!(run.front.screen, Screen::Login);
            assert!(run.says(says), "{says:?} not in {:?}", run.said());
            assert!(run.front.account.is_none());
        }
        // A new account whose email is taken.
        let mut run = Run::new(None, vec![refused(HubError::Taken)]);
        run.frame(&UiInput::default());
        run.click("New account");
        run.field("email", "someone@example.com");
        run.field("password", "a long password");
        run.field("password again", "a long password");
        run.click("Create account");
        run.settle();
        assert!(run.says("there is an account with this email already"));
        // A ban with no end in sight.
        assert_eq!(day(u64::MAX / 2), "for good");
        assert_eq!(day(0), "until 1970-01-01");
        assert_eq!(day(951_782_400), "until 2000-02-29");
    }

    #[test]
    fn an_account_without_characters_makes_one() {
        let mut run = Run::new(
            None,
            vec![
                session(),
                Ok(PlayerResponse::Characters(Vec::new())),
                content(),
                refused(HubError::Taken),
                Ok(PlayerResponse::Character(character(9, "Aldric", None))),
            ],
        );
        run.frame(&UiInput::default());
        run.field("email", "someone@example.com");
        run.field("password", "a long password");
        run.click("Log in");
        run.settle();
        assert_eq!(run.front.screen, Screen::NewCharacter);
        // The archetypes with the content's words, and no way back to an empty list.
        assert!(run.says("ironclad  colossus  plate"), "{:?}", run.said());
        assert!(run.says("A wall of plate behind a shield."));
        assert!(run.says("ground: greatsword, shield bash, shield wall, stomp, bellow"));
        assert!(
            run.state
                .find("Back")
                .is_none_or(|s| s.kind != ui::SeenKind::Button)
        );
        run.click("Create");
        assert!(run.says("a name is two letters or more"));
        run.field("name", "Zone");
        run.click("Create");
        assert!(run.says("that name is the game's own"));
        // The name is taken out again, by hand: End, then four times Backspace.
        run.click("name");
        run.frame(&UiInput {
            keys: vec![Key::End],
            ..Default::default()
        });
        for _ in 0..4 {
            run.frame(&UiInput {
                keys: vec![Key::Backspace],
                ..Default::default()
            });
        }
        run.click("frostweaver");
        assert!(run.says("Ice from a distance."));
        run.field("name", "Aldric");
        run.click("Create");
        run.settle();
        assert_eq!(run.front.screen, Screen::NewCharacter);
        assert!(run.says("that name is taken"));
        run.click("Create");
        run.settle();
        assert_eq!(run.front.screen, Screen::Characters);
        assert_eq!(run.front.picked_name(), Some("Aldric"));
        let asked = run.hub.asked();
        assert!(matches!(
            asked.last(),
            Some(PlayerRequest::CreateCharacter { session: S, name, build: BuildChoice::Preset(p) })
                if name == "Aldric" && p == "frostweaver"
        ));
    }

    #[test]
    fn a_zone_that_refuses_and_a_session_that_ended_lead_back() {
        let mut run = Run::new(
            None,
            vec![
                session(),
                Ok(PlayerResponse::Characters(vec![character(
                    5, "Aldric", None,
                )])),
                content(),
                refused(HubError::Locked("the trial warden_leader".into())),
                refused(HubError::Unauthorized),
            ],
        );
        run.frame(&UiInput::default());
        run.field("email", "someone@example.com");
        run.field("password", "a long password");
        run.click("Log in");
        run.settle();
        run.click("Play");
        run.settle();
        assert_eq!(run.front.screen, Screen::Characters);
        assert!(
            run.says("that zone asks for the trial warden_leader"),
            "{:?}",
            run.said()
        );
        // The session has run out at the hub meanwhile.
        run.click("Play");
        assert_eq!(run.settle(), [Action::Session(None)]);
        assert_eq!(run.front.screen, Screen::Login);
        assert!(run.says("the session ended; log in again"));
        // The zone's connection ending while it is being entered or played.
        let mut run = Run::new(
            None,
            vec![
                session(),
                Ok(PlayerResponse::Characters(vec![character(
                    5, "Aldric", None,
                )])),
                content(),
                Ok(PlayerResponse::Ticket(ticket("town", 5))),
                Ok(PlayerResponse::Characters(vec![character(
                    5,
                    "Aldric",
                    Some("town"),
                )])),
            ],
        );
        run.frame(&UiInput::default());
        run.field("email", "someone@example.com");
        run.field("password", "a long password");
        run.click("Log in");
        run.settle();
        run.click("Play");
        run.settle();
        assert_eq!(run.front.screen, Screen::Entering);
        run.front.back_to_characters("kicked: zone stopped");
        run.settle();
        assert_eq!(run.front.screen, Screen::Characters);
        assert!(run.says("kicked: zone stopped"));
        assert!(
            run.says("Aldric  blade  town"),
            "the list was asked for again"
        );
    }

    #[test]
    fn an_entry_is_tried_again_while_the_zone_left_is_still_saving() {
        let busy = || refused(HubError::Busy);
        let mut run = Run::new(
            None,
            vec![
                session(),
                Ok(PlayerResponse::Characters(vec![character(
                    5,
                    "Aldric",
                    Some("town"),
                )])),
                content(),
                busy(),
                busy(),
                Ok(PlayerResponse::Ticket(ticket("town", 5))),
            ],
        );
        run.frame(&UiInput::default());
        run.field("email", "someone@example.com");
        run.field("password", "a long password");
        run.click("Log in");
        run.settle();
        run.click("Play");
        // Refused twice as busy: said, waited out, asked again; nobody has to click.
        let mut actions = Vec::new();
        for _ in 0..200 {
            actions.extend(run.settle());
            if !actions.is_empty() {
                break;
            }
            assert!(
                run.says("the zone it left is still putting the character away"),
                "{:?}",
                run.said()
            );
            assert!(
                run.state.find("Play").is_none(),
                "no second Play while it waits"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(
            matches!(&actions[..], [Action::Enter { .. }]),
            "{actions:?}"
        );
        let enters = run
            .hub
            .asked()
            .iter()
            .filter(|r| matches!(r, PlayerRequest::Enter { .. }))
            .count();
        assert_eq!(enters, 3);

        // A character still listed as in its zone: the list is asked for again by itself.
        let mut away = character(5, "Aldric", Some("town"));
        away.location = LocationSummary::Zone("town".into());
        let mut run = Run::new(
            None,
            vec![
                session(),
                Ok(PlayerResponse::Characters(vec![character(
                    5, "Aldric", None,
                )])),
                content(),
                Ok(PlayerResponse::Characters(vec![away])),
                Ok(PlayerResponse::Characters(vec![character(
                    5,
                    "Aldric",
                    Some("arena"),
                )])),
            ],
        );
        run.frame(&UiInput::default());
        run.field("email", "someone@example.com");
        run.field("password", "a long password");
        run.click("Log in");
        run.settle();
        run.front.back_to_characters("");
        run.settle();
        assert!(run.says("Aldric  blade  town"));
        std::thread::sleep(std::time::Duration::from_millis(20));
        run.settle();
        assert!(run.says("Aldric  blade  arena"), "{:?}", run.said());
    }

    #[test]
    fn the_list_asked_for_again_behind_the_screen_gets_in_nobodys_way() {
        let mut away = character(5, "Aldric", Some("town"));
        away.location = LocationSummary::Zone("town".into());
        let list = |first: &CharacterSummary| {
            Ok(PlayerResponse::Characters(vec![
                first.clone(),
                character(6, "Brena", None),
            ]))
        };
        let mut run = Run::new(
            None,
            vec![session(), list(&character(5, "Aldric", None)), content()],
        );
        run.frame(&UiInput::default());
        run.field("email", "someone@example.com");
        run.field("password", "a long password");
        run.click("Log in");
        run.settle();
        run.click("Brena");
        // Back from a zone, on a slow line: the list is on its way again.
        run.hub.slow.set(true);
        run.front.back_to_characters("");
        run.frame(&UiInput::default());
        assert!(run.front.wait.is_some(), "the list was asked for");
        // Play is there to be clicked meanwhile, and the click is not lost: the entry is
        // asked for as soon as the list has come.
        assert_eq!(run.click("Play"), Action::None);
        run.frame(&UiInput::default());
        assert!(run.says("asking the hub for a way in"), "{:?}", run.said());
        assert!(run.state.find("Play").is_none(), "one entry at a time");
        run.hub.answer(list(&away));
        run.frame(&UiInput::default());
        run.frame(&UiInput::default());
        assert!(
            matches!(
                run.hub.asked().last(),
                Some(PlayerRequest::Enter { character: 6, .. })
            ),
            "{:?}",
            run.hub.asked().last()
        );
        // The row that was picked is still the one picked.
        assert_eq!(run.front.picked_name(), Some("Brena"));
        run.hub
            .answer(Ok(PlayerResponse::Ticket(ticket("town", 6))));
        let actions = run.settle();
        assert!(matches!(&actions[..], [Action::Enter { name, .. }] if name == "Brena"));

        // A list that comes while a new character is being named leaves that screen up.
        run.front.back_to_characters("");
        run.frame(&UiInput::default());
        run.click("New character");
        assert_eq!(run.front.screen, Screen::NewCharacter);
        run.hub.answer(list(&away));
        run.frame(&UiInput::default());
        assert_eq!(run.front.screen, Screen::NewCharacter);
        run.click("Back");

        // Logging out while the list is on its way (it is asked for again in a moment:
        // Aldric was still away): its answer is nobody's any more.
        for _ in 0..500 {
            run.frame(&UiInput::default());
            if run.front.wait.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(run.front.wait.is_some(), "the list was asked for again");
        assert_eq!(run.click("Log out"), Action::Session(None));
        // (The list is the oldest request waiting, then the logout.)
        run.hub.answer(list(&away));
        run.hub.answer(Ok(PlayerResponse::Ok));
        run.frame(&UiInput::default());
        assert_eq!(run.front.screen, Screen::Login);
        assert!(run.front.characters.is_empty());
    }

    /// Tab until the keyboard is on `widget`.
    fn tab_to(run: &mut Run, widget: &str) {
        for _ in 0..12 {
            if run.state.focus() == Some(widget) {
                return;
            }
            run.frame(&UiInput {
                keys: vec![Key::Tab],
                ..Default::default()
            });
        }
        panic!(
            "the keyboard never reached {widget}: it is on {:?}",
            run.state.focus()
        );
    }

    fn enter() -> UiInput {
        UiInput {
            keys: vec![Key::Enter],
            ..Default::default()
        }
    }

    #[test]
    fn enter_presses_the_button_that_has_the_keyboard() {
        let mut run = Run::new(
            None,
            vec![
                session(),
                Ok(PlayerResponse::Characters(vec![character(
                    5, "Aldric", None,
                )])),
                content(),
                Ok(PlayerResponse::Ok),
            ],
        );
        run.frame(&UiInput::default());
        run.field("email", "someone@example.com");
        run.field("password", "a long password");
        run.click("Log in");
        run.settle();
        // On the new character's screen with the keyboard on Back: Enter goes back, and
        // makes no character.
        run.click("New character");
        run.field("name", "Brena");
        tab_to(&mut run, "button:Back");
        assert_eq!(run.frame(&enter()), Action::None);
        assert_eq!(run.front.screen, Screen::Characters);
        // On the characters' screen with the keyboard on Log out: Enter logs out, and
        // nobody enters a zone.
        run.frame(&UiInput::default());
        tab_to(&mut run, "button:Log out");
        assert_eq!(run.frame(&enter()), Action::Session(None));
        assert_eq!(run.front.screen, Screen::Login);
        let asked = run.hub.asked();
        assert!(
            !asked.iter().any(|r| matches!(
                r,
                PlayerRequest::Enter { .. } | PlayerRequest::CreateCharacter { .. }
            )),
            "{asked:?}"
        );
        assert!(matches!(asked.last(), Some(PlayerRequest::Logout { .. })));
    }

    #[test]
    fn the_archetypes_are_asked_for_again_and_an_empty_account_can_log_out() {
        // The content is lost on its way once (the hub does not answer), then comes.
        let mut run = Run::new(
            None,
            vec![
                session(),
                Ok(PlayerResponse::Characters(Vec::new())),
                Err(RpcError::Other("the hub does not answer".into())),
                content(),
                Ok(PlayerResponse::Ok),
            ],
        );
        run.frame(&UiInput::default());
        run.field("email", "someone@example.com");
        run.field("password", "a long password");
        run.click("Log in");
        run.settle();
        assert_eq!(run.front.screen, Screen::NewCharacter);
        assert!(run.says("the hub does not answer"), "{:?}", run.said());
        for _ in 0..500 {
            run.frame(&UiInput::default());
            if run.says("ironclad") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(run.says("ironclad  colossus  plate"), "{:?}", run.said());
        // Nothing of ours is cut short on this screen, the longest refusal of a name
        // that the field lets through included.
        run.field("name", "Aldric!");
        run.click("Create");
        assert!(
            run.says("a name is made of letters, digits, spaces, hyphens and apostrophes"),
            "{:?}",
            run.said()
        );
        assert!(run.state.clipped.is_empty(), "{:?}", run.state.clipped);
        // No character, so nowhere to go back to: but out.
        assert!(run.state.find("Back").is_none());
        assert_eq!(run.click("Log out"), Action::Session(None));
        assert_eq!(run.front.screen, Screen::Login);
    }

    #[test]
    fn a_session_that_ended_takes_what_was_queued_under_it_along() {
        let mut away = character(5, "Aldric", Some("town"));
        away.location = LocationSummary::Zone("town".into());
        let mut run = Run::new(
            None,
            vec![
                session(),
                Ok(PlayerResponse::Characters(vec![character(
                    5, "Aldric", None,
                )])),
                content(),
            ],
        );
        run.frame(&UiInput::default());
        run.field("email", "someone@example.com");
        run.field("password", "a long password");
        run.click("Log in");
        run.settle();
        // The list is on its way again; Play is clicked meanwhile; and the list's answer
        // is that the session has ended.
        run.hub.slow.set(true);
        run.front.back_to_characters("");
        run.frame(&UiInput::default());
        assert_eq!(run.click("Play"), Action::None);
        run.hub.answer(refused(HubError::Unauthorized));
        assert_eq!(run.frame(&UiInput::default()), Action::Session(None));
        assert_eq!(run.front.screen, Screen::Login);
        // The entry that was queued is not made under a session that is no more, and
        // nothing brings the characters' screen back without an account.
        for _ in 0..5 {
            run.frame(&UiInput::default());
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
        assert_eq!(run.front.screen, Screen::Login);
        assert!(
            !run.hub
                .asked()
                .iter()
                .any(|r| matches!(r, PlayerRequest::Enter { .. })),
            "{:?}",
            run.hub.asked()
        );
        assert!(run.says("the session ended; log in again"));
        // A new login is not thrown out by what the old session left behind.
        run.hub.slow.set(false);
        run.hub
            .then(session())
            .then(Ok(PlayerResponse::Characters(vec![character(
                5, "Aldric", None,
            )])));
        run.field("password", "a long password");
        run.click("Log in");
        run.settle();
        for _ in 0..5 {
            run.frame(&UiInput::default());
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
        assert_eq!(run.front.screen, Screen::Characters);
        assert!(run.front.account.is_some());
    }

    #[test]
    fn an_account_has_room_for_ten_and_a_password_can_be_shown() {
        let ten: Vec<CharacterSummary> = (0..10)
            .map(|i| character(i, &format!("Name{i}"), None))
            .collect();
        let mut run = Run::new(
            None,
            vec![session(), Ok(PlayerResponse::Characters(ten)), content()],
        );
        run.frame(&UiInput::default());
        run.field("email", "someone@example.com");
        // A password is whatever the keyboard gives; shown only when asked.
        run.field("password", "pa\u{df}w\u{f6}rt \u{20ac}1");
        assert!(!run.says("w\u{f6}rt"));
        run.click("show the password");
        assert!(run.says("password: pa\u{df}w\u{f6}rt"), "{:?}", run.said());
        run.click("show the password");
        run.click("Log in");
        run.settle();
        assert_eq!(run.front.screen, Screen::Characters);
        assert!(matches!(
            &run.hub.asked()[0],
            PlayerRequest::Login { password, .. } if password == "pa\u{df}w\u{f6}rt \u{20ac}1"
        ));
        assert!(run.state.find("New character").is_none(), "ten is the most");
        assert!(run.state.find("Play").is_some());
    }

    #[test]
    fn every_screen_is_whole_at_every_size() {
        // The longest of everything: names of 24 wide letters, an address as long as a
        // field takes, a blurb of 160 characters, a refusal of two lines.
        let wide = "W".repeat(24);
        let long_blurb = "word ".repeat(32).trim_end().to_string();
        assert_eq!(long_blurb.len(), 159);
        for size in crate::ui::tests::SIZES {
            let ten: Vec<CharacterSummary> = (0..10)
                .map(|i| {
                    character(
                        i,
                        &format!("{wide}{i}")[1..],
                        Some("a-zone-with-a-long-name"),
                    )
                })
                .collect();
            let pack = test_content::pack(TickRate::COMBAT);
            let blurbs = vec![long_blurb.clone(); pack.builds.len()];
            let mut run = Run::new(
                None,
                vec![
                    refused(HubError::Banned {
                        until: 1_793_664_000,
                        reason: "a reason as long as a moderator cares to make it, and longer"
                            .into(),
                    }),
                    session(),
                    Ok(PlayerResponse::Characters(ten)),
                    Ok(PlayerResponse::Content { pack, blurbs }),
                    Ok(PlayerResponse::Ticket(ticket("a-zone-with-a-long-name", 3))),
                ],
            );
            run.size = size;
            run.frame(&UiInput::default());
            run.tidy();
            run.click("New account");
            run.tidy();
            run.click("Create account");
            run.tidy();
            run.click("Back");
            run.field("email", &format!("{}@example.com", "m".repeat(60)));
            run.field("password", "a long password");
            run.click("show the password");
            run.click("Log in");
            run.settle();
            assert!(run.says("this account is banned"), "{:?}", run.said());
            run.tidy();
            run.click("Log in");
            run.settle();
            assert_eq!(run.front.screen, Screen::Characters);
            run.tidy();
            // The new character's screen, with the list at its fullest.
            run.front.screen = Screen::NewCharacter;
            run.frame(&UiInput::default());
            run.click("Create");
            assert!(run.says("a name is two letters or more"));
            run.field("name", &wide);
            run.tidy();
            run.front.screen = Screen::Characters;
            run.frame(&UiInput::default());
            run.click("Play");
            run.settle();
            assert_eq!(run.front.screen, Screen::Entering);
            run.tidy();
        }
    }

    #[test]
    fn the_command_line_is_the_screens_with_nobody_clicking() {
        let auto = Auto {
            email: "bot@example.com".into(),
            password: "a long password".into(),
            register: true,
            character: "Aldric".into(),
            preset: Some("blade".into()),
            zone: "arena".into(),
        };
        // The account exists, the character does not.
        let mut run = Run::new(
            Some(auto.clone()),
            vec![
                refused(HubError::Taken),
                session(),
                Ok(PlayerResponse::Characters(Vec::new())),
                Ok(PlayerResponse::Character(character(9, "Aldric", None))),
                Ok(PlayerResponse::Ticket(ticket("arena", 9))),
            ],
        );
        let actions = run.settle();
        assert!(
            matches!(&actions[..], [Action::Session(Some(_)), Action::Enter { name, .. }] if name == "Aldric"),
            "{actions:?}"
        );
        let asked = run.hub.asked();
        assert!(matches!(
            &asked[..],
            [
                PlayerRequest::Register { .. },
                PlayerRequest::Login { .. },
                PlayerRequest::Characters { .. },
                PlayerRequest::CreateCharacter { .. },
                PlayerRequest::Enter { zone, .. },
            ] if zone == "arena"
        ));
        // A refusal ends it, in the hub's words.
        let mut run = Run::new(
            Some(Auto {
                register: false,
                ..auto.clone()
            }),
            vec![refused(HubError::Credentials)],
        );
        assert_eq!(
            run.settle(),
            [Action::Failed("wrong email or password".into())]
        );
        // Somebody cancels while the zone is being entered: from there they choose, and
        // the program goes on.
        let mut run = Run::new(
            Some(auto.clone()),
            vec![
                session(),
                Ok(PlayerResponse::Characters(vec![character(
                    5, "Aldric", None,
                )])),
                Ok(PlayerResponse::Ticket(ticket("arena", 5))),
                Ok(PlayerResponse::Characters(vec![character(
                    5, "Aldric", None,
                )])),
                content(),
            ],
        );
        let actions = run.settle();
        assert!(matches!(
            &actions[..],
            [Action::Session(Some(_)), Action::Enter { .. }]
        ));
        assert_eq!(run.front.screen, Screen::Entering);
        run.frame(&UiInput::default());
        assert_eq!(run.click("Cancel"), Action::CancelEntering);
        assert_eq!(run.settle(), []);
        assert!(!run.front.on_autopilot());
        assert_eq!(run.front.screen, Screen::Characters);
        assert!(run.state.find("Play").is_some(), "{:?}", run.said());
        // No character named: the autopilot stops at the list.
        let mut run = Run::new(
            Some(Auto {
                character: String::new(),
                register: false,
                ..auto
            }),
            vec![
                session(),
                Ok(PlayerResponse::Characters(vec![character(
                    5, "Aldric", None,
                )])),
                content(),
            ],
        );
        let actions = run.settle();
        assert!(matches!(&actions[..], [Action::Session(Some(_))]));
        assert_eq!(run.front.screen, Screen::Characters);
        assert!(!run.front.on_autopilot());
        assert!(run.says("Aldric"));
    }
}
