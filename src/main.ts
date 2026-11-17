import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import { exit, getVersion } from "@tauri-apps/api/app";
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

function openSheet(): void {
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

let saved: string | null = null;
try {
  saved = localStorage.getItem(TAB_KEY);
} catch {
  // Storage unavailable.
}
show(saved ?? order[0]);
