import { describe, expect, it } from "vitest";
import html from "../../index.html?raw";
import appShellSource from "../components/AppShell.tsx?raw";
import appearanceSource from "../lib/appearance.ts?raw";
import authPagesSource from "../pages/AuthPages.tsx?raw";
import css from "../styles.css?raw";

describe("Still Water web brand", () => {
  it("publishes the vector favicon first, raster fallbacks, touch icon, and OG image", () => {
    const svgIndex = html.indexOf('href="/favicon.svg"');
    const pngIndex = html.indexOf('href="/favicon-32.png"');

    expect(svgIndex).toBeGreaterThan(-1);
    expect(pngIndex).toBeGreaterThan(svgIndex);
    expect(html).toContain('href="/favicon-16.png"');
    expect(html).toContain('href="/apple-touch-icon.png"');
    expect(html).toContain('property="og:image" content="https://brunn.ai/og.png"');
    expect(appearanceSource).toContain('appearance === "dark" ? "#06152c"');
  });

  it("uses the approved well mark and has no retired beam mark reference", () => {
    const retiredMark = ["brunn", "mark.png"].join("-");

    expect(appShellSource).toContain('src="/brunn-well-128.webp"');
    expect(authPagesSource).toContain('src="/brunn-well-128.webp"');
    expect(`${appShellSource}\n${authPagesSource}`).not.toContain(retiredMark);
  });

  it("uses the depth surface, well token, and one sidebar waterline", () => {
    expect(css).toContain("--well: #02060f");
    expect(css).toMatch(/\.sidebar\s*\{[\s\S]*?radial-gradient\(circle at 14% 8%/u);
    expect(css).toContain("--night-depth-glow: rgb(49 88 217 / 24%)");
    expect(css).toMatch(
      /linear-gradient\(180deg, var\(--night-depth-top\) 0%, var\(--night-depth-bottom\) 100%\)/u,
    );
    expect(css).toMatch(/\.sidebar > \.brand\s*\{[\s\S]*?border-bottom: 1px solid transparent/u);
    expect(
      css.match(/border-image: linear-gradient\(90deg, transparent, var\(--signal-300\), transparent\) 1/gu),
    ).toHaveLength(1);
  });
});
