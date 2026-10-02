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
