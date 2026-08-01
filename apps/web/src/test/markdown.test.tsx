import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { MarkdownView } from "../components/MarkdownView";

function renderMarkdown(markdown: string, className?: string) {
  const { container } = render(
    <MarkdownView markdown={markdown} className={className} />,
  );
  const view = container.querySelector(".markdown-view");
  if (!view) throw new Error("markdown view did not render");
  return view;
}

describe("MarkdownView", () => {
  it("renders bold links and emphasis", () => {
    const view = renderMarkdown(
      "**[OpenAI ships o5](https://example.com/story)** _quietly_",
    );
    const anchor = view.querySelector("strong a");
    expect(anchor).not.toBeNull();
    expect(anchor).toHaveAttribute("href", "https://example.com/story");
    expect(anchor).toHaveTextContent("OpenAI ships o5");
    expect(view.querySelector("em")).toHaveTextContent("quietly");
  });

  it("applies the optional class alongside markdown-view", () => {
    const view = renderMarkdown("plain text", "inline");
    expect(view.classList.contains("markdown-view")).toBe(true);
    expect(view.classList.contains("inline")).toBe(true);
  });

  it("strips script tags and event handler attributes", () => {
    const view = renderMarkdown(
      'before <script>window.pwned = true;</script>' +
        '<img src="x" onerror="window.pwned = true" /> after',
    );
    expect(view.querySelector("script")).toBeNull();
    expect(view.innerHTML).not.toContain("onerror");
    expect(view.innerHTML).not.toContain("pwned");
    expect(view.textContent).toContain("before");
    expect(view.textContent).toContain("after");
  });

  it("forbids svg and math subtrees", () => {
    const view = renderMarkdown(
      '<svg><animate onbegin="window.pwned = true" /></svg><math><mi>x</mi></math>',
    );
    expect(view.querySelector("svg")).toBeNull();
    expect(view.querySelector("math")).toBeNull();
  });

  it("neutralizes javascript: hrefs", () => {
    const view = renderMarkdown("[click me](javascript:window.pwned/*x*/())");
    const anchor = view.querySelector("a");
    expect(anchor?.getAttribute("href") ?? "").not.toContain("javascript:");
    expect(view.textContent).toContain("click me");
  });

  it("opens external links in a new tab with a safe rel", () => {
    const view = renderMarkdown(
      "[external](https://example.com/a) and [internal](/briefings)",
    );
    const anchors = Array.from(view.querySelectorAll("a"));
    const external = anchors.find(
      (anchor) => anchor.getAttribute("href") === "https://example.com/a",
    );
    const internal = anchors.find(
      (anchor) => anchor.getAttribute("href") === "/briefings",
    );
    expect(external).toHaveAttribute("target", "_blank");
    expect(external).toHaveAttribute("rel", "noreferrer noopener");
    expect(internal).not.toHaveAttribute("target");
    expect(internal).not.toHaveAttribute("rel");
  });

  it("keeps the rendered html stable for identical input", () => {
    const markdown = "- one\n- two\n\n`code`";
    const first = renderMarkdown(markdown).innerHTML;
    const second = renderMarkdown(markdown).innerHTML;
    expect(first).toBe(second);
  });
});
