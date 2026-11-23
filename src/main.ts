import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import { exit, getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import * as clipboard from "./features/clipboard";
import * as notes from "./features/notes";
import { mountVoiceSettings } from "./features/notes/voice-settings";
import * as screenshot from "./features/screenshot";
import { mountPermission } from "./features/screenshot/permission";
import { mountCaptureSettings } from "./features/screenshot/settings";
import { fillIcons } from "./icons";
import { openReleaseNotes, startUpdater } from "./updater";

// Each feature owns its folder and exports mount(el). A tab mounts its feature the first time it is shown.
const features: Record<string, { mount(el: HTMLElement): void }> = { clipboard, screenshot, notes };
const order = ["clipboard", "screenshot", "notes"];
const mounted = new Set<string>();
const TAB_KEY = "hl-desk.tab";
const PIN_KEY = "hl-desk.pinned";
const isMac = navigator.userAgent.includes("Mac");
const mod = isMac ? "⌘" : "Ctrl+";

const tabs = [...document.querySelectorAll<HTMLButtonElement>("[data-tab]")];
// Tab and Settings heading icons (index.html data-icon), before anything else draws.
fillIcons();
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
  // The selected pill slides to this tab (styles.css .segmented-pill).
  tabs[0].parentElement!.style.setProperty("--tab", String(order.indexOf(name)));
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

// ---- Pin: keep the popover open when it loses focus (lib.rs shell_set_pinned); Esc still hides it ----
// Remembered in localStorage: a UI preference of this window only (settings.json is shared
// with the capture-key settings).
const pinBtn = document.getElementById("pin") as HTMLButtonElement;

function setPinned(on: boolean): void {
  pinBtn.setAttribute("aria-pressed", String(on));
  document.body.dataset.pinned = String(on);
  invoke("shell_set_pinned", { pinned: on }).catch(() => {});
  try {
    localStorage.setItem(PIN_KEY, String(on));
  } catch {
    // Storage unavailable: the pin is just not remembered.
  }
}

pinBtn.addEventListener("click", () => setPinned(pinBtn.getAttribute("aria-pressed") !== "true"));

// ---- Settings sheet ----
const shortcuts: [string, string][] = [
  ["Open or close HL Desk", `${mod}${isMac ? "⇧" : "Shift+"}Space`],
  ["Clipboard, Screenshot, Notes", `${mod}1, ${mod}2, ${mod}3`],
  ["Open the Translate window", `${mod}${isMac ? "⇧" : "Shift+"}1`],
  ["Capture an area", `${mod}${isMac ? "⇧" : "Shift+"}2`],
  ["Editor: number, pen, crop, undo", `N, P, C, ${mod}Z`],
  ["Pin a clipboard item", `${mod}P`],
  ["New note", `${mod}N`],
  ["Dictate into a note (stop: Esc)", `${mod}${isMac ? "⇧" : "Shift+"}D`],
  ["Move in a list, open", "↑ ↓, Enter"],
  ["Close", "Esc"],
];
mountCaptureSettings(document.getElementById("settings-capture")!);
mountVoiceSettings(document.getElementById("settings-voice")!);
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

document.getElementById("open-notices")!.addEventListener("click", () => {
  settingsError.hidden = true;
  invoke("shell_open_notices").catch((e) => {
    settingsError.textContent = `Could not open the notices: ${String(e)}`;
    settingsError.hidden = false;
  });
});

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
// Icons from Feather (MIT): sidebar, crop, clipboard, globe, image, mic.
const ICON_TRANSLATE = svg('<circle cx="12" cy="12" r="10"/><path d="M2 12h20"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/>');
const ICON_IMAGE = svg('<rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><path d="M21 15l-5-5L5 21"/>');
const ICON_MIC = svg('<path d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z"/><path d="M19 10v2a7 7 0 0 1-14 0v-2"/><path d="M12 19v4M8 23h8"/>');
const copyTwice = [cmdKey, "C", "C"];
const captureKeys = [cmdKey, shiftKey, "2"];
const dictateKeys = [cmdKey, shiftKey, "D"];
const translatorKeys = [cmdKey, shiftKey, "1"];
// [title, hint, keys, icon, new?]
type GuideRow = [string, string, string[], string, boolean?];
const welcomeItems: GuideRow[] = [
  ["Open HL Desk", isMac ? "Or click HL in the menu bar" : "Or click HL in the tray",
    [cmdKey, shiftKey, "Space"], svg('<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M9 3v18"/>')],
  ["Capture an area", "Drag over anything, it's copied",
    [cmdKey, shiftKey, "2"], svg('<path d="M6.13 1L6 16a2 2 0 0 0 2 2h15"/><path d="M1 6.13L16 6a2 2 0 0 1 2 2v15"/>')],
  ["Clipboard keeps 1 day", "Copy as usual, then search it here",
    [cmdKey, "C"], svg('<path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"/><rect x="8" y="2" width="8" height="4" rx="1"/>')],
  ["Translate", "Select text, copy twice. Screenshots: Translate button", copyTwice, ICON_TRANSLATE],
  ["Translate window", "Type or paste, then copy the English", translatorKeys, ICON_TRANSLATE],
  ["Voice to text", "Click the mic in a note and speak", dictateKeys, ICON_MIC],
];
// "New in HL Desk" (What's new on the Updated line): rows per version. A later release adds an
// entry; a version without one shows the newest entry.
const whatsNew: Record<string, GuideRow[]> = {
  "0.1.12": [
    ["Copy text from a screenshot", "Capture, then click Copy text", captureKeys, ICON_IMAGE, true],
  ],
  "0.1.11": [
    ["Translate window", "Type or paste, then copy the English", translatorKeys, ICON_TRANSLATE, true],
  ],
  "0.1.10": [
    ["Screenshots copy at once", "Capture, then paste. Mark up from the notice", captureKeys, ICON_IMAGE, true],
  ],
  "0.1.9": [
    ["Translate text", "Select text in any app, copy twice", copyTwice, ICON_TRANSLATE, true],
    ["Translate a screenshot", "Capture, then click Translate", captureKeys, ICON_IMAGE, true],
    ["Voice to text", "Click the mic in a note and speak", dictateKeys, ICON_MIC],
  ],
};
const keycaps = (keys: string[]) => keys.map((k) => `<kbd class="keycap">${k}</kbd>`).join("");
const guideRows = (rows: GuideRow[]) =>
  rows
    .map(([title, hint, keys, icon, isNew]) => `<li><span class="welcome-icon">${icon}</span>
    <span class="welcome-text"><strong>${title}${isNew ? ' <span class="badge-new">New</span>' : ""}</strong><span class="hint">${hint}</span></span>
    <span class="welcome-keys">${keycaps(keys)}</span></li>`)
    .join("");
const welcomeTitle = document.getElementById("welcome-title")!;
const welcomeLead = document.getElementById("welcome-lead")!;
const welcomeList = document.getElementById("welcome-list")!;
const welcomeNotes = document.getElementById("welcome-notes")!;
const welcomeFoot = document.getElementById("welcome-foot")!;
const welcomePermEl = document.getElementById("welcome-perm")!;
const welcomeDone = document.getElementById("welcome-done") as HTMLButtonElement;
const welcomeTry = document.getElementById("welcome-try") as HTMLButtonElement;
/** null: the sheet shows the welcome; a version: it shows "New in HL Desk" for that version. */
let whatsNewVersion: string | null = null;

// Screen Recording is a macOS permission: the same card as on the Screenshot tab, gone once allowed.
const welcomePerm = mountPermission(welcomePermEl);

function showWelcome(): void {
  whatsNewVersion = null;
  welcomeTitle.textContent = "Welcome to HL Desk";
  welcomeLead.textContent = isMac
    ? "It lives in your menu bar. Everything stays on this Mac."
    : "It lives in the tray, bottom right. Everything stays on this PC.";
  welcomeList.innerHTML = guideRows(welcomeItems);
  welcomeNotes.hidden = true;
  welcomeFoot.textContent = "Show it again in Settings";
  welcomeDone.className = "btn";
  welcomeTry.hidden = false;
  closeSheet(false);
  welcome.hidden = false;
  if (isMac) welcomePerm.check();
  welcomeTry.focus();
}

/** "New in HL Desk": only from What's new on the Updated line; welcomeSeen is left alone. */
function showWhatsNew(version: string): void {
  whatsNewVersion = version;
  const latest = Object.keys(whatsNew).sort((a, b) => a.localeCompare(b, undefined, { numeric: true })).pop()!;
  welcomeTitle.textContent = "New in HL Desk";
  welcomeLead.textContent = `Updated to ${version}. Here is what you can do now.`;
  welcomeList.innerHTML = guideRows(whatsNew[version] ?? whatsNew[latest]);
  welcomeNotes.hidden = false;
  welcomeFoot.textContent = "All tips: Settings, Show welcome again";
  welcomeDone.className = "btn-primary";
  welcomeTry.hidden = true;
  welcomePermEl.hidden = true;
  closeSheet(false);
  welcome.hidden = false;
  welcomeDone.focus();
}

function closeWelcome(): void {
  welcome.hidden = true;
  if (whatsNewVersion === null) invoke("settings_set_welcome_seen", { seen: true }).catch(() => {});
}

welcomeDone.addEventListener("click", () => {
  closeWelcome();
  show(localTab() ?? order[0]);
});
document.getElementById("welcome-release-notes")!.addEventListener("click", () => {
  if (whatsNewVersion) openReleaseNotes(whatsNewVersion);
});
// Translate window (src-tauri/src/translator.rs): the header button, as Ctrl+Shift+1 / Cmd+Shift+1.
document.getElementById("translator-open")!.addEventListener("click", () => {
  invoke("translator_open").catch(() => {});
});
// Guide: the full welcome list, any time. Opening it does not change welcomeSeen.
document.getElementById("guide-open")!.addEventListener("click", showWelcome);
document.getElementById("welcome-try")!.addEventListener("click", () => {
  closeWelcome();
  show("screenshot");
  invoke("screenshot_capture").catch(() => {});
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

function localPinned(): boolean {
  try {
    return localStorage.getItem(PIN_KEY) === "true";
  } catch {
    return false;
  }
}

function localTab(): string | null {
  try {
    return localStorage.getItem(TAB_KEY);
  } catch {
    return null; // Storage unavailable.
  }
}
show(localTab() ?? order[0]);
setPinned(localPinned());
void loadSettings().then((s) => {
  if (s && !s.welcomeSeen) showWelcome();
});
// Update check 10 s after start and every 6 hours (src/updater.ts).
startUpdater(showWhatsNew);

// A failed capture shows this window (Rust) with the error on the Screenshot tab; so does a
// capture stopped for the macOS Screen Recording permission (the tab shows the permission card).
void listen("screenshot-error", () => show("screenshot"));
void listen("screenshot-permission", () => {
  welcome.hidden = true;
  show("screenshot");
});
