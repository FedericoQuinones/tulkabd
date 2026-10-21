import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Tauri expects a fixed dev port and must not clear the terminal output.
export default defineConfig(({ mode }) => ({
  plugins: [svelte()],
  resolve: mode === "test" ? { conditions: ["browser"] } : undefined,
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**", "**/crates/**", "**/vendor/**"] },
  },
  test: {
    include: ["src/**/*.test.ts"],
    globals: true,
  },
}));
