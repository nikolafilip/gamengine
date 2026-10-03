//! The browser plays (SOUND.md 5): one `AudioContext`, every patch an `AudioBuffer`, a
//! pool of thirty-two chains (a gain into a panner into the master) made once, and a cue
//! a source node started into a free chain. The browser mixes on its own thread; the
//! page only makes source nodes (the one node the Web Audio API will not let a page
//! reuse). Nothing of the native mixer is here.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{
    AnalyserNode, AudioBuffer, AudioBufferSourceNode, AudioContext, AudioContextState, AudioParam,
    AudioScheduledSourceNode, GainNode, StereoPannerNode,
};

use super::mixer::{Command, LOOP_SLOTS, VOICES};
use super::synth::{Cue, RATE};

/// A loop's fade, in seconds (the mixer's `LOOP_FADE_PER_SEC` is the same second).
const FADE_SECS: f64 = 1.0;

/// One chain of the pool: made once, a source started into it per cue.
struct Chain {
    gain: GainNode,
    panner: StereoPannerNode,
    /// The source playing through it (kept to be stopped when the chain is stolen).
    source: Option<AudioBufferSourceNode>,
    /// Its gain, for stealing the quietest.
    loud: f32,
    /// Which start the source in it was; `ended` is the latest start whose source has
    /// ended (set by the source's own `ended` event, which fires on a stop as well). The
    /// chain is free when the two agree: an older source's late `ended` cannot free the
    /// chain under a newer one.
    serial: u32,
    ended: Rc<Cell<u32>>,
}

impl Chain {
    fn free(&self) -> bool {
        self.serial == self.ended.get()
    }
}

/// One loop slot: its source, its gain, and which cue it plays.
struct Slot {
    cue: Cue,
    source: AudioBufferSourceNode,
    gain: GainNode,
}

/// The nodes, once the browser allows sound.
struct Graph {
    context: AudioContext,
    buffers: Vec<AudioBuffer>,
    master: GainNode,
    analyser: AnalyserNode,
    chains: Vec<Chain>,
    slots: [Option<Slot>; LOOP_SLOTS],
    /// Loops on their way out (a second's fade), and when each will have stopped: a
    /// `Quiet` stops them at once too.
    fading: Vec<(Slot, f64)>,
}

pub struct Audio {
    /// The rendered patches, until they are the browser's buffers.
    patches: Arc<Vec<Vec<f32>>>,
    graph: Option<Graph>,
    master: f32,
    /// What was said before the graph existed (the air): applied when it is.
    pending_air: [Option<(Cue, f32)>; LOOP_SLOTS],
    /// The loudest RMS the analyser saw, and when it was last read.
    loudest: f32,
    last_read: f64,
    scratch: Vec<f32>,
    /// Cues the graph took, and how many took a chain from a sound still playing.
    started: u32,
    stolen: u32,
    /// The context, made by the handler of the first click or key on the page: the
    /// gesture a browser wants before it plays, and some browsers (Safari) want the
    /// context made or resumed on the gesture's own call stack, not at the next frame.
    context: Rc<RefCell<Option<AudioContext>>>,
}

/// Move a parameter from where it is now to `value` over `secs` (0: at once). A ramp
/// runs from the parameter's previous event, which may be long past: so the present
/// value is set first, as the anchor, and what was scheduled after now is cancelled.
fn move_param(p: &AudioParam, now: f64, value: f32, secs: f64) {
    let _ = p.cancel_scheduled_values(now);
    let _ = p.set_value_at_time(p.value(), now);
    if secs > 0.0 {
        let _ = p.linear_ramp_to_value_at_time(value, now + secs);
    } else {
        let _ = p.set_value_at_time(value, now);
    }
}

/// Have `source` call `f` once when it ends (the closure frees itself then).
fn on_ended(source: &AudioBufferSourceNode, f: impl FnOnce() + 'static) {
    let f = Closure::once_into_js(f);
    AudioScheduledSourceNode::set_onended(source, Some(f.unchecked_ref()));
}

impl Audio {
    pub fn new(patches: Arc<Vec<Vec<f32>>>) -> Audio {
        let context = Rc::new(RefCell::new(None));
        // The first click or key anywhere on the page is the gesture (the click that
        // takes the pointer, usually): its handler makes the context and resumes it,
        // there and then; the graph is made on it at the next frame. Every later gesture
        // resumes a context found suspended. (The listeners live as long as the page:
        // made once, never dropped.)
        if let Some(document) = web_sys::window().and_then(|w| w.document()) {
            for kind in ["pointerdown", "keydown"] {
                let cell = context.clone();
                let on_gesture = Closure::<dyn FnMut()>::new(move || {
                    let Ok(mut slot) = cell.try_borrow_mut() else {
                        return;
                    };
                    match &*slot {
                        None => match AudioContext::new() {
                            Ok(c) => {
                                let _ = c.resume();
                                *slot = Some(c);
                            }
                            Err(e) => {
                                log::info!("sound: the browser gives no audio context: {e:?}")
                            }
                        },
                        Some(c) if c.state() == AudioContextState::Suspended => {
                            let _ = c.resume();
                        }
                        Some(_) => {}
                    }
                });
                let _ = document
                    .add_event_listener_with_callback(kind, on_gesture.as_ref().unchecked_ref());
                on_gesture.forget();
            }
        }
        Audio {
            patches,
            graph: None,
            master: 1.0,
            pending_air: [None; LOOP_SLOTS],
            loudest: 0.0,
            last_read: 0.0,
            scratch: vec![0.0; 1024],
            started: 0,
            stolen: 0,
            context,
        }
    }

    /// Make the graph once the gesture has made the context, resume a suspended
    /// context, read the analyser once a second.
    pub fn frame(&mut self) {
        if self.graph.is_none() {
            let context = self.context.try_borrow().ok().and_then(|c| c.clone());
            let Some(context) = context else { return };
            self.graph = self.build(context);
            if self.graph.is_some() {
                // The patches are the browser's buffers now.
                self.patches = Arc::new(Vec::new());
                let pending = self.pending_air;
                for (slot, air) in pending.iter().enumerate() {
                    if let Some((cue, gain)) = air {
                        self.play_loop(slot, Some(*cue), *gain);
                    }
                }
            }
        }
        let Some(g) = &self.graph else { return };
        if g.context.state() == AudioContextState::Suspended {
            let _ = g.context.resume();
        }
        let now = g.context.current_time();
        if now - self.last_read >= 1.0 {
            self.last_read = now;
            let n = (g.analyser.fft_size() as usize).min(self.scratch.len());
            g.analyser
                .get_float_time_domain_data(&mut self.scratch[..n]);
            let rms = (self.scratch[..n].iter().map(|v| v * v).sum::<f32>() / n as f32).sqrt();
            if rms.is_finite() {
                self.loudest = self.loudest.max(rms);
            }
        }
    }

    fn build(&self, context: AudioContext) -> Option<Graph> {
        let mut buffers = Vec::with_capacity(self.patches.len());
        for p in self.patches.iter() {
            let buffer = context
                .create_buffer(1, p.len().max(1) as u32, RATE as f32)
                .ok()?;
            if !p.is_empty() {
                buffer.copy_to_channel(p, 0).ok()?;
            }
            buffers.push(buffer);
        }
        let master = context.create_gain().ok()?;
        master.gain().set_value(self.master);
        let analyser = context.create_analyser().ok()?;
        analyser.set_fft_size(1024);
        master.connect_with_audio_node(&analyser).ok()?;
        analyser
            .connect_with_audio_node(&context.destination())
            .ok()?;
        let mut chains = Vec::with_capacity(VOICES);
        for _ in 0..VOICES {
            let gain = context.create_gain().ok()?;
            let panner = context.create_stereo_panner().ok()?;
            gain.connect_with_audio_node(&panner).ok()?;
            panner.connect_with_audio_node(&master).ok()?;
            chains.push(Chain {
                gain,
                panner,
                source: None,
                loud: 0.0,
                serial: 0,
                ended: Rc::new(Cell::new(0)),
            });
        }
        log::info!(
            "sound: the browser's audio context is made at {} Hz",
            context.sample_rate()
        );
        Some(Graph {
            context,
            buffers,
            master,
            analyser,
            chains,
            slots: [None, None],
            fading: Vec::new(),
        })
    }

    /// Apply a command; `false` when the browser does not play yet (the cue is lost; the
    /// air and the master are kept for when it does).
    pub fn apply(&mut self, c: Command) -> bool {
        match c {
            Command::Master(m) => {
                self.master = m;
                if let Some(g) = &self.graph {
                    move_param(&g.master.gain(), g.context.current_time(), m, 0.0);
                }
                true
            }
            Command::Loop { slot, cue, gain } => {
                let slot = slot as usize;
                if slot >= LOOP_SLOTS {
                    return true;
                }
                self.pending_air[slot] = cue.map(|c| (c, gain));
                if self.graph.is_some() {
                    self.play_loop(slot, cue, gain);
                }
                true
            }
            Command::Quiet => {
                self.pending_air = [None; LOOP_SLOTS];
                if let Some(g) = &mut self.graph {
                    for chain in g.chains.iter_mut() {
                        if let Some(s) = chain.source.take() {
                            let _ = AudioScheduledSourceNode::stop(&s);
                        }
                        chain.loud = 0.0;
                    }
                    let going = g.slots.iter_mut().filter_map(Option::take);
                    for s in going.chain(g.fading.drain(..).map(|(s, _)| s)) {
                        let _ = AudioScheduledSourceNode::stop(&s.source);
                        let _ = s.source.disconnect();
                        let _ = s.gain.disconnect();
                    }
                }
                true
            }
            Command::Cue {
                cue,
                left,
                right,
                pitch,
            } => self.play_cue(cue, left, right, pitch),
        }
    }

    fn play_cue(&mut self, cue: Cue, left: f32, right: f32, pitch: f32) -> bool {
        let Some(g) = &mut self.graph else {
            return false;
        };
        if g.context.state() != AudioContextState::Running {
            return false;
        }
        let Some(buffer) = g.buffers.get(cue as usize) else {
            return false;
        };
        // A free chain, or else the quietest (its sound stops; a stop ends it, and that
        // `ended` carries the old serial, so it cannot free the chain under the new).
        let i = match g.chains.iter().position(Chain::free) {
            Some(i) => i,
            None => {
                self.stolen += 1;
                let mut quietest = 0;
                for (i, c) in g.chains.iter().enumerate() {
                    if c.loud < g.chains[quietest].loud {
                        quietest = i;
                    }
                }
                if let Some(s) = g.chains[quietest].source.take() {
                    let _ = AudioScheduledSourceNode::stop(&s);
                }
                quietest
            }
        };
        let Ok(source) = g.context.create_buffer_source() else {
            return false;
        };
        source.set_buffer(Some(buffer));
        source.playback_rate().set_value(pitch);
        let chain = &mut g.chains[i];
        // The pair of ears as the mixer would have them, as one gain and one pan.
        let loud = left.max(right).max(1e-6);
        let pan = ((right - left) / (right + left).max(1e-6)).clamp(-1.0, 1.0);
        let now = g.context.current_time();
        move_param(&chain.gain.gain(), now, loud, 0.0);
        move_param(&chain.panner.pan(), now, pan, 0.0);
        if source.connect_with_audio_node(&chain.gain).is_err()
            || AudioScheduledSourceNode::start(&source).is_err()
        {
            return false;
        }
        // (The closure after the start: a source that never started never ends, and a
        // closure never called is never freed. `ended` is an event, never fired inside
        // `start`.)
        chain.serial = chain.serial.wrapping_add(1);
        let (serial, ended) = (chain.serial, chain.ended.clone());
        on_ended(&source, move || {
            // (Serials only grow; the latest end wins.)
            if serial.wrapping_sub(ended.get()) < u32::MAX / 2 {
                ended.set(serial);
            }
        });
        chain.source = Some(source);
        chain.loud = loud;
        self.started += 1;
        true
    }

    fn play_loop(&mut self, slot: usize, cue: Option<Cue>, gain: f32) {
        let Some(g) = &mut self.graph else { return };
        let now = g.context.current_time();
        // The same loop stays, moved to its gain; another, or none, fades the old one out.
        if let Some(s) = &g.slots[slot]
            && Some(s.cue) == cue
        {
            move_param(&s.gain.gain(), now, gain, FADE_SECS);
            return;
        }
        if let Some(old) = g.slots[slot].take() {
            let stops = now + FADE_SECS + 0.1;
            move_param(&old.gain.gain(), now, 0.0, FADE_SECS);
            let _ = AudioScheduledSourceNode::stop_with_when(&old.source, stops);
            let (source, node) = (old.source.clone(), old.gain.clone());
            on_ended(&old.source, move || {
                let _ = source.disconnect();
                let _ = node.disconnect();
            });
            // Remembered until it has stopped, for a `Quiet` in the meantime.
            g.fading.retain(|(_, by)| *by > now);
            g.fading.push((old, stops));
        }
        let Some(cue) = cue else { return };
        let Some(buffer) = g.buffers.get(cue as usize) else {
            return;
        };
        let (Ok(source), Ok(node)) = (g.context.create_buffer_source(), g.context.create_gain())
        else {
            return;
        };
        source.set_buffer(Some(buffer));
        source.set_loop(true);
        node.gain().set_value(0.0);
        move_param(&node.gain(), now, gain, FADE_SECS);
        if source.connect_with_audio_node(&node).is_err()
            || node.connect_with_audio_node(&g.master).is_err()
            || AudioScheduledSourceNode::start(&source).is_err()
        {
            return;
        }
        g.slots[slot] = Some(Slot {
            cue,
            source,
            gain: node,
        });
    }

    /// For the report: whether the browser plays, what it took, how many chains are
    /// busy now, the loudest it got.
    pub fn report(&self) -> String {
        let (state, busy) = match &self.graph {
            None => ("none", 0),
            Some(g) => (
                match g.context.state() {
                    AudioContextState::Running => "running",
                    AudioContextState::Suspended => "suspended",
                    _ => "closed",
                },
                g.chains.iter().filter(|c| !c.free()).count(),
            ),
        };
        format!(
            " context={state} started={} stolen={} busy={busy} loudest_rms={:.4}",
            self.started, self.stolen, self.loudest
        )
    }
}
