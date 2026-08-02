import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, vi } from "vitest";

Object.defineProperty(window, "scrollTo", { value: vi.fn(), writable: true });

afterEach(() => {
  cleanup();
  window.sessionStorage.clear();
  document.cookie = "straylight_csrf=; Max-Age=0; Path=/";
  document.cookie = "__Host-straylight_csrf=; Max-Age=0; Path=/; Secure";
  window.history.replaceState({}, "", "/");
  vi.unstubAllGlobals();
});
