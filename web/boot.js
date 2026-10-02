// The loader of the browser client (docs/WEB.md 3.2, 5): pick the build this browser can
// run, collect the options, hand them to the client. No framework, no bundler.

const statusLine = document.getElementById("status");
const panel = document.getElementById("panel");
const canvas = document.getElementById("gm-canvas");
const fullscreen = document.getElementById("fullscreen");
const query = new URLSearchParams(location.search);

function say(text, error = false) {
  statusLine.textContent = text;
  statusLine.classList.toggle("error", error);
}

// Options a link may set: what a visitor sees, nothing that acts for them. Everything else
// is honoured only where config.json says this is a development site: on a real one a link
// with `connect` would walk a visitor's client into a stranger's zone, one with `script` or
// `travel-to` would play their character for them once they log in, and one with `cache-mb`
// would resize their cache.
const LINK_OPTIONS = ["third-person", "tactical", "map", "zone"];
const DEV_OPTIONS = ["connect", "cert", "user", "password", "character", "register", "name",
  "build", "team", "seconds", "script", "report", "cache-mb", "vram-mb", "travel-to", "travel-after"];

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

let running = false;

async function start(options, build) {
  panel.hidden = true;
  say("loading the client");
  globalThis.gmOptions = options;
  globalThis.gmStatus = (kind, text) => {
    if (kind === "stats") {
      console.log("GM-STATS " + text);
      return;
    }
    if (kind === "done") {
      console.log("GM-DONE " + text);
      running = false;
      say("finished");
      return;
    }
    if (kind === "error") {
      console.log("GM-ERROR " + text);
      // The WebGPU build could not get a device after all: once, try the other build on
      // a fresh page (a canvas that had a WebGPU context cannot give a WebGL2 one).
      if (build === "webgpu" && /renderer setup failed/.test(text)) {
        const retry = new URLSearchParams(location.search);
        retry.set("gl", "1");
        location.search = retry.toString();
        return;
      }
      running = false;
      say(text, true);
      return;
    }
    say(text);
  };
  running = true;
  fullscreen.hidden = false;
  canvas.focus();
  try {
    const client = await import(`./gm-client-${build}.js`);
    console.log(`GM-BUILD ${build}`);
    await client.default();
  } catch (e) {
    // wasm-bindgen unwinds out of `main` with an exception once the event loop is running;
    // that one is how winit hands control to the browser, not a failure.
    if (!String(e && e.message).includes("Using exceptions for control flow")) {
      globalThis.gmStatus("error", "the client did not start: " + (e && e.message ? e.message : e));
    }
  }
}

// Closing the tab mid-fight should take a second thought: Ctrl+W is one key from walking.
addEventListener("beforeunload", (e) => {
  if (running && !query.has("script")) e.preventDefault();
});

// Fullscreen takes the pointer and, where the browser has the Keyboard Lock API (Chromium),
// the keys a tab normally keeps: Ctrl+W, Tab, Esc (leaving is then a long press of Esc).
fullscreen.addEventListener("click", async () => {
  try {
    await canvas.requestFullscreen();
    if (navigator.keyboard && navigator.keyboard.lock) await navigator.keyboard.lock();
    canvas.focus();
  } catch (e) {
    say("fullscreen refused: " + e.message, true);
  }
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
  if (cfg.dev) for (const key of DEV_OPTIONS) if (query.has(key)) options[key] = query.get(key) || "1";
  const scripted = options.connect || options.user || query.has("offline") || query.has("map");
  if (scripted) {
    await start(options, build);
    return;
  }
  // The login form. Without a hub in config.json there is only the offline walk.
  panel.hidden = false;
  if (!options.hub) {
    say("config.json names no hub: only the offline walk is available here");
  } else if (typeof WebTransport === "undefined") {
    say("This browser has no WebTransport: only the offline walk is available here", true);
  }
  panel.addEventListener("submit", (e) => {
    e.preventDefault();
    const field = (id) => document.getElementById(id).value.trim();
    const password = document.getElementById("password");
    const login = { ...options, user: field("user"), password: password.value,
      character: field("character"), zone: field("zone"), build: field("build"),
      register: document.getElementById("register").checked ? "1" : "", "third-person": "1" };
    // The client takes the password out of its options once; the page keeps no copy.
    password.value = "";
    start(login, build);
  });
  document.getElementById("offline").addEventListener("click", () => start({ ...options, hub: "" }, build));
})();
