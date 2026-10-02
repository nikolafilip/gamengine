#!/usr/bin/env node
// Run the browser client in headless Chromium and print what it says (docs/WEB.md 8).
//   node scripts/web-run.mjs --url URL [--seconds N] [--screenshot FILE [--at S]] [--software]
//                            [--chrome BIN] [--size WxH] [--profile DIR] [--cache NAME]
// --cache NAME reads every entry of that Cache API cache back at the end and prints
// "GM-CACHE entries=N bytes=M": what the browser holds, whatever the page believes.
// Console lines go to stdout as they come. Exit 0 when the page reported GM-DONE, 1 on
// GM-ERROR or a timeout. Needs Node 22+ (its built-in WebSocket) and a Chromium.
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : fallback;
};
const url = opt("--url");
if (!url) {
  console.error("web-run: --url is required");
  process.exit(2);
}
const seconds = Number(opt("--seconds", "30"));
const screenshot = opt("--screenshot");
const shotAt = Number(opt("--at", String(Math.max(1, seconds - 2))));
const size = opt("--size", "1280x720").replace("x", ",");
const chrome = opt("--chrome", process.env.CHROME || "chromium");
const software = args.includes("--software");

// --profile DIR keeps the browser's storage between runs (the model cache lives there).
const kept = opt("--profile");
const profile = kept || mkdtempSync(join(tmpdir(), "gm-web-"));
const port = 9300 + (process.pid % 600);
const flags = [
  "--headless=new", "--no-first-run", "--no-default-browser-check", `--user-data-dir=${profile}`,
  `--remote-debugging-port=${port}`, `--window-size=${size}`, "--disable-gpu-sandbox",
  // WebGPU in headless Linux Chromium is behind these; with --use-angle=vulkan it is the
  // machine's GPU, without it SwiftShader (what CI has).
  "--enable-unsafe-webgpu", "--enable-features=Vulkan",
  ...(software ? [] : ["--use-angle=vulkan"]),
  "about:blank",
];
const browser = spawn(chrome, flags, { stdio: ["ignore", "ignore", "ignore"] });
let finished = false;
const finish = (code) => {
  if (finished) return;
  finished = true;
  browser.kill("SIGTERM");
  setTimeout(() => {
    if (!kept) {
      try { rmSync(profile, { recursive: true, force: true }); } catch { /* still held */ }
    }
    process.exit(code);
  }, 300);
};
browser.on("exit", () => { if (!finished) { console.error("web-run: the browser exited"); finish(1); } });

async function target() {
  for (let i = 0; i < 100; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      const page = list.find((t) => t.type === "page");
      if (page) return page.webSocketDebuggerUrl;
    } catch { /* not up yet */ }
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error("no DevTools endpoint");
}

const ws = new WebSocket(await target());
let nextId = 1;
const waiting = new Map();
const send = (method, params = {}) => new Promise((resolve) => {
  const id = nextId++;
  waiting.set(id, resolve);
  ws.send(JSON.stringify({ id, method, params }));
});
let verdict = null;
ws.addEventListener("message", (ev) => {
  const msg = JSON.parse(ev.data);
  if (msg.id && waiting.has(msg.id)) {
    waiting.get(msg.id)(msg.result || {});
    waiting.delete(msg.id);
    return;
  }
  if (msg.method === "Runtime.consoleAPICalled") {
    const line = msg.params.args.map((a) => a.value ?? a.description ?? "").join(" ");
    console.log(line);
    if (line.startsWith("GM-DONE")) verdict ??= 0;
    if (line.startsWith("GM-ERROR")) verdict ??= 1;
  } else if (msg.method === "Runtime.exceptionThrown") {
    const d = msg.params.exceptionDetails;
    console.log("EXCEPTION " + (d.exception?.description || d.text));
  }
});
await new Promise((r) => ws.addEventListener("open", r, { once: true }));
await send("Runtime.enable");
await send("Page.enable");
await send("Page.navigate", { url });
const started = Date.now();
let shot = !screenshot;
while (Date.now() - started < (seconds + 8) * 1000) {
  await new Promise((r) => setTimeout(r, 100));
  if (!shot && (Date.now() - started >= shotAt * 1000 || verdict !== null)) {
    shot = true;
    const png = await send("Page.captureScreenshot", { format: "png" });
    if (png.data) writeFileSync(screenshot, Buffer.from(png.data, "base64"));
  }
  if (verdict !== null && shot) break;
}
const cacheName = opt("--cache");
if (cacheName) {
  const expr = `(async () => {
    const cache = await caches.open(${JSON.stringify(cacheName)});
    let entries = 0, bytes = 0;
    for (const request of await cache.keys()) {
      const response = await cache.match(request);
      if (response) { entries++; bytes += (await response.arrayBuffer()).byteLength; }
    }
    return "entries=" + entries + " bytes=" + bytes;
  })()`;
  const held = await send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
  console.log("GM-CACHE " + (held.result?.value ?? "unreadable"));
}
if (verdict === null) console.error("web-run: no GM-DONE within the time");
finish(verdict ?? 1);
