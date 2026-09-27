#!/usr/bin/env node
// Runs the built bench page in headless Chrome over the DevTools protocol
// and prints the results. Zero npm dependencies; needs Node >= 21 for the
// global WebSocket. Set CHROME_BIN to point at a specific browser.

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const UI_DIR = fileURLToPath(new URL("..", import.meta.url));
const PORT = 4179;
const POLL_MS = 400;
const TIMEOUT_MS = 240_000;

function resolveVite() {
  const local = join(UI_DIR, "node_modules", "vite", "bin", "vite.js");
  if (existsSync(local)) return local;
  const root = join(UI_DIR, "..", "..", "..", "node_modules", "vite", "bin", "vite.js");
  if (existsSync(root)) return root;
  throw new Error("vite not found; run `npm install` in the workspace root");
}

function resolveChrome() {
  const candidates = [
    process.env.CHROME_BIN,
    "google-chrome-stable",
    "google-chrome",
    "chromium",
    "chromium-browser",
    "chrome",
  ].filter(Boolean);
  for (const bin of candidates) {
    const probe = spawnSync(bin, ["--version"], { timeout: 10_000 });
    if (probe.status === 0) return bin;
  }
  throw new Error("no Chrome/Chromium found; set CHROME_BIN or install google-chrome-stable");
}

class Cdp {
  #socket;
  #nextId = 0;
  #pending = new Map();

  constructor(socket) {
    this.#socket = socket;
    this.#socket.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data));
      if (message.id !== undefined) this.#pending.get(message.id)?.(message);
    });
  }

  static async connect(url) {
    const socket = new WebSocket(url);
    await new Promise((resolve, reject) => {
      socket.addEventListener("open", () => resolve());
      socket.addEventListener("error", () => reject(new Error(`cannot connect to ${url}`)));
    });
    return new Cdp(socket);
  }

  send(method, params, sessionId) {
    const id = ++this.#nextId;
    const payload = { id, method, params };
    if (sessionId) payload.sessionId = sessionId;
    return new Promise((resolve, reject) => {
      this.#pending.set(id, (message) => {
        if (message.error) reject(new Error(`${method}: ${message.error.message}`));
        else resolve(message.result);
      });
      this.#socket.send(JSON.stringify(payload));
    });
  }

  close() {
    this.#socket.close();
  }
}

async function startPreview() {
  const child = spawn(
    process.execPath,
    [resolveVite(), "preview", "--port", String(PORT), "--strictPort", "--host", "127.0.0.1"],
    { cwd: UI_DIR, stdio: ["ignore", "ignore", "pipe"] },
  );
  child.stderr.on("data", (chunk) => process.stderr.write(`[vite] ${chunk}`));

  const deadline = Date.now() + 20_000;
  for (;;) {
    try {
      const response = await fetch(`http://127.0.0.1:${PORT}/`);
      if (response.ok) break;
    } catch {
      // not up yet
    }
    if (Date.now() > deadline) throw new Error("vite preview did not start");
    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  return {
    child,
    stop: () =>
      new Promise((resolve) => {
        child.on("exit", () => resolve());
        child.kill("SIGTERM");
        setTimeout(() => child.kill("SIGKILL"), 3_000).unref();
      }),
  };
}

function startChrome() {
  const userDataDir = mkdtempSync(join(tmpdir(), "nanostores-bench-"));
  const child = spawn(
    resolveChrome(),
    [
      "--headless=new",
      "--disable-gpu",
      "--no-sandbox",
      "--no-first-run",
      "--no-default-browser-check",
      "--disable-background-timer-throttling",
      "--disable-renderer-backgrounding",
      `--user-data-dir=${userDataDir}`,
      "--remote-debugging-port=0",
      "about:blank",
    ],
    { stdio: ["ignore", "ignore", "pipe"] },
  );

  return new Promise((resolve, reject) => {
    let buffer = "";
    const timer = setTimeout(() => reject(new Error("chrome did not expose DevTools")), 20_000);
    child.stderr.on("data", (chunk) => {
      buffer += chunk.toString();
      const match = buffer.match(/DevTools listening on (ws:\/\/\S+)/);
      if (match) {
        clearTimeout(timer);
        resolve({
          child,
          wsUrl: match[1],
          stop: () =>
            new Promise((resolveStop) => {
              child.on("exit", () => {
                rmSync(userDataDir, { recursive: true, force: true });
                resolveStop();
              });
              child.kill("SIGTERM");
              setTimeout(() => child.kill("SIGKILL"), 3_000).unref();
            }),
        });
      }
    });
    child.on("exit", () => {
      clearTimeout(timer);
      reject(new Error("chrome exited before exposing DevTools"));
    });
  });
}

async function evaluate(cdp, sessionId, expression) {
  const result = await cdp.send("Runtime.evaluate", { expression, returnByValue: true }, sessionId);
  if (result.exceptionDetails) {
    const details = result.exceptionDetails;
    throw new Error(details.exception?.description ?? details.text);
  }
  return result.result?.value;
}

async function main() {
  const preview = await startPreview();
  let chrome;
  try {
    chrome = await startChrome();
    const cdp = await Cdp.connect(chrome.wsUrl);
    try {
      const target = await cdp.send("Target.createTarget", { url: `http://127.0.0.1:${PORT}/` });
      const session = await cdp.send("Target.attachToTarget", {
        targetId: target.targetId,
        flatten: true,
      });

      const deadline = Date.now() + TIMEOUT_MS;
      for (;;) {
        const done = await evaluate(cdp, session.sessionId, "window.__benchDone === true");
        if (done === true) break;
        if (Date.now() > deadline) throw new Error("benchmark timed out");
        await new Promise((resolve) => setTimeout(resolve, POLL_MS));
      }

      const status = await evaluate(
        cdp,
        session.sessionId,
        "document.getElementById('status')?.textContent",
      );
      const table = await evaluate(
        cdp,
        session.sessionId,
        "document.getElementById('bench-table')?.textContent",
      );
      const json = await evaluate(
        cdp,
        session.sessionId,
        "JSON.stringify(window.__benchResults ?? null)",
      );

      if (status !== "done") throw new Error(`page reported: ${status}\n${json}`);
      process.stdout.write(`${table}\n\n`);
      process.stdout.write("BENCH_JSON ");
      process.stdout.write(`${json}\n`);
    } finally {
      cdp.close();
    }
  } finally {
    await chrome?.stop();
    await preview.stop();
  }
}

main().catch((error) => {
  console.error(String(error?.stack ?? error?.message ?? error));
  process.exitCode = 1;
});
