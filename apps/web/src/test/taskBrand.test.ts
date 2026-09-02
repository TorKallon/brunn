import { describe, expect, it } from "vitest";
import { toneForStatus } from "../components/StateViews";
import css from "../styles.css?raw";

const taskCss = (css.split("/* Agent-first task surfaces */")[1] ?? "").split(
  "@media (max-width: 1100px)",
)[0];

describe("Still Water task contracts", () => {
  it("uses design tokens rather than new color literals", () => {
    expect(taskCss).not.toBe("");
    expect(taskCss).not.toMatch(/#[0-9a-f]{3,8}\b/iu);
    expect(taskCss).not.toMatch(/\b(?:rgb|hsl)a?\(/iu);
  });

  it("uses the status ramp, never brand blue, for completion", () => {
    const completionRule = taskCss.match(/\.task-action-complete\s*\{([^}]*)\}/u)?.[1];
    const dropRule = taskCss.match(/\.task-action-drop,[^{]+\{([^}]*)\}/u)?.[1];
    expect(completionRule).toContain("var(--green-line)");
    expect(completionRule).toContain("var(--green-soft)");
    expect(completionRule).not.toMatch(/--(?:signal|blue|link)/u);
    expect(dropRule).toContain("var(--red-line)");
    expect(dropRule).toContain("var(--red-soft)");
    expect(dropRule).not.toMatch(/--(?:signal|blue|link)/u);
    expect(toneForStatus("done")).toBe("success");
    expect(toneForStatus("success")).toBe("success");
  });
});

describe("Brunn wordmark contract", () => {
  it("uses the lowercase serif wordmark without introducing color literals", () => {
    const wordmarkRule = css.match(/\.brand strong\s*\{([^}]*)\}/u)?.[1] ?? "";
    expect(wordmarkRule).toContain("font-family: var(--font-display)");
    expect(wordmarkRule).toContain("font-weight: 500");
    expect(wordmarkRule).toContain("letter-spacing: -0.03em");
    expect(wordmarkRule).not.toMatch(/#[0-9a-f]{3,8}\b/iu);
    expect(wordmarkRule).not.toMatch(/\b(?:rgb|hsl)a?\(/iu);
  });
});
