# Releasing HL Desk

Downloads live in the public repo ops612/hl-desk-releases (installers and the download page only; this repo stays private).

- Download page: https://ops612.github.io/hl-desk-releases/ (GitHub Pages, from `site/index.html` here, copied to `index.html` there)
- Fixed links that always serve the newest release:
  - https://github.com/ops612/hl-desk-releases/releases/latest/download/HL-Desk-mac.dmg
  - https://github.com/ops612/hl-desk-releases/releases/latest/download/HL-Desk-setup.exe
  - https://github.com/ops612/hl-desk-releases/releases/latest/download/latest.json (the update manifest the app reads)

## Auto-update (since 0.1.4)

The app checks `latest.json` on the latest release 10 s after start and every 6 hours
(`src/updater.ts`, tauri-plugin-updater). A newer version downloads in the background; the user
clicks "Restart to update" to install it. Windows installs with the same `HL-Desk-setup.exe`
(passive mode), macOS with `HL-Desk-mac.app.tar.gz`.

- Every update file is signed in CI with the updater key (repository secrets `TAURI_SIGNING_PRIVATE_KEY`
  and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`); the public key is in `src-tauri/tauri.conf.json`
  (`plugins.updater.pubkey`). Losing the private key means no more auto-updates: users would have
  to install a build with a new key by hand.
- `latest.json` must sit in the same release as the files it points to, and that release must be
  marked latest. A latest release without `latest.json` stops every update check (the app shows
  "Could not check for updates" in Settings).
- 0.1.3 and earlier have no updater: those users install 0.1.4 once by hand.

## Steps for a new version (Windows PowerShell)

1. Bump the version in `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and `package.json` +
   `package-lock.json` (`npm version <version> --no-git-tag-version`), merge to main, wait for the
   main build to pass.
2. Download the artifacts from that run into an empty folder:

   ```
   $v = "0.1.4"
   gh run download <run id> -R ops612/hl-desk -n HL-Desk-macos-universal -n HL-Desk-windows -D rel
   ```

   `rel\HL-Desk-macos-universal` holds `HL Desk_<v>_universal.dmg`, `HL Desk.app.tar.gz` and
   `HL Desk.app.tar.gz.sig`; `rel\HL-Desk-windows` holds `HL Desk_<v>_x64-setup.exe` and its `.sig`.
3. Rename them to the fixed names:

   ```
   Move-Item "rel\HL-Desk-macos-universal\HL Desk_${v}_universal.dmg" rel\HL-Desk-mac.dmg
   Move-Item "rel\HL-Desk-macos-universal\HL Desk.app.tar.gz" rel\HL-Desk-mac.app.tar.gz
   Move-Item "rel\HL-Desk-macos-universal\HL Desk.app.tar.gz.sig" rel\HL-Desk-mac.app.tar.gz.sig
   Move-Item "rel\HL-Desk-windows\HL Desk_${v}_x64-setup.exe" rel\HL-Desk-setup.exe
   Move-Item "rel\HL-Desk-windows\HL Desk_${v}_x64-setup.exe.sig" rel\HL-Desk-setup.exe.sig
   ```

4. Write `latest.json` (it checks both signatures against the public key and fails if one does not match):

   ```
   node scripts/make-latest-json.mjs --version $v --notes "<what changed>" --mac rel\HL-Desk-mac.app.tar.gz --win rel\HL-Desk-setup.exe --out rel\latest.json
   ```

5. Create the release with the installers, the macOS update bundle and `latest.json` (the `.sig`
   files are not uploaded: their contents are inside `latest.json`):

   ```
   gh release create v$v -R ops612/hl-desk-releases rel\HL-Desk-mac.dmg rel\HL-Desk-setup.exe rel\HL-Desk-mac.app.tar.gz rel\latest.json --title "HL Desk $v" --notes "<what changed>" --latest
   ```

6. Check the three links above return the new files (`curl.exe -sL https://github.com/ops612/hl-desk-releases/releases/latest/download/latest.json`
   shows the new version), and the page shows the new version.

If the page text changes, edit `site/index.html` here and copy it to the releases repo's `index.html`.
