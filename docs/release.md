# Releasing HL Desk

Downloads live in the public repo ops612/hl-desk-releases (installers and the download page only; this repo stays private).

- Download page: https://ops612.github.io/hl-desk-releases/ (GitHub Pages, from `site/index.html` here, copied to `index.html` there)
- Fixed links that always serve the newest release:
  - https://github.com/ops612/hl-desk-releases/releases/latest/download/HL-Desk-mac.dmg
  - https://github.com/ops612/hl-desk-releases/releases/latest/download/HL-Desk-setup.exe

## Steps for a new version

1. Bump `version` in `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml`, merge to main, wait for the main build to pass.
2. Download both artifacts from that run: `gh run download <run id> -R ops612/hl-desk -D <folder>`.
3. Rename them to the fixed names `HL-Desk-mac.dmg` and `HL-Desk-setup.exe`.
4. `gh release create v<version> -R ops612/hl-desk-releases HL-Desk-mac.dmg HL-Desk-setup.exe --title "HL Desk <version>" --notes "<what changed>" --latest`
5. Check both links above return the new files, and the page shows the new version.

If the page text changes, edit `site/index.html` here and copy it to the releases repo's `index.html`.
