// Auto-update (tauri-plugin-updater; endpoint and public key in src-tauri/tauri.conf.json).
// Checks 10 s after start and every 6 hours. A new version downloads in the background, then a
// line at the top of the popover and of Settings says "HL Desk <version> is ready" with
// "Restart to update" and "Later". Nothing installs or restarts without that click.
// Settings > General has "Check for updates". Every step goes to the app log (app_log).
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";

const FIRST_CHECK_MS = 10_000;
const CHECK_EVERY_MS = 6 * 60 * 60 * 1000;
const CHECK_TIMEOUT_MS = 30_000;

type State =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "current" }
  | { kind: "downloading"; version: string }
  | { kind: "ready"; version: string }
  | { kind: "installing"; version: string }
  | { kind: "failed"; message: string } // install failed: shown on the line
  | { kind: "error"; message: string }; // check or download failed: shown in Settings only

let state: State = { kind: "idle" };
let current = "";
/** The downloaded update, waiting for "Restart to update". */
let ready: Update | null = null;
/** "Later": the line stays hidden until another version is ready or "Check for updates" is clicked. */
let dismissed = false;
/** A check or download is running. */
let busy = false;

const bars = [...document.querySelectorAll<HTMLElement>("[data-update-bar]")];
const status = document.getElementById("update-status")!;
const checkBtn = document.getElementById("update-check") as HTMLButtonElement;

function log(level: "info" | "warn" | "error", message: string): void {
  invoke("app_log", { level, message: `updater: ${message}` }).catch(() => {});
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function set(next: State): void {
  state = next;
  render();
}

function render(): void {
  // The line: ready (or installing, or an install that failed), unless "Later" was clicked.
  let line = "";
  let showRestart = false;
  if (state.kind === "ready") {
    line = `HL Desk ${state.version} is ready`;
    showRestart = true;
  } else if (state.kind === "installing") {
    line = `Updating to HL Desk ${state.version}…`;
    showRestart = true;
  } else if (state.kind === "failed") {
    line = `Could not update: ${state.message}`;
  }
  for (const bar of bars) {
    bar.hidden = !line || dismissed;
    bar.querySelector<HTMLElement>(".update-text")!.textContent = line;
    bar.querySelector<HTMLElement>(".update-text")!.title = line;
    const restart = bar.querySelector<HTMLButtonElement>(".update-restart")!;
    restart.hidden = !showRestart;
    restart.disabled = state.kind === "installing";
    bar.querySelector<HTMLButtonElement>(".update-later")!.disabled = state.kind === "installing";
  }

  // Settings > General > Updates.
  status.textContent = statusText(state);
  status.classList.toggle("update-error", state.kind === "failed" || state.kind === "error");
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
    // Windows: starts the installer (passive) and exits HL Desk; the installer starts it again.
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

export function startUpdater(): void {
  for (const bar of bars) {
    bar.innerHTML = `<span class="update-text"></span>
      <button class="btn-primary update-restart" type="button">Restart to update</button>
      <button class="btn update-later" type="button">Later</button>`;
    bar.querySelector(".update-restart")!.addEventListener("click", () => void restart());
    bar.querySelector(".update-later")!.addEventListener("click", later);
  }
  checkBtn.addEventListener("click", () => void run(true));
  getVersion()
    .then((v) => {
      current = v;
      render();
    })
    .catch(() => {});
  render();
  setTimeout(() => {
    void run(false);
    setInterval(() => void run(false), CHECK_EVERY_MS);
  }, FIRST_CHECK_MS);
}
