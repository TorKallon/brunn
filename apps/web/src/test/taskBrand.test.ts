import { describe, expect, it } from "vitest";
import { toneForStatus } from "../components/StateViews";
import css from "../styles.css?raw";

const taskCss = (css.split("/* Agent-first task surfaces */")[1] ?? "").split(
  "@media (max-width: 1100px)",
)[0];

describe("Night Signal task contracts", () => {
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
