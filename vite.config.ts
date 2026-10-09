import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Tauri expects a fixed dev endpoint (see src-tauri config devUrl) and a static dist build.
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**", "**/vendor/**"],
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "safari16",
    sourcemap: false,
  },
});
