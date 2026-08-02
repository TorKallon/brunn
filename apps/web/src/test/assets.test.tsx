import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { defaultMe, installApiMock, renderApp } from "./renderApp";

const binary = {
  entry_ref: "entry:receipt-1",
  path: "Trips/Switzerland/Receipts/train.jpg",
  title: "train.jpg",
  version: 3,
  content_hash:
    "sha256:6e568e1f67fba258184c78181539e5e8fdee447e49bb706fc0ea34fbf12336a5",
  size_bytes: 3,
  media_type: "image/jpeg",
  updated_at: "2026-07-24T18:30:00Z",
  metadata: {
    companion_path: ".straylight/binaries/train.md",
    description_status: "provided",
    provenance: "Scanned original",
  },
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe("workspace binaries", () => {
  it("lists current binaries without a pinned session and exposes metadata", async () => {
    installApiMock({
      "GET /api/v1/workspace/binaries": (request: Request) => {
        const query = new URL(request.url).searchParams;
        expect(query.get("offset")).toBe("0");
        expect(query.get("limit")).toBe("100");
        expect(query.has("session_id")).toBe(false);
        return { status: "complete", data: { binaries: [binary] } };
      },
      "GET /api/v1/workspace/binaries/entry%3Areceipt-1": {
        status: "complete",
        data: binary,
      },
    });
    const user = userEvent.setup();
    renderApp("/assets", "asset-token");

    expect(
      await screen.findByRole("row", { name: "Inspect train.jpg" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Provided")).toBeInTheDocument();
    await user.click(screen.getByRole("row", { name: "Inspect train.jpg" }));

    const heading = await screen.findByRole("heading", {
      name: "Binary metadata",
    });
    const section = heading.closest("section");
    expect(section).not.toBeNull();
    expect(
      within(section!).getByLabelText("Stored binary metadata"),
    ).toHaveTextContent("Scanned original");
    expect(
      within(section!).getByText("entry:receipt-1"),
    ).toBeInTheDocument();
  });

  it("downloads and verifies current exact bytes through the workspace route", async () => {
    const NativeURL = URL;
    const createObjectURL = vi.fn(() => "blob:binary-download");
    const revokeObjectURL = vi.fn();
    class DownloadURL extends NativeURL {
      static createObjectURL = createObjectURL;
      static revokeObjectURL = revokeObjectURL;
    }
    vi.stubGlobal("URL", DownloadURL);
    const anchorClick = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(() => undefined);
    let downloadRequested = false;

    installApiMock({
      "GET /api/v1/workspace/binaries": {
        status: "complete",
        data: { binaries: [binary] },
      },
      "GET /api/v1/workspace/binaries/entry%3Areceipt-1/content": (
        request: Request,
      ) => {
        downloadRequested = true;
        expect(request.credentials).toBe("same-origin");
        expect(request.headers.get("authorization")).toBeNull();
        return new Response(new Uint8Array([0xff, 0xd8, 0xff]), {
          headers: {
            "Content-Type": "image/jpeg",
            "Content-Length": "3",
            "Content-Disposition": 'attachment; filename="train.jpg"',
            "X-CarryState-SHA256":
              "6e568e1f67fba258184c78181539e5e8fdee447e49bb706fc0ea34fbf12336a5",
          },
        });
      },
    });
    const user = userEvent.setup();
    renderApp("/assets", "asset-token");

    await user.click(
      await screen.findByRole("button", { name: "Download train.jpg" }),
    );
    await vi.waitFor(() => expect(downloadRequested).toBe(true));
    expect(createObjectURL).toHaveBeenCalledOnce();
    expect(anchorClick).toHaveBeenCalledOnce();
  });

  it("keeps binary reads enabled for a read-only credential", async () => {
    const me = structuredClone(defaultMe);
    me.data.read_only = true;
    me.data.capabilities = ["open", "query", "read", "status"];
    me.data.active_scope.access = "read_only";
    installApiMock({
      "GET /api/v1/me": me,
      "GET /api/v1/workspace/binaries": {
        status: "complete",
        data: { binaries: [binary] },
      },
    });
    renderApp("/assets", "readonly-token");

    expect(
      await screen.findByText("Read-only credential active"),
    ).toBeInTheDocument();
    expect(
      await screen.findByRole("button", { name: "Download train.jpg" }),
    ).toBeEnabled();
  });
});
