import { fireEvent, render, screen } from "@testing-library/react";
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  Outlet,
  RouterProvider,
  useRouterState,
} from "@tanstack/react-router";
import { describe, expect, it, vi } from "vitest";
import { MarkdownView } from "../components/MarkdownView";
import {
  resolveWorkspaceEntryLink,
  WorkspaceEntryView,
} from "../components/WorkspaceEntryView";

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

  it("turns wiki links into safe internal entry callbacks", () => {
    const onEntryLink = vi.fn();
    const { container } = render(
      <MarkdownView
        markdown="Open [[Projects/Straylight/Plan#Next|the plan]]."
        onEntryLink={onEntryLink}
      />,
    );
    const anchor = container.querySelector("a");
    expect(anchor).toHaveTextContent("the plan");
    expect(anchor?.getAttribute("href")).not.toContain("straylight:");
    fireEvent.click(anchor!);
    expect(onEntryLink).toHaveBeenCalledWith({
      kind: "wiki",
      target: "Projects/Straylight/Plan#Next",
    });
  });

  it("routes root-relative markdown-suffix links through the entry callback", () => {
    const onEntryLink = vi.fn();
    const { container } = render(
      <MarkdownView
        markdown="[Plan](/sources/Projects/Plan.markdown)"
        onEntryLink={onEntryLink}
      />,
    );

    fireEvent.click(container.querySelector("a")!);

    expect(onEntryLink).toHaveBeenCalledWith({
      kind: "markdown",
      target: "/sources/Projects/Plan.markdown",
    });
  });

  it("rewrites direct entry references before sanitizing and opens them internally", () => {
    const onEntryLink = vi.fn();
    const { container } = render(
      <MarkdownView
        markdown="[Exact entry](entry:019fc7a8-c466-7132-af69-91f731d31281)"
        onEntryLink={onEntryLink}
      />,
    );
    const anchor = container.querySelector("a");
    expect(anchor?.getAttribute("href")).not.toMatch(/^entry:/);
    fireEvent.click(anchor!);
    expect(onEntryLink).toHaveBeenCalledWith({
      kind: "markdown",
      target: "entry:019fc7a8-c466-7132-af69-91f731d31281",
    });
  });

  it("does not parse wiki-link syntax inside inline code", () => {
    const view = renderMarkdown("`[[Not a link]]`");
    expect(view.querySelector("code")).toHaveTextContent("[[Not a link]]");
    expect(view.querySelector("a")).toBeNull();
  });

  it("resolves relative and imported-vault entry paths", () => {
    expect(
      resolveWorkspaceEntryLink("sources/Projects/Current/Entry.md", {
        kind: "markdown",
        target: "../Related.md#decision",
      }),
    ).toMatchObject({ path: "sources/Projects/Related.md" });
    expect(
      resolveWorkspaceEntryLink("sources/Projects/Current/Entry.md", {
        kind: "markdown",
        target: "../Related.md%23decision",
      }),
    ).toMatchObject({ path: "sources/Projects/Related.md" });
    expect(
      resolveWorkspaceEntryLink("sources/Projects/Current/Entry.md", {
        kind: "wiki",
        target: "Projects/Straylight/Plan",
      }),
    ).toEqual({
      path: "Projects/Straylight/Plan.md",
      alternatePaths: ["sources/Projects/Straylight/Plan.md"],
      fallbackQuery: "Projects/Straylight/Plan",
    });
    expect(
      resolveWorkspaceEntryLink("sources/Projects/Current/Entry.md", {
        kind: "wiki",
        target: "Projects/Straylight/Plan.markdown",
      }),
    ).toEqual({
      path: "Projects/Straylight/Plan.markdown",
      alternatePaths: ["sources/Projects/Straylight/Plan.markdown"],
      fallbackQuery: "Projects/Straylight/Plan.markdown",
    });
    expect(
      resolveWorkspaceEntryLink("sources/Projects/Current/Entry.md", {
        kind: "markdown",
        target: "straylight://entry/sources/Projects/Plan.markdown",
      }),
    ).toEqual({
      path: "sources/Projects/Plan.markdown",
      alternatePaths: undefined,
      fallbackQuery: "sources/Projects/Plan.markdown",
    });
    expect(
      resolveWorkspaceEntryLink("sources/Projects/Current/Entry.md", {
        kind: "wiki",
        target: "Sibling",
      }),
    ).toEqual({
      path: "sources/Projects/Current/Sibling.md",
      alternatePaths: ["Sibling.md", "sources/Sibling.md"],
      linkTarget: "Sibling",
      fallbackQuery: "Sibling",
    });
  });

  it("routes internal links from shared entry readers through Explore", async () => {
    const rootRoute = createRootRoute({ component: Outlet });
    const readerRoute = createRoute({
      getParentRoute: () => rootRoute,
      path: "/",
      component: ReaderFixture,
    });
    const exploreRoute = createRoute({
      getParentRoute: () => rootRoute,
      path: "/explore",
      component: LocationProbe,
    });
    const router = createRouter({
      routeTree: rootRoute.addChildren([readerRoute, exploreRoute]),
      history: createMemoryHistory({ initialEntries: ["/"] }),
    });
    render(<RouterProvider router={router} />);

    fireEvent.click(await screen.findByRole("link", { name: "Sibling" }));
    const location = await screen.findByTestId("location");
    expect(location).toHaveAttribute("data-pathname", "/explore");
    const params = new URLSearchParams(location.getAttribute("data-search") ?? "");
    expect(params.get("entryPath")).toBe("sources/Folder/Sibling.md");
    expect(params.get("linkTarget")).toBe("Sibling");
    expect(params.get("alternatePaths")?.split("\n")).toEqual([
      "Sibling.md",
      "sources/Sibling.md",
    ]);
  });

  it("keeps the rendered html stable for identical input", () => {
    const markdown = "- one\n- two\n\n`code`";
    const first = renderMarkdown(markdown).innerHTML;
    const second = renderMarkdown(markdown).innerHTML;
    expect(first).toBe(second);
  });
});

function LocationProbe() {
  const location = useRouterState({ select: (state) => state.location });
  return (
    <output
      data-testid="location"
      data-pathname={location.pathname}
      data-search={location.searchStr}
    />
  );
}

function ReaderFixture() {
  return (
    <WorkspaceEntryView
      entry={{
        reference: "entry:current",
        path: "sources/Folder/Current.md",
        title: "Current",
        version: 1,
        version_ref: "entry-version:current-v1",
        content_hash: "sha256:test",
        media_type: "text/markdown",
        view: "full",
        status: "complete",
        text: "Open [[Sibling]].",
        metadata: {},
        updated_at: "2026-08-03T00:00:00Z",
      }}
    />
  );
}
