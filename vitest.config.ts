import { defineConfig, mergeConfig } from "vitest/config";
import viteConfig from "./vite.config.ts";

// Unit tests for src/**/*.ts. The shared Vite config supplies the Svelte plugin, which compiles
// runes in *.svelte.ts stores; jsdom stands in for the web view (Tauri IPC is mocked per test with
// @tauri-apps/api/mocks). Coverage feeds SonarQube Cloud through coverage/lcov.info.
export default mergeConfig(
  viteConfig,
  defineConfig({
    // Resolve Svelte's client runtime, so $state/$derived behave as they do in the web view.
    resolve: { conditions: ["browser"] },
    test: {
      environment: "jsdom",
      include: ["src/**/*.test.ts"],
      restoreMocks: true,
      unstubGlobals: true,
      coverage: {
        provider: "v8",
        include: ["src/**/*.ts"],
        // src/test holds shared test fixtures; every production module counts, main.ts included.
        exclude: ["src/**/*.test.ts", "src/test/**"],
        reporter: ["text", "text-summary", "lcov"],
        reportsDirectory: "coverage",
      },
    },
  }),
);
