import { defineConfig } from "@playwright/test";

const baseURL = process.env.BRUNN_REVIEW_BASE_URL;
if (!baseURL) throw new Error("BRUNN_REVIEW_BASE_URL must point at an existing Brunn Web test server");

export default defineConfig({
  testDir: "./e2e",
  testMatch: "dreamer-review.spec.ts",
  workers: 1,
  retries: 0,
  timeout: 30_000,
  outputDir: "test-results/dreamer-review",
  use: { baseURL, browserName: "chromium", screenshot: "only-on-failure", trace: "retain-on-failure" },
});
