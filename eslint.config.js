import js from "@eslint/js";
import svelte from "eslint-plugin-svelte";
import globals from "globals";
import ts from "typescript-eslint";
import svelteConfig from "./svelte.config.js";

export default ts.config(
  {
    ignores: [
      "dist/",
      "coverage/",
      "node_modules/",
      "src-tauri/",
      "vendor/",
      "docs/",
      "patches/",
      "scripts/",
      // Local makepkg copies the whole tree into packaging/arch/{src,pkg}.
      "packaging/",
    ],
  },
  js.configs.recommended,
  ts.configs.recommendedTypeChecked,
  svelte.configs.recommended,
  {
    languageOptions: {
      globals: { ...globals.browser },
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
        extraFileExtensions: [".svelte"],
      },
    },
  },
  {
    rules: {
      // Positional snippet/callback parameters that are deliberately unused are `_`-prefixed.
      "@typescript-eslint/no-unused-vars": ["error", { argsIgnorePattern: "^_" }],
      // Every flagged Map is scratch state built and discarded inside one $derived computation;
      // a reactive SvelteMap there would only add tracking overhead.
      "svelte/prefer-svelte-reactivity": "off",
    },
  },
  {
    files: ["**/*.svelte", "**/*.svelte.ts"],
    languageOptions: {
      parserOptions: {
        parser: ts.parser,
        svelteConfig,
      },
    },
  },
  {
    // typescript-eslint cannot see .svelte component/snippet types (they resolve to `any`
    // without the Svelte language service); svelte-check type-checks these files instead.
    files: ["**/*.svelte"],
    rules: {
      "@typescript-eslint/no-unsafe-argument": "off",
      "@typescript-eslint/no-unsafe-assignment": "off",
      "@typescript-eslint/no-unsafe-call": "off",
      "@typescript-eslint/no-unsafe-member-access": "off",
      "@typescript-eslint/no-unsafe-return": "off",
    },
  },
  {
    // Build tooling runs in Node and sits outside the app's tsconfig project.
    files: ["*.config.js", "*.config.ts"],
    extends: [ts.configs.disableTypeChecked],
    languageOptions: { globals: { ...globals.node } },
  },
);
