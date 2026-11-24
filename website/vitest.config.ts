import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "node",
    include: ["tests/contracts/**/*.test.ts"],
    // Whole-site contracts cover every generated tool page as well as the guides.
    testTimeout: 30_000,
    restoreMocks: true,
  },
});
