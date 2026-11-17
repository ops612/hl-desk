// macOS Screen Recording card: the same flow on the Screenshot tab and in the welcome.
// Rust (src-tauri/src/screenshot.rs): screenshot_permission_status ("granted", "restart" or
// "denied"; while denied it asks macOS once more per app start, so HL Desk is in the list),
// screenshot_open_permission_settings (the Screen Recording pane) and screenshot_restart.
// - denied: a picture of the Screen Recording list with HL Desk's switch on, "Allow screen
//   recording", one line why, and "Open Screen Recording settings".
// - restart: allowed, but macOS's own "Quit & Reopen" was skipped: "One more step" with
//   "Restart HL Desk" and "Check again".
// Each time HL Desk gets the focus back while the card shows, it checks again.
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./permission.css";

type Status = "granted" | "restart" | "denied";

// The Screen Recording list in System Settings, drawn in our tokens: another app with its
// switch off, then HL Desk with its switch on.
const ART = `<svg class="perm-art" viewBox="0 0 232 72" role="img" aria-label="System Settings, Screen Recording: HL Desk switched on">
  <rect x="0.5" y="0.5" width="231" height="71" rx="8" fill="#fff" stroke="#E3E6EB"/>
  <text x="12" y="17" font-size="9" font-weight="500" fill="#5B6474">Screen Recording</text>
  <rect x="12" y="27" width="16" height="16" rx="4" fill="#E3E6EB"/>
  <rect x="34" y="32" width="58" height="6" rx="3" fill="#E3E6EB"/>
  <rect x="192" y="28" width="26" height="14" rx="7" fill="#E3E6EB"/>
  <circle cx="199" cy="35" r="5.5" fill="#fff"/>
  <line x1="12" y1="49.5" x2="220" y2="49.5" stroke="#E3E6EB"/>
  <rect x="12" y="52" width="16" height="16" rx="4" fill="#2F4CC8"/>
  <path transform="translate(12 52)" d="M2 4h2v3h2V4h2v8H6V9H4v3H2zM10 4h2v6h2v2h-4z" fill="#fff"/>
  <text x="34" y="63.5" font-size="10" font-weight="500" fill="#0B1220">HL Desk</text>
  <rect x="192" y="53" width="26" height="14" rx="7" fill="#2F4CC8"/>
  <circle cx="211" cy="60" r="5.5" fill="#fff"/>
</svg>`;

/** Fills `host` (hidden while allowed) with the card. Returns `check`, which asks Rust again. */
export function mountPermission(host: HTMLElement): { check: () => void } {
  host.classList.add("perm-card");
  host.innerHTML = `
    ${ART}
    <div class="perm-denied">
      <strong>Allow screen recording</strong>
      <p class="hint">Without it, a capture shows only your wallpaper. Turn on HL Desk in the list.</p>
      <div class="perm-actions">
        <button class="btn-primary perm-open" type="button">Open Screen Recording settings</button>
      </div>
      <p class="hint perm-done" hidden>Turned it on already? <button class="perm-link perm-restart" type="button">Restart HL Desk</button></p>
    </div>
    <div class="perm-reopen" hidden>
      <strong>One more step</strong>
      <p class="hint">Screen recording is on. Restart HL Desk so captures can use it.</p>
      <div class="perm-actions">
        <button class="btn-primary perm-restart" type="button">Restart HL Desk</button>
        <button class="btn perm-check" type="button">Check again</button>
      </div>
    </div>`;
  const denied = host.querySelector<HTMLElement>(".perm-denied")!;
  const reopen = host.querySelector<HTMLElement>(".perm-reopen")!;
  const done = host.querySelector<HTMLElement>(".perm-done")!;

  const apply = (status: Status) => {
    host.hidden = status === "granted";
    denied.hidden = status !== "denied";
    reopen.hidden = status !== "restart";
  };
  const check = () => {
    invoke<Status>("screenshot_permission_status")
      .then(apply)
      .catch(() => {});
  };

  host.querySelector(".perm-open")!.addEventListener("click", () => {
    // Back from System Settings, macOS may still say "not allowed" until HL Desk restarts.
    done.hidden = false;
    invoke("screenshot_open_permission_settings").catch(() => {});
  });
  for (const button of host.querySelectorAll(".perm-restart")) {
    button.addEventListener("click", () => {
      invoke("screenshot_restart").catch(() => {});
    });
  }
  host.querySelector(".perm-check")!.addEventListener("click", check);

  // Back from System Settings: check again (only while this card is on screen).
  void getCurrentWindow().onFocusChanged(({ payload: focused }) => {
    if (focused && host.getClientRects().length > 0) check();
  });
  return { check };
}
