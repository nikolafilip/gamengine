//! UI scripts (CLIENT.md 9): what a person would do, one line at a time, handed to the app
//! through the same entry points as real events. A gate drives the screens with one; a bug
//! report can carry one.
//!
//! ```text
//! wait screen characters        until that screen is up
//! field email                   give the field with that label the keyboard
//! type someone@example.com      characters, as if typed
//! key Enter                     Enter | Escape | Tab | Backspace | Up | Down ...
//! click "New character"         the button, row or box with that text: pressed and let
//!                               go within one frame, so that nothing changes under it
//! dclick "Aldric"               the same, twice
//! expect "Aldric"               some text on the screen contains it
//! say at the characters         print `ui-script: at the characters`
//! where "Play"                  print `ui-script: where X Y Play`: its middle, in pixels
//! sleep 2
//! quit
//! ```
//!
//! `say` and `where` are for whoever runs the client from outside and sends it real input
//! (a gate with `xdotool`, a browser driven over its debugging port): they say when the
//! screen is ready for it, and where to click.

use web_time::{Duration, Instant};

use crate::ui::{Key, UiState};

/// How long a line may wait for the screen to show what it needs: longer than the
/// screens themselves keep trying (an entry is asked for twenty times, a second apart).
const PATIENCE: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq)]
enum Cmd {
    WaitScreen(String),
    Type(String),
    Key(Key),
    /// A click on what says this; twice for a double click.
    Click(String, bool),
    Expect(String),
    Say(String),
    Where(String),
    Sleep(f32),
    Quit,
}

/// What the script does this frame, in the app's terms.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The pointer goes here and the left button goes down (the second of a double click
    /// says so).
    Press {
        at: (f32, f32),
        double: bool,
    },
    Release,
    Text(String),
    Key(Key),
    /// A line for whoever runs the client.
    Say(String),
    Quit,
}

pub struct UiScript {
    cmds: Vec<(usize, Cmd)>,
    at: usize,
    /// When the line being run began to wait.
    since: Option<Instant>,
    /// A double click is two clicks, a frame apart: which one is next.
    phase: u8,
    /// Frames to let pass before the next line: what a line did is on the screen only a
    /// frame later, and the next line must act on that screen, not on the one before.
    rest: u8,
}

/// The text of a line's argument: in double quotes or bare.
fn argument(rest: &str) -> &str {
    let rest = rest.trim();
    rest.strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .unwrap_or(rest)
}

impl UiScript {
    pub fn parse(text: &str) -> Result<UiScript, String> {
        let mut cmds = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            let bad = |why: &str| format!("ui-script line {}: {why}: {line:?}", n + 1);
            let text = || {
                let t = argument(rest);
                if t.is_empty() {
                    Err(bad("says nothing"))
                } else {
                    Ok(t.to_string())
                }
            };
            let cmd = match word {
                "wait" => {
                    let name = rest
                        .trim()
                        .strip_prefix("screen")
                        .ok_or_else(|| bad("only a screen can be waited for"))?;
                    Cmd::WaitScreen(argument(name).to_string())
                }
                // A field is given the keyboard by a click on it.
                "field" | "click" => Cmd::Click(text()?, false),
                "dclick" => Cmd::Click(text()?, true),
                // Quoted when it begins or ends with a space.
                "type" => Cmd::Type(text()?),
                "key" => Cmd::Key(Key::parse(rest.trim()).ok_or_else(|| bad("no such key"))?),
                "expect" => Cmd::Expect(text()?),
                "say" => Cmd::Say(text()?),
                "where" => Cmd::Where(text()?),
                "sleep" => Cmd::Sleep(
                    rest.trim()
                        .parse()
                        .ok()
                        .filter(|s: &f32| s.is_finite() && *s >= 0.0)
                        .ok_or_else(|| bad("not a number of seconds"))?,
                ),
                "quit" => Cmd::Quit,
                _ => return Err(bad("no such command")),
            };
            cmds.push((n + 1, cmd));
        }
        Ok(UiScript {
            cmds,
            at: 0,
            since: None,
            phase: 0,
            rest: 0,
        })
    }

    #[cfg(test)]
    fn done(&self) -> bool {
        self.at >= self.cmds.len()
    }

    fn next(&mut self) {
        self.at += 1;
        self.since = None;
        self.phase = 0;
        self.rest = 2;
    }

    /// One frame: what the script does now, given what the last frame showed. An error is
    /// the line that waited in vain, with what the screen showed instead.
    pub fn step(&mut self, ui: &UiState, now: Instant) -> Result<Vec<Event>, String> {
        if self.rest > 0 {
            self.rest -= 1;
            return Ok(Vec::new());
        }
        let Some((line, cmd)) = self.cmds.get(self.at).cloned() else {
            return Ok(Vec::new());
        };
        let since = *self.since.get_or_insert(now);
        let waited = now.duration_since(since);
        let gave_up = |what: String| {
            let seen: Vec<&str> = ui.seen.iter().map(|s| s.text.as_str()).collect();
            Err(format!(
                "ui-script line {line}: {what}; the screen {:?} showed {seen:?}",
                ui.screen
            ))
        };
        match cmd {
            Cmd::WaitScreen(name) => {
                if ui.screen == name {
                    self.next();
                } else if waited > PATIENCE {
                    return gave_up(format!("the screen {name:?} did not come up"));
                }
                Ok(Vec::new())
            }
            Cmd::Expect(text) => {
                if ui.shows(&text) {
                    self.next();
                } else if waited > PATIENCE {
                    return gave_up(format!("nothing said {text:?}"));
                }
                Ok(Vec::new())
            }
            Cmd::Click(text, twice) => match ui.find(&text) {
                // The press and the release in one frame: what is clicked cannot go
                // away between them. (Real input, a frame or more apart, is the gate's
                // to send: CLIENT.md 10.)
                Some(seen) => {
                    let double = self.phase == 1;
                    self.phase += 1;
                    if self.phase == if twice { 2 } else { 1 } {
                        self.next();
                    }
                    Ok(vec![
                        Event::Press {
                            at: seen.rect.centre(),
                            double,
                        },
                        Event::Release,
                    ])
                }
                None if waited > PATIENCE => gave_up(format!("nothing to click says {text:?}")),
                None => Ok(Vec::new()),
            },
            Cmd::Say(text) => {
                self.next();
                Ok(vec![Event::Say(text)])
            }
            Cmd::Where(text) => match ui.find(&text) {
                Some(seen) => {
                    let (x, y) = seen.rect.centre();
                    self.next();
                    Ok(vec![Event::Say(format!("where {x:.0} {y:.0} {text}"))])
                }
                None if waited > PATIENCE => gave_up(format!("nothing says {text:?}")),
                None => Ok(Vec::new()),
            },
            Cmd::Type(text) => {
                self.next();
                Ok(vec![Event::Text(text)])
            }
            Cmd::Key(key) => {
                self.next();
                Ok(vec![Event::Key(key)])
            }
            Cmd::Sleep(secs) => {
                if waited.as_secs_f32() >= secs {
                    self.next();
                }
                Ok(Vec::new())
            }
            Cmd::Quit => {
                self.next();
                Ok(vec![Event::Quit])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{Rect, Seen, SeenKind};

    fn screen(name: &str, seen: &[(&str, SeenKind)]) -> UiState {
        let mut state = UiState::default();
        state.screen = name.into();
        state.seen = seen
            .iter()
            .enumerate()
            .map(|(i, (text, kind))| Seen {
                kind: *kind,
                text: text.to_string(),
                rect: Rect::new(100.0, 40.0 * i as f32, 80.0, 20.0),
            })
            .collect();
        state
    }

    #[test]
    fn a_script_is_read_line_by_line_and_nonsense_is_refused_with_its_line() {
        let s = UiScript::parse(
            "# a comment\n\nwait screen login\nfield email\ntype  a b \nkey Enter\n\
             click \"New character\"\ndclick Aldric\nexpect \"in the arena\"\nsay ready\n\
             where \"Play\"\nsleep 1.5\nquit\n",
        )
        .unwrap();
        let cmds: Vec<Cmd> = s.cmds.iter().map(|c| c.1.clone()).collect();
        assert_eq!(
            cmds,
            [
                Cmd::WaitScreen("login".into()),
                Cmd::Click("email".into(), false),
                Cmd::Type("a b".into()),
                Cmd::Key(Key::Enter),
                Cmd::Click("New character".into(), false),
                Cmd::Click("Aldric".into(), true),
                Cmd::Expect("in the arena".into()),
                Cmd::Say("ready".into()),
                Cmd::Where("Play".into()),
                Cmd::Sleep(1.5),
                Cmd::Quit,
            ]
        );
        assert_eq!(s.cmds[0].0, 3, "lines are counted as the file has them");
        for bad in [
            "dance",
            "key Hyper",
            "sleep long",
            "wait for it",
            "click",
            "sleep -1",
        ] {
            let e = UiScript::parse(bad)
                .err()
                .unwrap_or_else(|| panic!("{bad:?} was accepted"));
            assert!(e.starts_with("ui-script line 1"), "{e}");
        }
    }

    /// The script's next act: the frames it rests between lines are let pass.
    fn act(s: &mut UiScript, ui: &UiState, now: Instant) -> Result<Vec<Event>, String> {
        while s.rest > 0 {
            assert_eq!(s.step(ui, now), Ok(vec![]));
        }
        s.step(ui, now)
    }

    #[test]
    fn a_script_waits_for_the_screen_clicks_what_it_names_and_gives_up_in_words() {
        let mut s =
            UiScript::parse("wait screen login\nclick \"Log in\"\ndclick Aldric\ntype x\nquit")
                .unwrap();
        let t0 = Instant::now();
        let boot = screen("", &[]);
        assert_eq!(act(&mut s, &boot, t0), Ok(vec![]), "not there yet: wait");
        let login = screen(
            "login",
            &[
                ("Log in to play", SeenKind::Label),
                ("Log in", SeenKind::Button),
            ],
        );
        assert_eq!(act(&mut s, &login, t0), Ok(vec![]));
        // After a line the script lets two frames pass: what the line did is on the
        // screen only then, and the next line must act on that.
        assert_eq!(s.step(&login, t0), Ok(vec![]));
        assert_eq!(s.step(&login, t0), Ok(vec![]));
        // The button, not the label that also says it; pressed and let go in one frame.
        let at = (140.0, 50.0);
        assert_eq!(
            s.step(&login, t0),
            Ok(vec![Event::Press { at, double: false }, Event::Release])
        );
        // A double click: two clicks a frame apart, the second marked as the second.
        let list = screen("characters", &[("Aldric  blade", SeenKind::Row)]);
        let at = (140.0, 10.0);
        assert_eq!(
            act(&mut s, &list, t0),
            Ok(vec![Event::Press { at, double: false }, Event::Release])
        );
        assert_eq!(
            s.step(&list, t0),
            Ok(vec![Event::Press { at, double: true }, Event::Release])
        );
        assert_eq!(act(&mut s, &list, t0), Ok(vec![Event::Text("x".into())]));
        assert_eq!(act(&mut s, &list, t0), Ok(vec![Event::Quit]));
        assert!(s.done());
        assert_eq!(act(&mut s, &list, t0), Ok(vec![]));

        // What the one outside is told: a word, and where something is.
        let mut s = UiScript::parse("say the list is up\nwhere Aldric\nwhere Brena").unwrap();
        assert_eq!(
            act(&mut s, &list, t0),
            Ok(vec![Event::Say("the list is up".into())])
        );
        assert_eq!(
            act(&mut s, &list, t0),
            Ok(vec![Event::Say("where 140 10 Aldric".into())])
        );
        assert_eq!(act(&mut s, &list, t0), Ok(vec![]), "not there: wait");

        // What never shows up is given up on, with the line and what was there instead.
        let mut s = UiScript::parse("expect \"Brena\"").unwrap();
        assert_eq!(s.step(&list, t0), Ok(vec![]));
        let late = t0 + PATIENCE + Duration::from_secs(1);
        let e = s.step(&list, late).unwrap_err();
        assert!(
            e.contains("line 1") && e.contains("Brena") && e.contains("Aldric  blade"),
            "{e}"
        );
    }
}
