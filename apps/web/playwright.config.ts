import { defineConfig, devices } from "@playwright/test";

const baseURL = process.env.BRUNN_GATE12_BASE_URL;

if (!baseURL) {
  throw new Error(
    "BRUNN_GATE12_BASE_URL is required and must point at the disposable Web stack",
  );
}

export default defineConfig({
  testDir: "./e2e",
  testMatch: "gate12c.spec.ts",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 120_000,
  expect: { timeout: 15_000 },
  outputDir: "test-results/gate12c",
  reporter: [
    ["list"],
    ["json", { outputFile: "artifacts/gate12c-results.json" }],
  ],
  use: {
    ...devices["Desktop Chrome"],
    baseURL,
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
    video: "retain-on-failure",
  },
});
