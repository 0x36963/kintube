import { defineConfig, devices } from "@playwright/test";

/**
 * End-to-end tests against the real stack: Vite dev server → Rust API → Postgres,
 * with sign-in emails read back from Mailpit. Start Postgres + Mailpit first
 * (`make dev-up`); the API and web servers are started here if not already running.
 */
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? "github" : "list",
  use: {
    baseURL: "http://localhost:5173",
    trace: "retain-on-failure",
  },
  projects: [
    { name: "desktop", use: { ...devices["Desktop Chrome"] } },
    { name: "mobile", use: { ...devices["Pixel 7"] } },
  ],
  webServer: [
    {
      command: "cargo run -q -p api -- serve",
      cwd: "../backend",
      url: "http://127.0.0.1:8787/api/health",
      reuseExistingServer: !process.env.CI,
      timeout: 300_000,
    },
    {
      // The production build, as deployed. Vite's dev server compiles modules on first request,
      // which made the first test on a cold CI runner flaky. Locally, a running `make web` is reused.
      command: "pnpm run build && pnpm exec vite preview --port 5173 --strictPort",
      url: "http://localhost:5173",
      reuseExistingServer: !process.env.CI,
    },
  ],
});
