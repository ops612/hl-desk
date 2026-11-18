// Notes feature. Called once, the first time the Notes tab is shown.
// List view (search, +, rows) and editor view (plain textarea, autosave). No Save button.
import { invoke } from "@tauri-apps/api/core";
import "./notes.css";

interface NoteSummary {
  id: number;
  title: string;
  preview: string;
  updatedAt: number;
}

interface Note {
  id: number;
  title: string;
  body: string;
  updatedAt: number;
}

/** The note open in the editor. id is null until the first non-empty save creates it. */
interface Draft {
  id: number | null;
  body: string;
  dirty: boolean;
}

const AUTOSAVE_MS = 400;
const UNDO_MS = 5000;
const isMac = navigator.userAgent.includes("Mac");

const api = {
  list: (query: string) => invoke<NoteSummary[]>("notes_list", { query }),
  get: (id: number) => invoke<Note | null>("notes_get", { id }),
  create: () => invoke<Note>("notes_create"),
  save: (id: number, body: string) => invoke<number>("notes_save", { id, body }),
  remove: (id: number) => invoke<void>("notes_delete", { id }),
  dictationStatus: () => invoke<{ available: boolean; off: boolean }>("notes_dictation_status"),
  dictate: () => invoke<boolean>("notes_dictate"),
  dictationStop: (end: boolean) => invoke<void>("notes_dictation_stop", { end }),
  // Offline voice to text (Windows, Settings switch; Rust: voice.rs).
  voiceStatus: () => invoke<{ supported: boolean; enabled: boolean }>("voice_status"),
  voiceStop: () => invoke<string>("voice_stop"),
  voiceCancel: () => invoke<void>("voice_cancel"),
};

/** Same as the popover's blur guard in notes.rs: dictation may run this long after the click. */
const DICTATION_MS = 60_000;
/** No text this long after a start: dictation most likely did not start. */
const DICTATION_QUIET_MS = 10_000;
const DICTATION_TROUBLE = isMac
  ? "No dictation text yet. If it didn't start, check System Settings > Keyboard > Dictation."
  : "No dictation text yet. If it didn't start, check Settings > Privacy > Speech > Online speech recognition.";
/** Offline voice to text: recording stops by itself after this long (voice.rs stops at 120 s too). */
const OFFLINE_MAX_MS = 120_000;
const DICTATION_SETTINGS_URL = "x-apple.systempreferences:com.apple.Keyboard-Settings.extension";

function log(level: "info" | "warn" | "error", message: string): void {
  invoke("app_log", { level, message: `notes: ${message}` }).catch(() => {});
}

/** "Today 4:25 pm", "Yesterday", "12 Nov" (with the year when it is not this year). */
function editedLabel(ms: number, now = new Date()): string {
  const d = new Date(ms);
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((day(now) - day(d)) / 86_400_000);
  if (days === 0) {
    const h = d.getHours();
    return `Today ${h % 12 || 12}:${String(d.getMinutes()).padStart(2, "0")} ${h < 12 ? "am" : "pm"}`;
  }
  if (days === 1) return "Yesterday";
  const month = d.toLocaleString("en-GB", { month: "short" });
  return `${d.getDate()} ${month}${d.getFullYear() === now.getFullYear() ? "" : ` ${d.getFullYear()}`}`;
}

const ICON_BACK =
  '<svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M10 3L5 8l5 5"/></svg>';
const ICON_TRASH =
  '<svg width="15" height="15" viewBox="0 0 16 16" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 4h11M6.5 4V2.5h3V4M4 4l.7 9.5h6.6L12 4M6.8 6.5v4.5M9.2 6.5v4.5"/></svg>';
// Microphone, from Feather (MIT).
const ICON_MIC =
  '<svg width="15" height="15" viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z"/><path d="M19 10v2a7 7 0 0 1-14 0v-2M12 19v4M8 23h8"/></svg>';
const dictateKeys = isMac ? "⌘⇧D" : "Ctrl+Shift+D";

export function mount(el: HTMLElement): void {
  el.innerHTML = `
    <div class="notes-view notes-list-view">
      <div class="toolbar">
        <input class="field" type="search" placeholder="Search notes" aria-label="Search notes"
          aria-controls="notes-rows" autocomplete="off" spellcheck="false" data-autofocus />
        <button class="icon-btn notes-list-mic" type="button" aria-label="New note by voice"
          title="New note by voice">${ICON_MIC}</button>
        <button class="btn-primary notes-add" type="button" aria-label="New note"
          title="New note (${isMac ? "⌘" : "Ctrl+"}N)">+</button>
      </div>
      <div class="scroll" id="notes-rows" role="listbox" aria-label="Notes"></div>
      <div class="undo-bar" role="status" hidden>
        <span>Note deleted</span>
        <button type="button">Undo</button>
      </div>
    </div>
    <div class="notes-view notes-editor-view" hidden>
      <div class="toolbar">
        <button class="icon-btn notes-back" type="button" aria-label="Back to notes" title="Back to notes">${ICON_BACK}</button>
        <span class="hint notes-status" role="status"></span>
        <button class="notes-mic" type="button" aria-pressed="false"
          title="Dictate (${dictateKeys})">${ICON_MIC}<span class="notes-mic-label">Dictate</span></button>
        <span class="notes-divider" aria-hidden="true"></span>
        <button class="icon-btn notes-trash" type="button" aria-label="Delete note" title="Delete note">${ICON_TRASH}</button>
      </div>
      <textarea class="field notes-body" aria-label="Note"
        placeholder="Start typing. The first line becomes the title."></textarea>
    </div>
    <div class="footer">
      <span class="notes-count">Saved automatically</span>
      <span class="notes-voice" hidden></span>
      <button class="btn notes-voice-settings" type="button" hidden>Turn on Dictation</button>
    </div>`;

  const $ = <T extends HTMLElement>(sel: string) => el.querySelector<T>(sel)!;
  const listView = $(".notes-list-view");
  const editorView = $(".notes-editor-view");
  const search = $<HTMLInputElement>('input[type="search"]');
  const rowsEl = $("#notes-rows");
  const undoBar = $(".undo-bar");
  const textarea = $<HTMLTextAreaElement>(".notes-body");
  const status = $(".notes-status");
  const countEl = $(".notes-count");

  let notes: NoteSummary[] = [];
  let total = 0;
  let selected = -1;
  let draft: Draft | null = null;
  let saveTimer: number | undefined;
  let saveChain: Promise<void> = Promise.resolve();
  let pending: { id: number; timer: number } | null = null;
  let refreshToken = 0;

  // ---- List ----

  async function refresh(): Promise<void> {
    const token = ++refreshToken;
    const query = search.value.trim();
    try {
      const [found, all] = await Promise.all([api.list(query), query ? api.list("") : null]);
      if (token !== refreshToken) return;
      const hidden = pending?.id;
      notes = found.filter((n) => n.id !== hidden);
      total = (all ?? found).filter((n) => n.id !== hidden).length;
      selected = notes.length ? Math.min(Math.max(selected, 0), notes.length - 1) : -1;
      renderList();
    } catch (e) {
      if (token !== refreshToken) return;
      rowsEl.innerHTML = "";
      rowsEl.append(emptyState("Could not load notes", String(e)));
    }
    renderCount();
  }

  function renderCount(): void {
    countEl.textContent = `Saved automatically · ${total} ${total === 1 ? "note" : "notes"}`;
  }

  function emptyState(text: string, hint: string): HTMLElement {
    const box = document.createElement("div");
    box.className = "empty";
    const p = document.createElement("p");
    p.textContent = text;
    const h = document.createElement("p");
    h.className = "hint";
    h.textContent = hint;
    box.append(p, h);
    return box;
  }

  function renderList(): void {
    rowsEl.innerHTML = "";
    if (!notes.length) {
      const query = search.value.trim();
      rowsEl.append(
        query
          ? emptyState(`No notes match "${query}"`, "Try another word.")
          : emptyState("Write your first note", `Press + or ${isMac ? "⌘" : "Ctrl+"}N to start.`),
      );
      return;
    }
    notes.forEach((note, i) => {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "row notes-row" + (i === selected ? " selected" : "");
      row.setAttribute("role", "option");
      row.setAttribute("aria-selected", String(i === selected));
      row.tabIndex = -1;
      const line = document.createElement("span");
      line.className = "notes-line";
      const title = document.createElement("span");
      title.className = "notes-title";
      title.textContent = note.title || "New note";
      const date = document.createElement("span");
      date.className = "notes-date";
      date.textContent = editedLabel(note.updatedAt);
      line.append(title, date);
      const preview = document.createElement("span");
      preview.className = "notes-preview";
      preview.textContent = note.preview || "No more text";
      row.append(line, preview);
      row.addEventListener("click", () => void openNote(note.id));
      rowsEl.append(row);
    });
  }

  function select(i: number): void {
    if (!notes.length) return;
    selected = Math.min(Math.max(i, 0), notes.length - 1);
    rowsEl.querySelectorAll(".notes-row").forEach((row, j) => {
      row.classList.toggle("selected", j === selected);
      row.setAttribute("aria-selected", String(j === selected));
      if (j === selected) row.scrollIntoView({ block: "nearest" });
    });
  }

  let searchTimer: number | undefined;
  search.addEventListener("input", () => {
    clearTimeout(searchTimer);
    selected = 0;
    searchTimer = window.setTimeout(() => void refresh(), 120);
  });
  search.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      select(selected + (e.key === "ArrowDown" ? 1 : -1));
    } else if (e.key === "Enter" && selected >= 0 && notes[selected]) {
      e.preventDefault();
      void openNote(notes[selected].id);
    } else if (e.key === "Escape" && search.value) {
      // First Esc clears the search; the next one closes the window (main.ts).
      e.preventDefault();
      search.value = "";
      selected = 0;
      void refresh();
    }
  });
  $(".notes-add").addEventListener("click", () => void newNote());

  // ---- Editor ----

  function showView(editor: boolean): void {
    listView.hidden = editor;
    editorView.hidden = !editor;
    // main.ts focuses [data-autofocus] when the Notes tab is shown again.
    search.toggleAttribute("data-autofocus", !editor);
    textarea.toggleAttribute("data-autofocus", editor);
    status.textContent = "";
    voiceTrouble = false;
    if (!editor) stopDictation();
    else void refreshOffline();
    renderVoice();
  }

  function openEditor(next: Draft): void {
    draft = next;
    textarea.value = next.body;
    showView(true);
    textarea.focus();
    textarea.setSelectionRange(next.body.length, next.body.length);
    textarea.scrollTop = 0;
  }

  async function openNote(id: number): Promise<void> {
    try {
      const note = await api.get(id);
      if (!note) return void refresh();
      openEditor({ id: note.id, body: note.body, dirty: false });
    } catch (e) {
      status.textContent = `Could not open: ${e}`;
    }
  }

  async function newNote(): Promise<void> {
    if (draft) {
      if (draft.id === null && !draft.body.trim()) return textarea.focus();
      await closeDraft();
    }
    openEditor({ id: null, body: "", dirty: false });
  }

  /** Queues a save of the draft; saves run one at a time, in order. */
  function save(): Promise<void> {
    clearTimeout(saveTimer);
    const d = draft;
    if (!d) return saveChain;
    saveChain = saveChain.then(async () => {
      if (!d.dirty) return;
      const body = d.body;
      try {
        if (d.id === null) {
          if (!body.trim()) return;
          d.id = (await api.create()).id;
          // The footer count includes the new note from its first character.
          total += 1;
          renderCount();
        }
        await api.save(d.id, body);
        if (d.body === body) d.dirty = false;
        if (draft === d) status.textContent = "";
      } catch (e) {
        if (draft === d) status.textContent = `Not saved: ${e}`;
      }
    });
    return saveChain;
  }

  /** Saves the open note and leaves the editor. An emptied note is removed (nothing to undo). */
  async function closeDraft(): Promise<void> {
    const d = draft;
    if (!d) return;
    await save();
    draft = null;
    if (d.id !== null && !d.body.trim()) {
      await api.remove(d.id).catch(() => {});
    }
  }

  async function backToList(): Promise<void> {
    await closeDraft();
    showView(false);
    await refresh();
    search.focus();
  }

  textarea.addEventListener("input", () => {
    if (!draft) return;
    draft.body = textarea.value;
    draft.dirty = true;
    clearTimeout(quietTimer);
    if (voiceTrouble) {
      voiceTrouble = false;
      renderVoice();
    }
    clearTimeout(saveTimer);
    // A new note is created on its first character (so it counts at once); then autosave.
    if (draft.id === null && draft.body.trim()) void save();
    else saveTimer = window.setTimeout(() => void save(), AUTOSAVE_MS);
  });
  $(".notes-back").addEventListener("click", () => void backToList());
  $(".notes-trash").addEventListener("click", () => void deleteDraft());
  // Leaving the window (Esc, click outside, Quit) saves at once.
  window.addEventListener("blur", () => void save());
  document.addEventListener("visibilitychange", () => void save());

  // ---- Delete with undo ----

  async function commitPending(): Promise<void> {
    const p = pending;
    if (!p) return;
    clearTimeout(p.timer);
    pending = null;
    undoBar.hidden = true;
    await api.remove(p.id).catch(() => {});
  }

  async function deleteDraft(): Promise<void> {
    const d = draft;
    if (!d) return;
    clearTimeout(saveTimer);
    await save();
    draft = null;
    if (d.id !== null) {
      await commitPending();
      if (d.body.trim()) {
        pending = { id: d.id, timer: window.setTimeout(() => void commitPending().then(refresh), UNDO_MS) };
        undoBar.hidden = false;
      } else {
        await api.remove(d.id).catch(() => {});
      }
    }
    showView(false);
    await refresh();
    search.focus();
  }

  undoBar.querySelector("button")!.addEventListener("click", () => {
    if (!pending) return;
    clearTimeout(pending.timer);
    pending = null;
    undoBar.hidden = true;
    void refresh();
    search.focus();
  });

  // ---- Voice to text: the computer's own dictation (Rust: notes.rs notes_dictate) ----
  // Windows: Win+H (Voice typing) types into the textarea; macOS: startDictation:. Autosave
  // picks the text up through the input event. While it may be running (60 s, or until the mic
  // is clicked again or Esc), the popover does not hide when it loses the focus.

  const mic = $<HTMLButtonElement>(".notes-mic");
  const micLabel = $(".notes-mic-label");
  const micDivider = $(".notes-divider");
  const listMic = $<HTMLButtonElement>(".notes-list-mic");
  const voiceHint = $(".notes-voice");
  const voiceSettings = $<HTMLButtonElement>(".notes-voice-settings");
  /** null until notes_dictation_status answers. */
  let voice: { available: boolean; off: boolean } | null = null;
  let dictating = false;
  let dictationTimer: number | undefined;
  /** Fires when nothing was typed for a while after a start. */
  let quietTimer: number | undefined;
  /** Show the "didn't start" hint (a failed start, or no text after one). */
  let voiceTrouble = false;
  /** Settings > Offline voice to text is on (Windows): the mic records in the app, no Win+H. */
  let offlineOn = false;
  /** The current dictation is an offline recording. */
  let offline = false;
  /** An offline recording is being written down. */
  let writing = false;
  /** A start is under way (status check, or the offline microphone opening). */
  let starting = false;
  /** Stop, Esc or closing the note came while the offline microphone was opening. */
  let stopWhileOpening = false;

  async function refreshOffline(): Promise<boolean> {
    if (isMac) return false;
    const s = await api.voiceStatus().catch(() => null);
    offlineOn = !!s?.supported && s.enabled;
    renderVoice();
    return offlineOn;
  }

  function renderVoice(): void {
    const editor = !editorView.hidden;
    countEl.hidden = editor;
    voiceHint.hidden = !editor;
    voiceHint.classList.toggle("warn", voiceTrouble);
    mic.hidden = voice !== null && !voice.available;
    micDivider.hidden = mic.hidden;
    listMic.hidden = mic.hidden;
    mic.setAttribute("aria-pressed", String(dictating));
    mic.classList.toggle("active", dictating);
    micLabel.textContent = dictating ? "Stop" : "Dictate";
    mic.title = dictating ? "Stop dictation (Esc)" : `Dictate (${dictateKeys})`;
    mic.disabled = writing;
    const off = voice?.off === true && isMac;
    voiceSettings.hidden = !editor || !off;
    if (voice && !voice.available) {
      voiceHint.textContent = isMac ? "Dictation isn't available on this Mac" : "Voice typing isn't available on this PC";
    } else if (off) {
      voiceHint.textContent = "Dictation is off on this Mac";
    } else if (writing) {
      voiceHint.textContent = "Writing it down…";
    } else if (dictating && offline) {
      voiceHint.textContent = "Listening… click Stop or press Esc";
    } else if (offlineOn) {
      voiceHint.textContent = "Speak in English (offline voice to text)";
    } else if (voiceTrouble) {
      voiceHint.textContent = DICTATION_TROUBLE;
    } else if (dictating) {
      voiceHint.textContent = "Listening… click the mic or press Esc to stop";
    } else {
      voiceHint.textContent = "Speak in English, 中文 or 廣東話 (uses your computer's dictation)";
    }
    voiceHint.title = voiceHint.textContent;
  }

  /** end: the user stopped it (mic or Esc), so the computer's dictation is ended too. */
  function stopDictation(end = false): void {
    clearTimeout(dictationTimer);
    clearTimeout(quietTimer);
    if (!dictating) return;
    if (offline) {
      // Still opening the microphone: startOffline calls it off once notes_dictate answers.
      if (starting) {
        stopWhileOpening = true;
        return;
      }
      // Stop / Esc: write it down. The note closed: throw the recording away.
      if (end) void finishOffline();
      else cancelOffline();
      return;
    }
    dictating = false;
    log("info", end ? "dictation stopped by the user" : "dictation stopped");
    api.dictationStop(end).catch(() => {});
    renderVoice();
  }

  async function startDictation(): Promise<void> {
    if (!draft || editorView.hidden || voice?.available === false) return;
    // One start at a time: a double click or Ctrl+Shift+D while a start is under way does nothing.
    if (writing || starting || dictating) return;
    // Focus the note at the cursor (the click on the mic must not move it).
    const { selectionStart, selectionEnd } = textarea;
    textarea.focus();
    textarea.setSelectionRange(selectionStart, selectionEnd);
    voiceTrouble = false;
    // Dictation types into whatever has the focus: never start it unless it is the note.
    if (document.activeElement !== textarea) {
      log("warn", "dictation not started: the note does not have the focus");
      status.textContent = "Click in the note, then try again";
      renderVoice();
      return;
    }
    starting = true;
    let useOffline = false;
    try {
      useOffline = await refreshOffline();
    } finally {
      starting = false;
    }
    if (!draft || editorView.hidden || dictating) return;
    if (useOffline) return startOffline();
    dictating = true;
    renderVoice();
    log("info", "dictation requested");
    clearTimeout(dictationTimer);
    dictationTimer = window.setTimeout(stopDictation, DICTATION_MS);
    try {
      const started = await api.dictate();
      textarea.focus();
      if (started && dictating) {
        clearTimeout(quietTimer);
        quietTimer = window.setTimeout(() => {
          if (!dictating) return;
          log("warn", "dictation: no text yet");
          voiceTrouble = true;
          renderVoice();
        }, DICTATION_QUIET_MS);
      }
      if (!started) {
        // macOS: nothing took startDictation:, so dictation is most likely off.
        voice = { available: true, off: true };
        log("warn", "dictation did not start (off in System Settings?)");
        stopDictation();
      }
    } catch (e) {
      log("error", `dictation failed: ${e}`);
      status.textContent = `Could not start dictation: ${e}`;
      // Not when our window was not in front (notes.rs): the status line already says so.
      voiceTrouble = !String(e).includes("not the window in front");
      stopDictation();
    }
    renderVoice();
  }

  // ---- Offline voice to text (Windows; Rust: voice.rs) ----
  // notes_dictate starts recording; Stop, Esc or 120 s stop it, voice_stop writes it down on a
  // background thread and the text goes in at the cursor (the input event runs autosave).

  async function startOffline(): Promise<void> {
    const d = draft;
    if (!d || editorView.hidden) return;
    offline = true;
    dictating = true;
    starting = true;
    stopWhileOpening = false;
    status.textContent = "";
    renderVoice();
    log("info", "offline voice to text: start");
    let failed: unknown = null;
    try {
      await api.dictate();
    } catch (e) {
      failed = e ?? "unknown error";
    } finally {
      starting = false;
    }
    if (failed === null && (stopWhileOpening || draft !== d || editorView.hidden)) {
      // Stop, Esc or closing the note came while the microphone was opening.
      log("info", "offline voice to text: stopped while the microphone was opening");
      cancelOffline();
      return;
    }
    if (failed !== null) {
      const e = failed;
      log("error", `offline voice to text: ${e}`);
      status.textContent = String(e);
      dictating = false;
      offline = false;
      api.dictationStop(false).catch(() => {});
      renderVoice();
      return;
    }
    clearTimeout(dictationTimer);
    dictationTimer = window.setTimeout(() => void finishOffline(), OFFLINE_MAX_MS);
  }

  function cancelOffline(): void {
    dictating = false;
    offline = false;
    log("info", "offline voice to text: cancelled");
    api.voiceCancel().catch(() => {});
    api.dictationStop(false).catch(() => {});
    renderVoice();
  }

  async function finishOffline(): Promise<void> {
    if (!offline || !dictating) return;
    clearTimeout(dictationTimer);
    dictating = false;
    writing = true;
    renderVoice();
    const d = draft;
    try {
      const text = await api.voiceStop();
      if (!d || draft !== d || editorView.hidden) {
        log("warn", "offline voice to text: the note was closed before the text was ready");
      } else {
        insertAtCursor(text);
        log("info", `offline voice to text: inserted ${text.length} characters`);
      }
    } catch (e) {
      log("error", `offline voice to text: ${e}`);
      if (draft === d) status.textContent = String(e);
    } finally {
      writing = false;
      offline = false;
      api.dictationStop(false).catch(() => {});
      renderVoice();
    }
  }

  /** Puts text at the cursor (replacing a selection), with a space before it when needed. */
  function insertAtCursor(text: string): void {
    const start = textarea.selectionStart;
    const end = textarea.selectionEnd;
    const before = textarea.value.slice(0, start);
    const gap = before && !/\s$/.test(before) ? " " : "";
    textarea.focus();
    textarea.setRangeText(gap + text, start, end, "end");
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
  }

  function toggleDictation(): void {
    if (writing) return;
    if (dictating) stopDictation(true);
    else void startDictation();
  }

  // Keep the textarea's focus and cursor when the mic is clicked.
  mic.addEventListener("mousedown", (e) => e.preventDefault());
  mic.addEventListener("click", toggleDictation);
  // List: a new note, then dictation through the same path as the editor mic.
  listMic.addEventListener("click", () => {
    log("info", "new note by voice");
    void newNote().then(startDictation);
  });
  voiceSettings.addEventListener("click", () => {
    log("info", "open Keyboard settings (Turn on Dictation)");
    invoke("shell_open_url", { url: DICTATION_SETTINGS_URL }).catch((e) => log("error", `open settings: ${e}`));
  });
  api
    .dictationStatus()
    .then((s) => {
      voice = s;
      if (!s.available) log("warn", isMac ? "dictation not available" : "voice typing not available on this PC");
      renderVoice();
    })
    .catch(() => {});
  void refreshOffline();
  renderVoice();

  // Cmd+N (Ctrl+N on Windows) makes a new note while the Notes tab is showing.
  // Cmd+Shift+D (Ctrl+Shift+D) starts or stops dictation while a note is open; Esc stops it.
  document.addEventListener("keydown", (e) => {
    if (el.hidden || e.defaultPrevented) return;
    const cmd = isMac ? e.metaKey : e.ctrlKey;
    if (cmd && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "n") {
      e.preventDefault();
      void newNote();
    } else if (cmd && e.shiftKey && !e.altKey && e.key.toLowerCase() === "d" && !editorView.hidden) {
      e.preventDefault();
      toggleDictation();
    } else if (e.key === "Escape" && (dictating || writing)) {
      // The first Esc stops dictation; the next one closes the window (main.ts).
      e.preventDefault();
      if (dictating) stopDictation(true);
    }
  });

  void refresh();
}
