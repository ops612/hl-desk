import { defineConfig } from "vite";

// Tauri expects a fixed port and the built files in ../dist (see src-tauri/tauri.conf.json).
// Four pages: the popover (index.html), the screenshot editor window (editor.html), the
// Windows area-capture overlay (overlay.html) and the corner toast (toast.html).
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    target: ["es2021", "safari15", "chrome105"],
    outDir: "dist",
    rolldownOptions: {
      input: { main: "index.html", editor: "editor.html", overlay: "overlay.html", toast: "toast.html" },
    },
  },
});
