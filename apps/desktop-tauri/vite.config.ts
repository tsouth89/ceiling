/// <reference types="vitest" />
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, searchForWorkspaceRoot } from "vite";
import react from "@vitejs/plugin-react";

const here = dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  plugins: [react()],
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
    fs: {
      // Vite denies `?raw` imports outside the app root. SBS-1048 pins
      // TEST_PROVIDER_CATALOG to rust/src/core/provider.rs; keep this
      // allowlist limited to that crate path plus the usual roots.
      allow: [
        here,
        searchForWorkspaceRoot(here),
        resolve(here, "../../rust/src/core"),
      ],
    },
  },
  clearScreen: false,
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
  },
});
