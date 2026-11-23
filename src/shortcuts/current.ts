// The current global shortcuts, for any window that shows them (guide rows, the capture card,
// the Translate window footer). Rust: shortcuts_get, and the "shortcuts-changed" event sent to
// every window after a change in Settings > Shortcuts (src-tauri/src/shortcuts.rs).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { defaults, type Combos } from "./combo";

export const isMac = navigator.userAgent.includes("Mac");
export const CHANGED_EVENT = "shortcuts-changed";

/** Calls `show` with the current combos now (the defaults until Rust answers) and after every
 * change. */
export function watchShortcuts(show: (combos: Combos) => void): void {
  show(defaults(isMac));
  invoke<Combos>("shortcuts_get").then(show).catch(() => {});
  void listen<Combos>(CHANGED_EVENT, (e) => show(e.payload));
}
