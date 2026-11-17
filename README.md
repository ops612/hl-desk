# HL Desk

A small menu-bar (macOS) / tray (Windows) app with three tabs: Clipboard, Screenshot and Notes.
Click the icon or press Cmd+Shift+Space (Ctrl+Shift+Space on Windows) to open it.
"HL Desk" is a placeholder name. All data stays on the computer. The plan is in `docs/plan.md`.

Built with Tauri 2 (Rust + the system web view) and a Vite + TypeScript front end with no framework.

## Local development

Node 22 is enough for the front end:

```
npm ci
npm run typecheck
npm run build
```

Running the whole app (`npm run tauri dev`) needs Rust and, on Windows, the MSVC build tools.

## How CI builds it

`.github/workflows/build.yml`:

- **macOS** (every push to `main`, and by hand): compiles a universal binary (Apple Silicon + Intel),
  then builds the `.app` and `.dmg`, signs them with the Developer ID certificate, notarizes and staples
  both, checks them with `spctl` and uploads the `.dmg` as the artifact `HL-Desk-macos-universal`,
  together with the signed updater bundle (`HL Desk.app.tar.gz` and its `.sig`).
  If a signing secret is missing the job fails and names it; it never ships an unsigned build.
  It also uploads the resolved `Cargo.lock`.
- **Windows** (every push to `main`, and by hand): builds an unsigned NSIS installer,
  artifact `HL-Desk-windows`, with its updater signature (`-setup.exe.sig`). Windows asks
  "More info, Run anyway" once.
- Both jobs sign the updater files with `TAURI_SIGNING_PRIVATE_KEY` and
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (repository secrets) and fail if a `.sig` is missing.
  Releasing and `latest.json`: `docs/release.md`.

Repository secrets used by the macOS job (names only):

- `MAC_DEVID_APP_P12`: base64 of the Developer ID Application .p12
- `MAC_CERT_PASSWORD`: the .p12 password
- `ASC_KEY_P8`: the App Store Connect API key (.p8 contents, raw or base64)
- `ASC_KEY_ID`: that key's ID
- `ASC_ISSUER_ID`: the App Store Connect issuer ID

## Architecture

Each feature lives in its own files and touches nothing else:

| Feature | Front end | Rust |
|---|---|---|
| Clipboard | `src/features/clipboard/` | `src-tauri/src/clipboard.rs` |
| Screenshot | `src/features/screenshot/` | `src-tauri/src/screenshot.rs` |
| Notes | `src/features/notes/` | `src-tauri/src/notes.rs` |

Shared files (change only in a separate, reviewed PR): `src/main.ts` (tabs), `src-tauri/src/lib.rs`
(tray, shortcut, plugins, routing), `src-tauri/src/db.rs`, `package.json`, `Cargo.toml`,
`tauri.conf.json`, `src-tauri/capabilities/default.json`, `src-tauri/Info.plist`, `src-tauri/src/settings.rs`.

- **Front end:** `src/features/<feature>/index.ts` exports `mount(el)`. The tab calls it once, the
  first time it is shown, with the tab's panel element. The panel is a flex column filling the
  popover (about 380 x 520); a feature may import its own CSS file from its folder.
- **Shell (`index.html`, `src/main.ts`, `src/styles.css`):** header with the "HL DESK" wordmark and a
  settings sheet (shortcuts, version, Quit), the segmented tab control (arrow keys, Cmd+1/2/3, last
  tab kept in localStorage), and Esc to hide the window. Esc is handled on `document` in the bubble
  phase: a feature that wants Esc first (for example to clear its search) handles it on its own
  element and calls `preventDefault()`. When a tab is shown again, the element in its panel marked
  `data-autofocus` gets focus.
- **Theme tokens** (CSS variables on `:root`, values in `docs/ui.md`): `--ink`, `--muted`, `--border`,
  `--bg`, `--surface`, `--indigo`, `--indigo-hover`, `--indigo-tint`, `--red`, plus `--radius` (10px)
  `--radius-sm` (8px) and `--inset` (12px, the side margin of every view). Defined only in
  `src/styles.css` (the editor window imports it too); features never copy them. Font: IBM Plex Sans
  400/500/600 bundled via `@fontsource/ibm-plex-sans`.
- **Shared classes** (`src/styles.css`):
  - `.btn-primary`: the one indigo-filled button per view. `.btn`: plain bordered button.
    `.icon-btn`: 28px square icon button (put an `aria-label` on it).
  - `.field`: text input or `textarea`, with the indigo focus ring.
  - `.select`: compact 26px select with a drawn chevron (for footer settings such as Keep).
  - `.toolbar`: a flex row padded 12px all round, for search plus buttons at the top of a view.
  - `.scroll`: the part of a view that scrolls (flex: 1), inset 12px each side.
  - `.row`: a 12px list row with 8px corners (a `button`, `div` or `li`); `.row.selected` or
    `aria-selected="true"` is the keyboard-selected row.
  - `.hint`: 11px muted text. `.empty`: centred empty-state message.
  - `.footer`: the bottom hint line of a panel (11px, grey band, full width, pushed to the bottom).
  - `.undo-bar`: the dark "Item deleted ... Undo" bar above the footer (5 s, then the delete happens).
  - `.keycap`: one key of a shortcut. `.switch`: an on/off `input type="checkbox" role="switch"`.
  - Layout: a panel is a column of `.toolbar`, `.scroll`, `.footer`. Never use negative margins to
    reach the window edge; the toolbar and scroll do the insetting.
- **Rust:** `<feature>.rs` has `init(app)`, called once at start-up, and `commands()`, which returns
  `tauri::generate_handler![...]`. To add a command, write `#[tauri::command] fn <feature>_<name>(...)`
  in the feature file and list it in that `generate_handler!`. `lib.rs` routes every command by its
  `<feature>_` prefix, so it never needs editing. Call it from the front end with
  `invoke("<feature>_<name>", args)` from `@tauri-apps/api/core`.
- **Storage:** `db::open(app, "<feature>.db")` opens that feature's own SQLite file (rusqlite, bundled)
  in the app data folder.
- **Already installed:** global-shortcut, clipboard-manager, fs (scoped to app data, temp, Pictures,
  Desktop), drag (`tauri-plugin-drag` by CrabNebula, drags files out of the window), `xcap` for screen
  capture, on macOS `objc2-app-kit` for `NSPasteboard` (change count, concealed copies), and on
  Windows `windows-sys` for the clipboard formats that mark a copy private.
- **Windows area capture:** `screenshot.rs` freezes every monitor with xcap and opens one
  transparent overlay window per monitor (`overlay.html`, `src/overlay/`, capability `overlay.json`):
  dim layer and hint at once, the frozen pixels (raw RGBA) fill in after; crosshair lines and a
  magnifier. The user drags a rectangle (Esc, right-click or the capture key again cancels; a
  cancel shows a "Capture cancelled" toast). macOS uses `screencapture -i` instead.
- **Capture keys and device screenshots:** `src-tauri/src/screenshot/keys.rs` (Ctrl/Cmd+Shift+2
  always; Print Screen / Cmd+Shift+4 as a setting, with a hint when the system still owns it),
  `screenshot/watch.rs` ("Mark up in HL Desk" after Win+Shift+S via the clipboard, or a new file in
  the macOS screenshot folder), `screenshot/toast.rs` (the corner toast: `toast.html`, `src/toast/`,
  capability `toast.json`). The settings live in `src/features/screenshot/settings.ts` (mounted in
  the Settings sheet) and `settings.json`.
- **macOS:** `src-tauri/Info.plist` sets `LSUIElement` (no Dock icon) and the Screen Recording prompt text.
- **Log:** `src-tauri/src/applog.rs` writes `<app data>/hl-desk.log` (UTC time, level, message; cut
  to the last ~500 KB past 1 MB; on Windows `%APPDATA%\com.hobbyland.hldesk\hl-desk.log`). Use
  `applog::info/warn/error` in Rust, never `eprintln!` (a release build has no console); pages call
  `invoke("app_log", { level, message })`. Every capture step is logged, and a failed capture
  shows the main window with the error on the Screenshot tab.
- **Settings and first run:** `src-tauri/src/settings.rs` (commands `settings_*`) keeps
  `<app data>/settings.json` ("welcome seen"). Open at login uses `tauri-plugin-autostart`
  (macOS: a LaunchAgent in `~/Library/LaunchAgents`; Windows: value "HL Desk" under
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`), registered with `--hidden`, and is read
  from the OS, never stored. The first run (no settings.json) turns it on and opens the window with
  the welcome card. Started with `--hidden`: tray icon only; started by hand: the window opens.
- **Tray icon:** `src-tauri/icons/tray/`, drawn by `npm run icons:tray` (`scripts/make-tray-icons.mjs`):
  macOS an 18pt template image, Windows an indigo "HL" square sized to the monitor scale.
- **macOS Screen Recording:** before each capture `screenshot.rs` checks
  `CGPreflightScreenCaptureAccess`; if not allowed it calls `CGRequestScreenCaptureAccess` and shows
  the permission card on the Screenshot tab instead of capturing. Minimum macOS is 10.15.
- **One instance:** `tauri-plugin-single-instance`. A second launch shows the running window;
  `hl-desk.exe --capture` (first or second launch) starts an area capture, for testing from a script.
- **Popover:** the main window hides when it loses focus. On Windows it has no maximize or minimize
  button and a fixed size.
