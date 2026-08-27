import { defineConfig, devices } from "@playwright/test";

const baseURL = process.env.STRAYLIGHT_GATE12_BASE_URL;

if (!baseURL) {
  throw new Error(
    "STRAYLIGHT_GATE12_BASE_URL is required and must point at the disposable Web stack",
  );
}

export default defineConfig({
  testDir: "./e2e",
  testMatch: "gate12d-messaging.spec.ts",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 120_000,
  expect: { timeout: 20_000 },
  outputDir: "test-results/gate12d-messaging",
  reporter: [
    ["list"],
    ["json", { outputFile: "artifacts/gate12d-messaging-results.json" }],
  ],
  use: {
    ...devices["Desktop Chrome"],
    baseURL,
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
    video: "retain-on-failure",
  },
});
