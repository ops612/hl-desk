// Auto-update (tauri-plugin-updater; endpoint and public key in src-tauri/tauri.conf.json).
// Checks 10 s after start and every 6 hours. A new version downloads in the background, then a
// card at the top of the popover and of Settings says "Update ready · <version>" with up to three
// lines from the release notes, "Restart to update" and "Later", and the HL DESK wordmark gets a
// small indigo dot. Nothing installs or restarts without that click.
// After an update the app opens its window once with "Updated to <version> ✓" and a "What's new"
// link (the previous version is remembered in localStorage). Settings > Updates shows when the
// last check ran and "Check now". Every step goes to the app log (app_log).
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";

const FIRST_CHECK_MS = 10_000;
const CHECK_EVERY_MS = 6 * 60 * 60 * 1000;
const CHECK_TIMEOUT_MS = 30_000;
/** The version that ran last time: a different one now means HL Desk was just updated. */
const VERSION_KEY = "hl-desk.version";
/** When the last check finished without an error (Unix ms). */
const LAST_CHECK_KEY = "hl-desk.update-last-check";
const NOTES_MAX = 3;
const RELEASE_URL = "https://github.com/ops612/hl-desk-releases/releases/tag/v";

type State =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "current" }
  | { kind: "downloading"; version: string }
  | { kind: "ready"; version: string }
  | { kind: "installing"; version: string }
  | { kind: "failed"; message: string } // install failed: shown on the card
  | { kind: "error"; message: string }; // check or download failed: shown in Settings only

let state: State = { kind: "idle" };
let current = "";
/** The downloaded update, waiting for "Restart to update". */
let ready: Update | null = null;
/** "Later": the card stays hidden until another version is ready or "Check now" is clicked. */
let dismissed = false;
/** A check or download is running. */
let busy = false;

const bars = [...document.querySelectorAll<HTMLElement>("[data-update-bar]")];
const wordmark = document.querySelector<HTMLElement>(".wordmark")!;
const status = document.getElementById("update-status")!;
const lastEl = document.getElementById("update-last")!;
const checkBtn = document.getElementById("update-check") as HTMLButtonElement;
const updatedLine = document.getElementById("updated-line")!;

function log(level: "info" | "warn" | "error", message: string): void {
  invoke("app_log", { level, message: `updater: ${message}` }).catch(() => {});
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function storageGet(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

/** False when storage is unavailable. */
function storageSet(key: string, value: string): boolean {
  try {
    localStorage.setItem(key, value);
    return true;
  } catch {
    return false;
  }
}

/** Up to three short lines from latest.json "notes": one per line or per "; ". */
export function noteLines(body: string | undefined): string[] {
  return (body ?? "")
    .split(/\r?\n|;\s+/)
    .map((l) => l.replace(/^\s*(?:[-*•]|\d+[.)])\s*/, "").trim())
    .filter((l) => l && !l.startsWith("#"))
    .slice(0, NOTES_MAX);
}

/** "just now", "5 min ago", "3 h ago", "yesterday", "4 days ago". */
export function relativeTime(ms: number, now = Date.now()): string {
  const min = Math.floor((now - ms) / 60_000);
  if (min < 1) return "just now";
  if (min < 60) return `${min} min ago`;
  const h = Math.floor(min / 60);
  if (h < 24) return `${h} h ago`;
  const days = Math.floor(h / 24);
  return days === 1 ? "yesterday" : `${days} days ago`;
}

function set(next: State): void {
  state = next;
  render();
}

function render(): void {
  // The card: ready (or installing, or an install that failed), unless "Later" was clicked.
  let title = "";
  let notes: string[] = [];
  let showRestart = false;
  if (state.kind === "ready") {
    title = `Update ready · ${state.version}`;
    notes = noteLines(ready?.body);
    showRestart = true;
  } else if (state.kind === "installing") {
    title = `Updating to HL Desk ${state.version}…`;
    notes = noteLines(ready?.body);
    showRestart = true;
  } else if (state.kind === "failed") {
    title = `Could not update: ${state.message}`;
  }
  for (const bar of bars) {
    bar.hidden = !title || dismissed;
    const titleEl = bar.querySelector<HTMLElement>(".update-title")!;
    titleEl.textContent = title;
    titleEl.title = title;
    const list = bar.querySelector<HTMLElement>(".update-notes")!;
    list.replaceChildren(
      ...notes.map((n) => {
        const li = document.createElement("li");
        li.textContent = n;
        li.title = n;
        return li;
      }),
    );
    list.hidden = !notes.length;
    const restart = bar.querySelector<HTMLButtonElement>(".update-restart")!;
    restart.hidden = !showRestart;
    restart.disabled = state.kind === "installing";
    bar.querySelector<HTMLButtonElement>(".update-later")!.disabled = state.kind === "installing";
  }
  // The dot stays after "Later": a quiet sign that an update is waiting (Settings shows it).
  wordmark.classList.toggle("has-update", state.kind === "ready" || state.kind === "installing");
  wordmark.title = state.kind === "ready" ? `Update ready · ${state.version}` : "";

  // Settings > General > Updates.
  status.textContent = statusText(state);
  status.classList.toggle("update-error", state.kind === "failed" || state.kind === "error");
  const last = Number(storageGet(LAST_CHECK_KEY));
  lastEl.textContent = state.kind === "checking" ? "" : `Last checked ${last ? relativeTime(last) : "never"}`;
  checkBtn.disabled = busy || state.kind === "installing";
}

function statusText(s: State): string {
  switch (s.kind) {
    case "idle":
      return current ? `HL Desk ${current}` : "";
    case "checking":
      return "Checking for updates…";
    case "current":
      return `You're up to date (${current})`;
    case "downloading":
      return `Downloading HL Desk ${s.version}…`;
    case "ready":
      return `HL Desk ${s.version} is ready`;
    case "installing":
      return `Updating to HL Desk ${s.version}…`;
    case "failed":
      return `Could not update: ${s.message}`;
    case "error":
      return `Could not check for updates: ${s.message}`;
  }
}

/** Checks, and downloads a new version in the background. manual: the Settings button. */
async function run(manual: boolean): Promise<void> {
  if (busy || state.kind === "installing") return;
  busy = true;
  log("info", manual ? "check (button)" : "check (scheduled)");
  if (manual) set({ kind: "checking" });
  else render();
  let update: Update | null = null;
  try {
    update = await check({ timeout: CHECK_TIMEOUT_MS });
    storageSet(LAST_CHECK_KEY, String(Date.now()));
    if (!update) {
      log("info", `up to date (${current})`);
      // An update already downloaded stays ready.
      if (ready) {
        if (manual) dismissed = false;
        set({ kind: "ready", version: ready.version });
      } else {
        set({ kind: "current" });
      }
      return;
    }
    if (ready && ready.version === update.version) {
      log("info", `${update.version} is already downloaded`);
      void update.close().catch(() => {});
      if (manual) dismissed = false;
      set({ kind: "ready", version: ready.version });
      return;
    }
    log("info", `found ${update.version} (running ${current}), downloading`);
    set({ kind: "downloading", version: update.version });
    let bytes = 0;
    await update.download((e) => {
      if (e.event === "Progress") bytes += e.data.chunkLength;
    });
    log("info", `downloaded ${update.version} (${bytes} bytes), ready to install`);
    if (ready) void ready.close().catch(() => {});
    ready = update;
    dismissed = false;
    set({ kind: "ready", version: update.version });
  } catch (e) {
    log("error", `${manual ? "check (button)" : "check (scheduled)"} failed: ${errorText(e)}`);
    if (update && update !== ready) void update.close().catch(() => {});
    // A failed later check keeps an update that is already downloaded.
    if (ready) set({ kind: "ready", version: ready.version });
    else set({ kind: "error", message: errorText(e) });
  } finally {
    busy = false;
    render();
  }
}

/** "Restart to update": install the downloaded version, then start HL Desk again. */
async function restart(): Promise<void> {
  if (!ready || state.kind === "installing") return;
  const update = ready;
  log("info", `restart to update: installing ${update.version}`);
  set({ kind: "installing", version: update.version });
  try {
    // Windows: starts the installer (passive) and exits HL Desk; the installer starts it again,
    // with the same arguments (so maybe --hidden). The new version then sees a different
    // VERSION_KEY and shows its window (checkUpdated).
    await update.install();
    log("info", `installed ${update.version}, relaunching`);
    await relaunch();
  } catch (e) {
    log("error", `install of ${update.version} failed: ${errorText(e)}`);
    // Download it again at the next check.
    ready = null;
    void update.close().catch(() => {});
    dismissed = false;
    set({ kind: "failed", message: errorText(e) });
  }
}

function later(): void {
  log("info", `later (${state.kind === "ready" ? state.version : state.kind})`);
  dismissed = true;
  // An install that failed is not shown again; a ready update still is, in Settings.
  if (state.kind === "failed") set(ready ? { kind: "ready", version: ready.version } : { kind: "idle" });
  else render();
}

/** "Updated to <version> ✓ · What's new": once, on the first start of a new version. */
function showUpdated(version: string): void {
  updatedLine.innerHTML = `<span class="update-title"></span>
    <button class="link-btn updated-whats-new" type="button">What's new</button>
    <button class="icon-btn updated-close" type="button" aria-label="Dismiss" title="Dismiss">
      <svg width="12" height="12" viewBox="0 0 14 14" aria-hidden="true" fill="none" stroke="currentColor"
        stroke-width="1.5" stroke-linecap="round"><path d="M3 3l8 8M11 3l-8 8" /></svg>
    </button>`;
  updatedLine.querySelector(".update-title")!.textContent = `Updated to ${version} ✓`;
  updatedLine.querySelector(".updated-whats-new")!.addEventListener("click", () => {
    log("info", `what's new ${version}`);
    invoke("shell_open_url", { url: `${RELEASE_URL}${version}` }).catch((e) =>
      log("error", `open release notes: ${errorText(e)}`),
    );
  });
  updatedLine.querySelector(".updated-close")!.addEventListener("click", () => {
    updatedLine.hidden = true;
  });
  updatedLine.hidden = false;
}

/** Compares the running version with the one remembered from the last start. */
async function checkUpdated(version: string): Promise<void> {
  const prev = storageGet(VERSION_KEY);
  if (!storageSet(VERSION_KEY, version)) return; // No storage: cannot tell, show nothing.
  let updated = prev !== null && prev !== version;
  if (prev === null) {
    // 0.1.5 and older did not remember the version: someone who has seen the welcome card
    // was already using HL Desk, so this start is an update (a fresh install has not seen it).
    const s = await invoke<{ welcomeSeen: boolean }>("settings_get").catch(() => null);
    updated = s?.welcomeSeen === true;
  }
  if (!updated) return;
  log("info", `updated: ${prev ?? "0.1.5 or older"} -> ${version}, showing the window`);
  showUpdated(version);
  // The updater restarts HL Desk with its old arguments, which may include --hidden.
  invoke("shell_show_main").catch(() => {});
}

export function startUpdater(): void {
  for (const bar of bars) {
    bar.innerHTML = `<p class="update-title"></p>
      <ul class="update-notes"></ul>
      <div class="update-actions">
        <button class="btn update-later" type="button">Later</button>
        <button class="btn-primary update-restart" type="button">Restart to update</button>
      </div>`;
    bar.querySelector(".update-restart")!.addEventListener("click", () => void restart());
    bar.querySelector(".update-later")!.addEventListener("click", later);
  }
  checkBtn.addEventListener("click", () => void run(true));
  getVersion()
    .then((v) => {
      current = v;
      render();
      return checkUpdated(v);
    })
    .catch(() => {});
  render();
  // Keeps "Last checked 5 min ago" current.
  setInterval(render, 60_000);
  setTimeout(() => {
    void run(false);
    setInterval(() => void run(false), CHECK_EVERY_MS);
  }, FIRST_CHECK_MS);
}
