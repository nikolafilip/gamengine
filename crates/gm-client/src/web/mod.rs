//! What the browser is to the client (WEB.md 3.3): its transport, its timers, its files, its
//! cache and its page.

pub mod hub;
pub mod net;
pub mod store;
pub mod wt;

use std::future::Future;
use std::pin::pin;
use std::task::Poll;

use js_sys::{Function, Promise, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use wt::js_text;

/// Resolves after `ms` milliseconds (`setTimeout`).
pub async fn sleep_ms(ms: u32) {
    let promise = Promise::new(&mut |resolve, _| {
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms as i32);
        }
    });
    let _ = JsFuture::from(promise).await;
}

/// `f`'s output, or `None` when it took longer than `ms`.
pub async fn timeout_ms<T>(ms: u32, f: impl Future<Output = T>) -> Option<T> {
    let mut f = pin!(f);
    let mut timer = pin!(sleep_ms(ms));
    std::future::poll_fn(move |cx| {
        if let Poll::Ready(v) = f.as_mut().poll(cx) {
            return Poll::Ready(Some(v));
        }
        if timer.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        Poll::Pending
    })
    .await
}

/// The bytes at `url` (relative to the page); `None` for a 404.
pub async fn fetch_bytes(url: &str) -> Result<Option<Vec<u8>>, String> {
    let window = web_sys::window().ok_or("no window")?;
    let response: web_sys::Response = JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|e| format!("fetching {url}: {}", js_text(&e)))?
        .unchecked_into();
    if response.status() == 404 {
        return Ok(None);
    }
    if !response.ok() {
        return Err(format!("fetching {url}: HTTP {}", response.status()));
    }
    let buffer = JsFuture::from(response.array_buffer().map_err(|e| js_text(&e))?)
        .await
        .map_err(|e| js_text(&e))?;
    Ok(Some(Uint8Array::new(&buffer).to_vec()))
}

/// Tell the page something (its status line): `globalThis.gmStatus(kind, text)` if the
/// loader defined it. Kinds: "status", "error", "stats", "done".
pub fn tell_page(kind: &str, text: &str) {
    let global = js_sys::global();
    if let Ok(f) = Reflect::get(&global, &"gmStatus".into())
        && let Ok(f) = f.dyn_into::<Function>()
    {
        let _ = f.call2(&JsValue::NULL, &kind.into(), &text.into());
    }
}

/// The options the loader left in `globalThis.gmOptions` (WEB.md 5).
pub struct PageOptions(JsValue);

impl PageOptions {
    pub fn read() -> PageOptions {
        PageOptions(
            Reflect::get(&js_sys::global(), &"gmOptions".into()).unwrap_or(JsValue::UNDEFINED),
        )
    }

    pub fn string(&self, key: &str) -> Option<String> {
        Reflect::get(&self.0, &key.into())
            .ok()
            .and_then(|v| v.as_string())
            .filter(|s| !s.is_empty())
    }

    /// A string that is read once: it is deleted from the page's object (the password).
    pub fn take(&self, key: &str) -> Option<String> {
        let value = self.string(key);
        if let Some(object) = self.0.dyn_ref::<js_sys::Object>() {
            let _ = Reflect::delete_property(object, &key.into());
        }
        value
    }

    pub fn number(&self, key: &str) -> Option<f64> {
        let v = Reflect::get(&self.0, &key.into()).ok()?;
        v.as_f64()
            .or_else(|| v.as_string().and_then(|s| s.parse().ok()))
    }

    pub fn flag(&self, key: &str) -> bool {
        Reflect::get(&self.0, &key.into())
            .map(|v| v.is_truthy() && v.as_string().as_deref() != Some("0"))
            .unwrap_or(false)
    }
}

/// What the page's login form sent, once (CLIENT.md 4.1): the email, the password and
/// whether a new account is wanted. The page leaves it in `globalThis.gmLogin`; it is
/// deleted as it is read, so the password lives in one place at a time.
pub fn take_login() -> Option<(String, String, bool)> {
    let global = js_sys::global();
    let login = Reflect::get(&global, &"gmLogin".into()).ok()?;
    if login.is_undefined() || login.is_null() {
        return None;
    }
    let _ = Reflect::delete_property(&global, &"gmLogin".into());
    let text = |key: &str| {
        Reflect::get(&login, &key.into())
            .ok()
            .and_then(|v| v.as_string())
    };
    let register = Reflect::get(&login, &"register".into()).is_ok_and(|v| v.is_truthy());
    Some((text("user")?, text("password")?, register))
}

thread_local! {
    /// What a refusal of the pointer falls into: nothing.
    static REFUSED: Closure<dyn FnMut(JsValue)> = Closure::new(|_why: JsValue| {});
}

/// Ask the browser for the pointer on the game's canvas (WEB.md 3.4). The browser may say
/// no: nobody's click is behind the asking (a page that entered the game by itself, an
/// entry that took longer than a click lasts). That is an answer, not an error, and the
/// next click on the canvas asks again. (winit asks too, and lets the refusal fall into
/// the console as an uncaught rejection: so the page asks for itself.)
pub fn ask_for_pointer() {
    let canvas = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("gm-canvas"));
    let Some(canvas) = canvas else { return };
    let Ok(ask) = Reflect::get(&canvas, &"requestPointerLock".into()) else {
        return;
    };
    let Ok(ask) = ask.dyn_into::<Function>() else {
        return;
    };
    // A promise in today's browsers, nothing in older ones.
    if let Ok(answer) = ask.call0(&canvas)
        && let Ok(promise) = answer.dyn_into::<Promise>()
    {
        REFUSED.with(|refused| {
            let _ = promise.catch(refused);
        });
    }
}

/// The game's canvas in device pixels: its CSS size times `devicePixelRatio` (WEB.md
/// 3.5), `None` without a canvas or while it has no size.
pub fn canvas_device_size() -> Option<winit::dpi::PhysicalSize<u32>> {
    let window = web_sys::window()?;
    let canvas: web_sys::HtmlElement = window
        .document()?
        .get_element_by_id("gm-canvas")?
        .dyn_into()
        .ok()?;
    let ratio = window.device_pixel_ratio().max(0.5);
    let (w, h) = (canvas.client_width(), canvas.client_height());
    if w <= 0 || h <= 0 {
        return None;
    }
    Some(winit::dpi::PhysicalSize::new(
        (w as f64 * ratio).round() as u32,
        (h as f64 * ratio).round() as u32,
    ))
}

/// Give the pointer back to the browser.
pub fn give_pointer_back() {
    if let Some(d) = web_sys::window().and_then(|w| w.document()) {
        d.exit_pointer_lock();
    }
}

/// Whether the page's canvas holds the pointer (the browser gives it up by itself when
/// Escape is pressed, and that key press never reaches the page, WEB.md 3.4).
pub fn pointer_locked() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .is_some_and(|d| d.pointer_lock_element().is_some())
}

/// A SHA-256 as 64 hex digits.
pub fn parse_hash(hex: &str) -> Option<[u8; 32]> {
    gm_model::id_from_hex(hex)
}

/// Bytes of the module's linear memory (it only ever grows).
pub fn wasm_memory_bytes() -> u64 {
    let memory: js_sys::WebAssembly::Memory = wasm_bindgen::memory().unchecked_into();
    let buffer: js_sys::ArrayBuffer = memory.buffer().unchecked_into();
    buffer.byte_length() as u64
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = log)]
    fn console_log(s: &str);
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn console_error(s: &str);
}

/// `log` to the browser's console.
pub struct ConsoleLogger;

impl log::Log for ConsoleLogger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        let noisy = m.target().starts_with("wgpu") || m.target().starts_with("naga");
        m.level()
            <= if noisy {
                log::Level::Warn
            } else {
                log::max_level().to_level().unwrap_or(log::Level::Info)
            }
    }

    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            let line = format!("[{}] {}", r.level(), r.args());
            if r.level() <= log::Level::Warn {
                console_error(&line);
            } else {
                console_log(&line);
            }
        }
    }

    fn flush(&self) {}
}

/// A panic aborts the module (`panic = "abort"`): say why on the page first, because
/// afterwards nothing of ours runs again.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let text = format!("the client stopped: {info}");
        console_error(&text);
        tell_page("error", &text);
    }));
}
