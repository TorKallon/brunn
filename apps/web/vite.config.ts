import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

const apiProxyTarget =
  process.env.STRAYLIGHT_API_PROXY_TARGET?.trim() || "http://127.0.0.1:8080";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      "/api": {
        target: apiProxyTarget,
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/api/, ""),
      },
    },
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}"],
    setupFiles: "./src/test/setup.ts",
    css: true,
    restoreMocks: true,
  },
});
