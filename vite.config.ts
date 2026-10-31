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
    // Lazy reader chunks are larger than an eager page should be: Mermaid's ELK layout engine
    // (~1.4 MB, loaded only for `layout: elk` diagrams), the C++ grammar (~800 kB, 60 kB gzipped)
    // and Mermaid's other layout code. They load from the app bundle, never the network, and only
    // when a document uses them; the initial bundle stays far below this.
    chunkSizeWarningLimit: 1500,
    rolldownOptions: {
      output: {
        // The document reader is loaded on demand (DocumentSurface imports the renderer lazily, and
        // the renderer imports KaTeX, Shiki and Mermaid lazily). These groups keep each of those
        // libraries in one named chunk instead of dozens of fragments; none is in the initial
        // bundle. Mermaid's per-diagram chunks and Shiki's per-language grammars stay split so
        // only what a document uses is fetched.
        codeSplitting: {
          groups: [
            // The reader's KaTeX, not the copy Mermaid bundles for diagram labels.
            { name: "katex", test: /(?<!mermaid[\\/])node_modules[\\/](katex|rehype-katex)[\\/]/ },
            {
              name: "shiki",
              test: /node_modules[\\/](@shikijs[\\/](core|engine-javascript|types|vscode-textmate)|oniguruma-to-es|oniguruma-parser|regex|regex-recursion|regex-utilities)[\\/]/,
            },
            {
              name: "markdown",
              test: /node_modules[\\/](unified|bail|trough|vfile[\w-]*|unist-[\w-]+|mdast-[\w-]+|hast-[\w-]+|micromark[\w-]*|remark-[\w-]+|rehype-(sanitize|stringify)|github-slugger|property-information|space-separated-tokens|comma-separated-tokens|html-void-elements|zwitch|ccount|character-entities[\w-]*|decode-named-character-reference|devlop|markdown-table|longest-streak|escape-string-regexp|trim-lines|stringify-entities|is-plain-obj|extend|web-namespaces)[\\/]/,
            },
          ],
        },
      },
    },
  },
});
