import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import { exit, getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import * as clipboard from "./features/clipboard";
import * as notes from "./features/notes";
import * as screenshot from "./features/screenshot";

// Each feature owns its folder and exports mount(el). A tab mounts its feature the first time it is shown.
const features: Record<string, { mount(el: HTMLElement): void }> = { clipboard, screenshot, notes };
const order = ["clipboard", "screenshot", "notes"];
const mounted = new Set<string>();
const TAB_KEY = "hl-desk.tab";
const isMac = navigator.userAgent.includes("Mac");
const mod = isMac ? "⌘" : "Ctrl+";

const tabs = [...document.querySelectorAll<HTMLButtonElement>("[data-tab]")];
const sheet = document.getElementById("settings")!;
const openBtn = document.getElementById("settings-open") as HTMLButtonElement;

function show(name: string, focusTab = false): void {
  if (!features[name]) name = order[0];
  for (const tab of tabs) {
    const active = tab.dataset.tab === name;
    tab.setAttribute("aria-selected", String(active));
    tab.tabIndex = active ? 0 : -1;
    if (active && focusTab) tab.focus();
    document.getElementById(`panel-${tab.dataset.tab}`)!.hidden = !active;
  }
  const panel = document.getElementById(`panel-${name}`)!;
  if (!mounted.has(name)) {
    mounted.add(name);
    features[name].mount(panel);
  }
  try {
    localStorage.setItem(TAB_KEY, name);
  } catch {
    // Storage unavailable: the app just opens on the first tab next time.
  }
  closeSheet(false);
  // A feature can mark the element to focus when its tab is shown (usually its search field).
  if (!focusTab) panel.querySelector<HTMLElement>("[data-autofocus]")?.focus();
}

function hideWindow(): void {
  getCurrentWindow().hide().catch(() => {});
}

// ---- Settings sheet ----
const shortcuts: [string, string][] = [
  ["Open or close HL Desk", `${mod}${isMac ? "⇧" : "Shift+"}Space`],
  ["Clipboard, Screenshot, Notes", `${mod}1, ${mod}2, ${mod}3`],
  ["Capture an area", `${mod}${isMac ? "⇧" : "Shift+"}2`],
  ["Editor: number, pen, crop, undo", `N, P, C, ${mod}Z`],
  ["Pin a clipboard item", `${mod}P`],
  ["New note", `${mod}N`],
  ["Move in a list, open", "↑ ↓, Enter"],
  ["Close", "Esc"],
];
document.getElementById("shortcuts")!.innerHTML = shortcuts
  .map(([what, keys]) => `<div><dt>${what}</dt><dd><kbd>${keys}</kbd></dd></div>`)
  .join("");
getVersion()
  .then((v) => (document.getElementById("app-version")!.textContent = `HL Desk ${v}`))
  .catch(() => {});

// ---- General settings (Rust: src-tauri/src/settings.rs) ----
type Settings = { welcomeSeen: boolean; openAtLogin: boolean };
const loginSwitch = document.getElementById("open-at-login") as HTMLInputElement;
const settingsError = document.getElementById("settings-error")!;

/** Reads the settings; the switch shows the real state from the OS. */
async function loadSettings(): Promise<Settings | null> {
  try {
    const s = await invoke<Settings>("settings_get");
    loginSwitch.checked = s.openAtLogin;
    return s;
  } catch {
    return null;
  }
}

loginSwitch.addEventListener("change", async () => {
  settingsError.hidden = true;
  const want = loginSwitch.checked;
  try {
    loginSwitch.checked = await invoke<boolean>("settings_set_open_at_login", { on: want });
  } catch (e) {
    loginSwitch.checked = !want;
    settingsError.textContent = String(e);
    settingsError.hidden = false;
  }
});

function openSheet(): void {
  void loadSettings();
  settingsError.hidden = true;
  sheet.hidden = false;
  openBtn.setAttribute("aria-expanded", "true");
  document.getElementById("settings-close")!.focus();
}

function closeSheet(restoreFocus = true): void {
  if (sheet.hidden) return;
  sheet.hidden = true;
  openBtn.setAttribute("aria-expanded", "false");
  if (restoreFocus) openBtn.focus();
}

// ---- First-run welcome ----
const welcome = document.getElementById("welcome")!;
const cmdKey = isMac ? "⌘" : "Ctrl";
const shiftKey = isMac ? "⇧" : "Shift";
const svg = (body: string) =>
  `<svg width="16" height="16" viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${body}</svg>`;
// Icons from Feather (MIT): sidebar, crop, clipboard.
const welcomeItems: [string, string, string[], string][] = [
  ["Open HL Desk", isMac ? "Or click HL in the menu bar" : "Or click HL in the tray",
    [cmdKey, shiftKey, "Space"], svg('<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M9 3v18"/>')],
  ["Capture an area", "Drag over anything, then mark it up",
    [cmdKey, shiftKey, "2"], svg('<path d="M6.13 1L6 16a2 2 0 0 0 2 2h15"/><path d="M1 6.13L16 6a2 2 0 0 1 2 2v15"/>')],
  ["Clipboard keeps 1 day", "Copy as usual, then search it here",
    [cmdKey, "C"], svg('<path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"/><rect x="8" y="2" width="8" height="4" rx="1"/>')],
];
const keycaps = (keys: string[]) => keys.map((k) => `<kbd class="keycap">${k}</kbd>`).join("");
document.getElementById("welcome-lead")!.textContent = isMac
  ? "It lives in your menu bar. Everything stays on this Mac."
  : "It lives in the tray, bottom right. Everything stays on this PC.";
document.getElementById("welcome-list")!.innerHTML = welcomeItems
  .map(([title, hint, keys, icon]) => `<li><span class="welcome-icon">${icon}</span>
    <span class="welcome-text"><strong>${title}</strong><span class="hint">${hint}</span></span>
    <span class="welcome-keys">${keycaps(keys)}</span></li>`)
  .join("");

function showWelcome(): void {
  welcome.hidden = false;
  // Screen Recording is a macOS permission; the row goes once it is allowed.
  const perm = document.getElementById("welcome-perm")!;
  perm.hidden = !isMac;
  if (isMac) {
    invoke<boolean>("screenshot_permission")
      .then((granted) => (perm.hidden = granted))
      .catch(() => {});
  }
  document.getElementById("welcome-try")!.focus();
}

function closeWelcome(): void {
  welcome.hidden = true;
  invoke("settings_set_welcome_seen", { seen: true }).catch(() => {});
}

document.getElementById("welcome-done")!.addEventListener("click", () => {
  closeWelcome();
  show(localTab() ?? order[0]);
});
document.getElementById("welcome-try")!.addEventListener("click", () => {
  closeWelcome();
  show("screenshot");
  invoke("screenshot_capture").catch(() => {});
});
document.getElementById("welcome-perm-open")!.addEventListener("click", () => {
  invoke("screenshot_open_permission_settings").catch(() => {});
});
document.getElementById("welcome-again")!.addEventListener("click", () => {
  invoke("settings_set_welcome_seen", { seen: false }).catch(() => {});
  closeSheet(false);
  showWelcome();
});

openBtn.addEventListener("click", openSheet);
document.getElementById("settings-close")!.addEventListener("click", () => closeSheet());
document.getElementById("quit")!.addEventListener("click", () => {
  exit(0).catch(() => {});
});

// ---- Keyboard ----
for (const tab of tabs) {
  tab.addEventListener("click", () => show(tab.dataset.tab!));
  // Arrow keys move between the segments (WAI-ARIA tabs pattern).
  tab.addEventListener("keydown", (e) => {
    const step = (<Record<string, number>>{ ArrowRight: 1, ArrowLeft: -1 })[e.key];
    if (step === undefined) return;
    e.preventDefault();
    const i = order.indexOf(tab.dataset.tab!);
    show(order[(i + step + order.length) % order.length], true);
  });
}

// Bubble phase, so a feature can handle Esc first (clear its search) and call preventDefault().
document.addEventListener("keydown", (e) => {
  if (e.defaultPrevented) return;
  const cmd = isMac ? e.metaKey : e.ctrlKey;
  if (cmd && !e.shiftKey && !e.altKey && ["1", "2", "3"].includes(e.key)) {
    e.preventDefault();
    show(order[Number(e.key) - 1]);
  } else if (e.key === "Escape") {
    e.preventDefault();
    if (!sheet.hidden) closeSheet();
    else hideWindow();
  }
});

function localTab(): string | null {
  try {
    return localStorage.getItem(TAB_KEY);
  } catch {
    return null; // Storage unavailable.
  }
}
show(localTab() ?? order[0]);
void loadSettings().then((s) => {
  if (s && !s.welcomeSeen) showWelcome();
});

// A failed capture shows this window (Rust) with the error on the Screenshot tab; so does a
// capture stopped for the macOS Screen Recording permission (the tab shows the permission card).
void listen("screenshot-error", () => show("screenshot"));
void listen("screenshot-permission", () => {
  welcome.hidden = true;
  show("screenshot");
});
