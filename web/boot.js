// The loader of the browser client (docs/WEB.md 3.2, 5): pick the build this browser can
// run, collect the options, hand them to the client. No framework, no bundler.

const statusLine = document.getElementById("status");
const stage = document.getElementById("stage");
const panel = document.getElementById("panel");
const canvas = document.getElementById("gm-canvas");
const fullscreen = document.getElementById("fullscreen");
const query = new URLSearchParams(location.search);
const field = (id) => document.getElementById(id);

function say(text, error = false) {
  statusLine.textContent = text;
  statusLine.classList.toggle("error", error);
}

// Options a link may set: what a visitor sees, nothing that acts for them. Everything else
// is honoured only where config.json says this is a development site: on a real one a link
// with `connect` would walk a visitor's client into a stranger's zone, one with `script` or
// `travel-to` would play their character for them once they log in, and one with `cache-mb`
// would resize their cache.
const LINK_OPTIONS = ["third-person", "map"];
const DEV_OPTIONS = ["connect", "cert", "user", "password", "character", "register", "name", "zone",
  "build", "team", "seconds", "script", "report", "cache-mb", "vram-mb", "travel-to", "travel-after",
  "ui-script"];

async function config() {
  try {
    const r = await fetch("config.json", { cache: "no-store" });
    return r.ok ? await r.json() : {};
  } catch {
    return {};
  }
}

// Which build: WebGPU where the browser hands out an adapter, WebGL2 otherwise.
async function pickBuild() {
  if (!query.has("gl") && navigator.gpu) {
    try {
      if (await navigator.gpu.requestAdapter()) return "webgpu";
    } catch { /* fall through to WebGL2 */ }
  }
  if (document.createElement("canvas").getContext("webgl2")) return "webgl";
  return null;
}

// Whether the client runs, and whether it can take a login (it has a hub and has not died).
let running = false;
let takesLogin = false;

// The login screen of a browser is this page's form (docs/CLIENT.md 4.1): the browser can
// fill it and remember it. The client says when it wants it and with what words; what is
// typed is left for the client in `globalThis.gmLogin`, which the client empties.
const notice = field("notice");
function form(shown, text = "", waiting = false) {
  panel.hidden = !shown;
  notice.textContent = text;
  for (const el of panel.querySelectorAll("input, button")) el.disabled = waiting;
  if (shown && !waiting) {
    (field("user").value ? field("password") : field("user")).focus();
  } else if (!shown) {
    canvas.focus();
  }
}

// A new account types its password twice (there is no reset: a typo would lock it away),
// and the browser is told it is a new password, to offer one and to save it.
function registering(on) {
  field("register").checked = on;
  field("again-row").hidden = !on;
  field("again").required = on;
  field("password").autocomplete = on ? "new-password" : "current-password";
  field("enter").textContent = on ? "create account" : "log in";
}
field("register").addEventListener("change", (e) => registering(e.target.checked));

panel.addEventListener("submit", (e) => {
  e.preventDefault();
  if (!running || !takesLogin) return;
  const password = field("password");
  const again = field("again");
  const register = field("register").checked;
  if (register && password.value !== again.value) {
    notice.textContent = "the two passwords differ";
    return;
  }
  globalThis.gmLogin = { user: field("user").value.trim(), password: password.value, register };
  // The client takes the password out of `gmLogin` once; the page keeps no copy.
  password.value = "";
  again.value = "";
  form(true, "asking the hub", true);
});
field("offline").addEventListener("click", () => {
  const q = new URLSearchParams(location.search);
  q.set("offline", "1");
  running = false;
  location.search = q.toString();
});

async function start(options, build) {
  panel.hidden = true;
  say("loading the client");
  globalThis.gmOptions = options;
  globalThis.gmStatus = (kind, text) => {
    if (kind === "stats") {
      console.log("GM-STATS " + text);
      return;
    }
    if (kind === "say") {
      // A UI script's word for whoever drives the browser (docs/CLIENT.md 9).
      console.log("GM-SAY " + text);
      return;
    }
    if (kind === "done") {
      console.log("GM-DONE " + text);
      running = false;
      say("finished");
      return;
    }
    if (kind === "login") {
      say("");
      form(true, text);
      return;
    }
    if (kind === "login-wait") {
      form(true, "asking the hub", true);
      return;
    }
    if (kind === "screen") {
      // Logged in: the next time the form is asked for, it is to log in.
      registering(false);
      form(false);
      return;
    }
    if (kind === "error") {
      console.log("GM-ERROR " + text);
      // The WebGPU build could not get a device after all: once, try the other build on
      // a fresh page (a canvas that had a WebGPU context cannot give a WebGL2 one).
      running = false;
      if (build === "webgpu" && /renderer setup failed/.test(text)) {
        const retry = new URLSearchParams(location.search);
        retry.set("gl", "1");
        location.search = retry.toString();
        return;
      }
      // Nobody is left to take a login: the form goes, and what it may have left behind.
      takesLogin = false;
      delete globalThis.gmLogin;
      form(false);
      say(text, true);
      return;
    }
    say(text);
  };
  running = true;
  fullscreen.hidden = false;
  canvas.focus();
  try {
    // The build's stamp (index.html's `boot.js?v=`, written by scripts/build-web.sh) goes on
    // the module and the wasm too: a browser that cached the last build fetches this one
    // instead of meeting the zone's "protocol version mismatch".
    const stamp = new URL(import.meta.url).searchParams.get("v");
    const v = stamp ? `?v=${encodeURIComponent(stamp)}` : "";
    const client = await import(`./gm-client-${build}.js${v}`);
    console.log(`GM-BUILD ${build} ${stamp || ""}`);
    await client.default({ module_or_path: new URL(`./gm-client-${build}_bg.wasm${v}`, import.meta.url) });
  } catch (e) {
    // wasm-bindgen unwinds out of `main` with an exception once the event loop is running;
    // that one is how winit hands control to the browser, not a failure.
    if (!String(e && e.message).includes("Using exceptions for control flow")) {
      globalThis.gmStatus("error", "the client did not start: " + (e && e.message ? e.message : e));
    }
  }
}

// Closing the tab mid-fight should take a second thought: Ctrl+W is one key from walking.
// Not while the login form is up (nothing is being played), and not in a scripted run.
addEventListener("beforeunload", (e) => {
  if (running && panel.hidden && !query.has("script") && !query.has("ui-script")) e.preventDefault();
});

// Fullscreen takes the pointer and, where the browser has the Keyboard Lock API (Chromium),
// the keys a tab normally keeps: Ctrl+W, Tab, Esc (leaving is then a long press of Esc).
// On a phone, fullscreen also turns the screen to landscape where the browser allows
// (docs/WEB.md 3.5): the game is wide, and a phone held upright shows little of it.
fullscreen.addEventListener("click", async () => {
  try {
    await stage.requestFullscreen();
    if (navigator.keyboard && navigator.keyboard.lock) await navigator.keyboard.lock();
    canvas.focus();
  } catch (e) {
    say("fullscreen refused: " + e.message, true);
    return;
  }
  try {
    if (screen.orientation && screen.orientation.lock && matchMedia("(pointer: coarse)").matches) {
      await screen.orientation.lock("landscape");
    }
  } catch { /* a desktop, or a browser that keeps the orientation to itself */ }
});

(async () => {
  if (typeof WebTransport === "undefined") {
    // Offline play needs no transport; say what is missing only when it is needed.
    console.log("GM-NOTE no WebTransport in this browser");
  }
  const build = await pickBuild();
  if (!build) {
    say("This browser has neither WebGPU nor WebGL2; the client cannot draw here.", true);
    return;
  }
  const cfg = await config();
  const options = { hub: cfg.hub || "", "hub-cert": cfg.hub_cert_sha256 || "" };
  for (const key of LINK_OPTIONS) if (query.has(key)) options[key] = query.get(key) || "1";
  if (cfg.dev === true) for (const key of DEV_OPTIONS) if (query.has(key)) options[key] = query.get(key) || "1";
  const scripted = options.connect || options.user || query.has("offline") || query.has("map");
  if (scripted) {
    // No account and no zone named: an offline walk, not the login.
    if (!options.user && !options.connect) options.hub = "";
    // (A scripted login that stops at the characters can still log out, to the form.)
    takesLogin = !!options.hub && typeof WebTransport !== "undefined";
    await start(options, build);
    return;
  }
  // Without a hub, or without a way to reach one, there is only the offline walk.
  if (!options.hub || typeof WebTransport === "undefined") {
    panel.hidden = false;
    for (const el of panel.querySelectorAll("input, #enter")) el.disabled = true;
    notice.textContent = options.hub
      ? "This browser has no WebTransport: only the offline walk is available here."
      : "config.json names no hub: only the offline walk is available here.";
    return;
  }
  takesLogin = true;
  // The client starts at once, behind the form: it asks for the form when it wants it.
  await start({ ...options, "third-person": options["third-person"] || "1" }, build);
})();
