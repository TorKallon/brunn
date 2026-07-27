import { fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { installApiMock, renderApp } from "./renderApp";

describe("workspace writes", () => {
  it("captures durable text as an ordinary Markdown receipt", async () => {
    let payload: Record<string, unknown> | undefined;
    installApiMock({
      "POST /api/v1/workspace/capture": async (request: Request) => {
        payload = (await request.json()) as Record<string, unknown>;
        return {
          status: "committed",
          corpus_revision: "generation:21",
          data: {
            entry_ref: "entry:capture",
            version_ref: "entry-version:capture-v1",
            path: "Inbox/Captures/2026/07/26/capture.md",
            version: 1,
            content_hash: "sha256:capture",
            workspace_generation: 21,
            search_status: "ready",
            no_op: false,
          },
        };
      },
    });
    const user = userEvent.setup();
    renderApp("/capture", "write-token");

    await user.type(await screen.findByRole("textbox", { name: "Source" }), "Gmail:thread-1");
    await user.type(screen.getByRole("textbox", { name: "Intent" }), "remember_decision");
    await user.type(
      screen.getByRole("textbox", { name: "Content" }),
      "The confirmed flight is LX39.",
    );
    await user.click(screen.getByRole("button", { name: "Capture" }));

    expect(
      await screen.findByText("Inbox/Captures/2026/07/26/capture.md"),
    ).toBeInTheDocument();
    await waitFor(() =>
      expect(payload).toMatchObject({
        content: "The confirmed flight is LX39.",
        intent: "remember_decision",
        source: {
          locator: "Gmail:thread-1",
          authority: "owner",
        },
      }),
    );
  });

  it("writes exact Markdown with optimistic version and metadata", async () => {
    let payload: Record<string, unknown> | undefined;
    installApiMock({
      "POST /api/v1/workspace/write": async (request: Request) => {
        payload = (await request.json()) as Record<string, unknown>;
        return {
          status: "committed",
          data: {
            entry_ref: "entry:status",
            version_ref: "entry-version:status-v4",
            path: "Projects/Status.md",
            version: 4,
            content_hash: "sha256:status",
            workspace_generation: 22,
            search_status: "lexical_ready_semantic_pending",
            no_op: false,
          },
        };
      },
    });
    const user = userEvent.setup();
    renderApp("/capture", "write-token");

    await screen.findByRole("heading", { name: "Write" });
    await user.click(screen.getByRole("tab", { name: "Markdown" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Path" }), {
      target: { value: "Projects/Status.md" },
    });
    fireEvent.change(screen.getByRole("spinbutton", { name: "Expected version" }), {
      target: { value: "3" },
    });
    fireEvent.change(screen.getByRole("textbox", { name: "Content" }), {
      target: { value: "# Status\n\nCurrent." },
    });
    const metadata = screen.getByRole("textbox", { name: "Metadata (JSON)" });
    fireEvent.change(metadata, {
      target: { value: '{"kind":"project","authority":"owner"}' },
    });
    await user.click(screen.getByRole("button", { name: "Write" }));

    await waitFor(() => expect(payload).toBeDefined());
    expect(payload).toMatchObject({
      path: "Projects/Status.md",
      content: "# Status\n\nCurrent.",
      media_type: "text/markdown",
      expected_version: 3,
      metadata: { kind: "project", authority: "owner" },
    });
    expect(
      await screen.findByText("Lexical Ready Semantic Pending"),
    ).toBeInTheDocument();
  });

  it("uploads exact binary bytes with description and provenance", async () => {
    const fetchMock = installApiMock({
      "POST /api/v1/workspace/binaries": {
        status: "committed",
        data: {
          entry_ref: "entry:receipt",
          version_ref: "entry-version:receipt-v1",
          path: "Trips/Receipts/train.png",
          version: 1,
          content_hash: "sha256:receipt",
          size_bytes: 4,
          media_type: "image/png",
          companion: {
            entry_ref: "entry:receipt-description",
            path: ".straylight/binaries/receipt.md",
            version: 1,
          },
          workspace_generation: 23,
          description_status: "provided",
          no_op: false,
        },
      },
    });
    const user = userEvent.setup();
    renderApp("/capture", "write-token");

    await screen.findByRole("heading", { name: "Write" });
    await user.click(screen.getByRole("tab", { name: "Binary" }));
    const file = new File([new Uint8Array([1, 2, 3, 4])], "train.png", {
      type: "image/png",
    });
    fireEvent.change(screen.getByLabelText(/File/), {
      target: { files: [file] },
    });
    const path = screen.getByRole("textbox", { name: "Workspace path" });
    fireEvent.change(path, {
      target: { value: "Trips/Receipts/train.png" },
    });
    fireEvent.change(screen.getByRole("textbox", { name: "Description" }), {
      target: { value: "Train receipt for expenses" },
    });
    fireEvent.change(screen.getByRole("textbox", { name: "Provenance" }), {
      target: { value: "Scanned original" },
    });
    expect(screen.getByText("train.png · 4 B")).toBeInTheDocument();
    const upload = screen.getByRole("button", { name: "Upload" });
    expect(upload).toBeEnabled();
    fireEvent.submit(upload.closest("form")!);

    expect(
      await screen.findByText(".straylight/binaries/receipt.md"),
    ).toBeInTheDocument();
    const uploadCall = fetchMock.mock.calls.find(([input]) =>
      String(input).endsWith("/api/v1/workspace/binaries"),
    );
    expect(uploadCall).toBeDefined();
    const form = uploadCall?.[1]?.body;
    expect(form).toBeInstanceOf(FormData);
    expect((form as FormData).get("path")).toBe(
      "Trips/Receipts/train.png",
    );
    expect((form as FormData).get("description")).toBe(
      "Train receipt for expenses",
    );
    expect((form as FormData).get("provenance")).toBe("Scanned original");
    expect((form as FormData).get("file")).toBe(file);
  });
});
