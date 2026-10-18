import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    // Тесты scripts/ — на node:test, их запускает `pnpm test:scripts`.
    include: ["src/**/*.{test,spec}.?(c|m)[jt]s?(x)"],
  },
});
