// Settings > Shortcuts: the three global shortcuts as key chips. Click a chip, press a new
// combination (Esc cancels); a changed chip is tinted and "Reset all to defaults" brings them
// back. Rust: shortcuts_get / shortcuts_pause / shortcuts_set / shortcuts_reset in
// src-tauri/src/shortcuts.rs. The static lines (tabs, editor keys) stay in the <dl> under it.
import { invoke } from "@tauri-apps/api/core";
import {
  LABELS, SHORTCUT_IDS, anyChanged, comboFromEvent, formatCombo, isChanged, refusalText,
  type Combos, type Refusal, type ShortcutId,
} from "./combo";
import { isMac, watchShortcuts } from "./current";

type SetResult = { combos: Combos; refusals: Refusal[] };

/** Returns a function that stops a recording (closing the sheet calls it). */
export function mountShortcutSettings(list: HTMLElement, resetBtn: HTMLButtonElement): () => void {
  list.innerHTML = SHORTCUT_IDS.map(
    (id) => `<div class="sc-row" data-id="${id}">
      <div class="sc-line"><span>${LABELS[id]}</span>
        <button class="sc-chip" type="button" aria-label="${LABELS[id]}: change shortcut"></button></div>
      <p class="setting-error sc-error" role="alert" hidden></p>
    </div>`,
  ).join("");
  const rows = new Map(
    SHORTCUT_IDS.map((id) => {
      const row = list.querySelector<HTMLElement>(`[data-id="${id}"]`)!;
      return [id, { chip: row.querySelector<HTMLButtonElement>(".sc-chip")!, error: row.querySelector<HTMLElement>(".sc-error")! }];
    }),
  );
  let combos: Combos | null = null;
  let recording: ShortcutId | null = null;

  const render = () => {
    if (!combos) return;
    for (const id of SHORTCUT_IDS) {
      const { chip } = rows.get(id)!;
      const rec = recording === id;
      chip.textContent = rec ? "Press new keys…" : formatCombo(combos[id], isMac);
      chip.classList.toggle("recording", rec);
      chip.classList.toggle("changed", !rec && isChanged(id, combos, isMac));
    }
    resetBtn.hidden = !anyChanged(combos, isMac);
  };

  const showRefusals = (refusals: Refusal[]) => {
    for (const id of SHORTCUT_IDS) rows.get(id)!.error.hidden = true;
    for (const r of refusals) {
      const row = rows.get(r.id as ShortcutId);
      if (!row) continue;
      row.error.textContent = refusalText(r, isMac);
      row.error.hidden = false;
    }
  };

  const apply = (result: SetResult) => {
    combos = result.combos;
    showRefusals(result.refusals);
    render();
  };

  /** Stops recording. HL Desk's shortcuts were paused so the keys reach this page. */
  const stop = async () => {
    if (!recording) return;
    recording = null;
    document.removeEventListener("keydown", onKey, true);
    window.removeEventListener("blur", cancel);
    render();
    await invoke("shortcuts_pause", { paused: false }).catch(() => {});
  };
  const cancel = () => void stop();

  async function onKey(e: KeyboardEvent): Promise<void> {
    // Every key goes to the recorder (not to Esc-hides-the-window or Ctrl+1/2/3).
    e.preventDefault();
    e.stopImmediatePropagation();
    const id = recording;
    if (!id) return;
    if (e.key === "Escape" && !e.ctrlKey && !e.altKey && !e.shiftKey && !e.metaKey) {
      await stop();
      return;
    }
    const combo = comboFromEvent(e);
    if (!combo) return; // a modifier alone or a bare key: keep waiting
    await stop();
    try {
      apply(await invoke<SetResult>("shortcuts_set", { id, combo }));
    } catch (err) {
      const row = rows.get(id)!;
      row.error.textContent = String(err);
      row.error.hidden = false;
    }
  }

  for (const id of SHORTCUT_IDS) {
    rows.get(id)!.chip.addEventListener("click", async () => {
      if (recording === id) {
        await stop();
        return;
      }
      await stop();
      showRefusals([]);
      recording = id;
      render();
      document.addEventListener("keydown", onKey, true);
      window.addEventListener("blur", cancel);
      await invoke("shortcuts_pause", { paused: true }).catch(() => {});
    });
  }

  resetBtn.addEventListener("click", async () => {
    await stop();
    try {
      apply(await invoke<SetResult>("shortcuts_reset"));
    } catch (err) {
      showRefusals([]);
      rows.get("open")!.error.textContent = String(err);
      rows.get("open")!.error.hidden = false;
    }
  });

  watchShortcuts((c) => {
    combos = c;
    render();
  });
  return cancel;
}
