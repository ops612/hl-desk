import { defineConfig } from "vite";

// Tauri expects a fixed port and the built files in ../dist (see src-tauri/tauri.conf.json).
// Two pages: the popover (index.html) and the screenshot editor window (editor.html).
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    target: ["es2021", "safari15", "chrome105"],
    outDir: "dist",
    rolldownOptions: {
      input: { main: "index.html", editor: "editor.html" },
    },
  },
});
