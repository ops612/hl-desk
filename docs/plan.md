# Desktop app (clipboard, screenshot, notes): build plan

Status: draft, waiting for go. No code yet.
Asked by: director, 2026-11-17 (WhatsApp, "Can we make an OS app").
Director's computer: Mac. Build Mac first; Windows comes from the same code.

## Goal

One app installed on the computer, with exactly three features:

1. Clipboard history for text. Every copied item is kept and auto-deleted after 1 day. The user can change the time.
2. Screenshot with markup: numbered markers, pen, crop. The finished image can be dragged into another app.
3. Quick notes.

Done means: the director installs it on his Mac, and all three features work there without help.
Approach: build once, use forever. Few dependencies, no server, no account, nothing to keep online.

## How we build it

- Tauri 2: one codebase, uses the browser engine already in macOS (WebKit) and Windows (WebView2), so security fixes come from Apple and Microsoft, not from us. Installer about 10 MB, low memory in the tray.
- Lives in the menu bar (Mac) / tray (Windows). One shortcut opens the window with three tabs.
- All data stays on the computer, in a local SQLite file.
- GitHub Actions builds the Mac installer (universal: Apple Silicon and Intel) and the Windows installer, so no Mac is needed here to build.
- Mac signing and notarization reuse the existing Developer ID certificates and App Store Connect key (already used for HKDP Messages desktop, 2026-11-13).

## Mac-specific points

- Screenshots need the Screen Recording permission: macOS asks once, the director clicks Allow.
- Clipboard: macOS has no "copied" event, so the app checks the clipboard's change counter about twice a second (very cheap).
- Password managers mark their copies as concealed (org.nspasteboard.ConcealedType); the app skips those.
- Testing on a real Mac: nobody here has one. Options: the director tests each build, or a colleague with a Mac. Decide before step 2.

## Steps (rough estimates, to be checked after step 1)

| # | Step | Rough time | Check |
|---|------|-----------|-------|
| 1 | [x] New private repo, Tauri skeleton, menu bar icon, signed Mac + Windows installers built by GitHub Actions (done 2026-11-17, b829a36; Windows job runs by hand only) | 1-2 days | Mac .dmg installs with no warning; Windows installer opens an empty window |
| 2 | [x] (built 2026-11-17, integrate branch; not yet tried on a Mac) Clipboard: watch copies, store, search, pin, auto-delete timer, setting for the time, skip concealed copies | 3-4 days | Copy 10 items, set 1 minute, all unpinned items gone after 1 minute |
| 3 | [x] (built 2026-11-17, integrate branch; not yet tried on a Mac) Screenshot: capture area by shortcut, editor with number, pen, crop, undo, copy, save, drag-out | 5-7 days | Mark a capture with 1-2-3, crop it, drag it into WhatsApp |
| 4 | [x] (built 2026-11-17, integrate branch; not yet tried on a Mac) Notes: list, editor, autosave, search | 1-2 days | Note survives quitting and reopening the app |
| 5 | (installers ready 2026-11-17: v0.1.0 on ops612/hl-desk-releases; waiting on the director) Install on the director's Mac, fix what he finds | 2-3 days | He uses all three features without help |

Total: about 2-3 weeks. Drag-out and screen capture are the riskiest parts, so step 3 starts with a small spike (a quick test build) for both.

## Costs

- Tauri and add-ons: free, open source.
- Apple Developer account: already paid; existing Developer ID certificates reused. US$0 extra.
- Windows code-signing certificate: not needed (director is on Mac). Windows users would click "More info, Run anyway" once.
- GitHub Actions: Mac build minutes count about 10x on private repos; a few builds a month fit the free minutes.

## Open decisions (recommended first)

1. Repo: new private repo in the hobbyland-apps org.
2. Mac tester: the director tests each build (simplest), or a colleague with a Mac.
3. Name: placeholder "HL Desk" until the director picks one.
4. Auto-updates: not in the first version; add the Tauri updater later if he keeps using it.

## Decision log

- 2026-11-17: Researched existing tools first; Raycast + Snagit (or free per-OS tools) already cover the three needs. Building chosen because the director asked for our own app.
- 2026-11-17: Only the director's three requirements, nothing extra. Data stays local; clipboard skips password-manager copies.
- 2026-11-17: Tauri over Electron for the build-once-use-forever approach: system browser engine gets OS security updates, small and light. Electron was briefly chosen for reuse of the tracker app's setup, then dropped.
- 2026-11-17: "OS app" means an app installed on the computer (not a website). Director is on Mac: Mac first, Windows from the same code.
- 2026-11-17: Mac signing reuses the existing Apple Developer ID certificates; no new Apple cost.
- 2026-11-17: The director tests each Mac build on his own Mac.
- 2026-11-17: Mac only for now: the Windows CI job runs only by hand; no local Rust install; all compiling in CI.
- 2026-11-17: Built all three features the same day as parallel agents, merged on one branch. Code review fixes: the editor saves on Esc and on close; Delete in the clipboard search edits the search text; open captures are not cleaned up. Left for later: editor size on a second display, one shared SQLite connection for screenshots.
- 2026-11-17: Windows version next. Area capture on Windows uses our own overlay (capture the screens with xcap, freeze them in a full-screen window, drag a rectangle) so it works like the Mac one; the Windows Snipping Tool route was rejected because it only puts the image on the clipboard. Clipboard skips copies that password managers mark as private on Windows too. Installer stays unsigned (one "Run anyway" at install). Tested on this Windows laptop.
- 2026-11-17: Windows overlay is one window per monitor (not one window across the whole desktop), because one web view has a single pixel ratio and monitors can have different scale factors. The crop uses the frozen image's own pixel size, so it matches physical pixels at any scale. The Windows build now also runs on every push to main. The popover opens in the bottom-right corner above the taskbar on Windows.
- 2026-11-17: Windows version merged (#2) after an end-to-end test on a Windows PC; the first Windows build failed because image bytes reached the page as a number array.
- 2026-11-17: Downloads published: public repo ops612/hl-desk-releases with a GitHub Pages download page and fixed latest-release links; v0.1.0 released from main c8f4af3. Steps in docs/release.md.
- 2026-11-17: UX study of CleanShot X, Snagit, Shottr, ShareX, Snipping Tool, macOS screenshots, Maccy, Raycast, Paste, Ditto, Win+V, Quick Note, Raycast Notes and Tot saved in docs/ux-study.md. Scope rule: build only what serves the director's three requirements; everything else parked in docs/ideas.md.
- 2026-11-17: 0.1.3 approved: instant capture overlay and no silent cancel; select/delete markers and a Drag to share bar; Keep change with Undo and a "Copied" notice; pin to keep the popover open and dated notes; capture-key setting (Print Screen / Cmd+Shift+4, guided) plus "Mark up in HL Desk" after a device screenshot. Keep change uses Undo, not a confirm.
- 2026-11-17: 0.1.3 Windows test fixes: the magnifier worked but sampled only 15 px (a flat colour on plain wallpaper), now 21 px with centre guide lines; the toast is built hidden, shown without taking the focus once its page has drawn (closed and logged if it never does), top-right on Windows (Windows notifications cover the bottom-right), 8 s for a device screenshot; Keep Undo puts rows back with their own id, time and pin, logs the change and the undo, and the undo bar stays while hovered. Pre-created overlay windows skipped for now (needs a reusable page and session flow, not testable without Rust here). macOS Screen Recording card shared by the Screenshot tab and the welcome: list picture, Open Screen Recording settings, re-check on focus, asks once more per start, One more step with Restart HL Desk.
- 2026-11-17: GitHub Actions free minutes ran out on the account; to keep cost at zero the code repo ops612/hl-desk was made public (full-history secret scan clean). Signing secrets stay in GitHub's encrypted secrets.
- 2026-11-17: 0.1.3 tested on a Windows 10 PC (capture, magnifier, cancel notice, marker/pen delete, drag to share, Win+Shift+S prompt, Keep undo, copy closes at once, pin, notes). Code review: 10 findings, 0 high; 9 fixed. Windows 10 snips come from svchost.exe and count when the copy is image-only. macOS untested on a real Mac.
