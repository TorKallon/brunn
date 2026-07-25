import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { defaultMe, installApiMock, renderApp } from "./renderApp";

const session = {
  status: "complete",
  corpus_revision: "revision:asset-snapshot",
  data: {
    id: "session_asset",
    title: "Trip expense review",
    status: "active",
    scope_id: "scope_1",
    corpus_revision: "revision:asset-snapshot",
    updated_at: "2026-07-24T19:00:00Z",
  },
};

const asset = {
  asset_ref: "asset:asset_1",
  version: 3,
  path: "Trips/Switzerland/Receipts/train.jpg",
  content_hash: "sha256:6e568e1f67fba258184c78181539e5e8fdee447e49bb706fc0ea34fbf12336a5",
  size_bytes: 3,
  media_type: "image/jpeg",
  stored_at: "2026-07-24T18:30:00Z",
  description_available: true,
  description_status: "complete",
  description_method: "openai_responses",
  access_count: 8,
  last_used_at: "2026-07-24T18:45:00Z",
  most_used_rank: 2,
  least_used_rank: 17,
  least_recently_used_rank: 5,
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe("asset audit surface", () => {
  it("requires a pinned session and offers existing sessions", async () => {
    installApiMock({
      "GET /api/v1/sessions": {
        status: "complete",
        data: { items: [session.data] },
      },
    });
    renderApp("/assets", "read-token");

    expect(
      await screen.findByRole("heading", { name: "Pinned session required" }),
    ).toBeInTheDocument();
    expect(
      await screen.findByRole("row", {
        name: "Audit assets pinned to Trip expense review",
      }),
    ).toBeInTheDocument();
  });

  it("lists projected assets, exposes metadata, and downloads through the guarded route", async () => {
    const NativeURL = URL;
    const createObjectURL = vi.fn(() => "blob:asset-download");
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
      "GET /api/v1/sessions/session_asset": session,
      "GET /api/v1/assets": (request: Request) => {
        const query = new URL(request.url).searchParams;
        expect(query.get("session_id")).toBe("session_asset");
        expect(query.get("offset")).toBe("0");
        expect(query.get("limit")).toBe("100");
        return {
          status: "complete",
          corpus_revision: "revision:asset-snapshot",
          data: { items: [asset], offset: 0, limit: 100, has_more: false, total: 1 },
        };
      },
      "GET /api/v1/assets/asset%3Aasset_1": (request: Request) => {
        expect(new URL(request.url).searchParams.get("session_id")).toBe(
          "session_asset",
        );
        return {
          status: "complete",
          corpus_revision: "revision:asset-snapshot",
          data: {
            asset_ref: asset.asset_ref,
            version: asset.version,
            path: asset.path,
            content_hash: asset.content_hash,
            size_bytes: asset.size_bytes,
            media_type: asset.media_type,
            stored_at: asset.stored_at,
            corpus_revision: "revision:asset-snapshot",
            etag: "\"asset-etag\"",
            metadata: {
              stable_import_id: "switzerland-2026",
              original_path: asset.path,
            },
            sources: [
              {
                role: "source_native",
                source_ref: "source:source_receipt",
                title: "Swiss rail receipt",
                source_kind: "native_asset",
                path: asset.path,
              },
            ],
            download: {
              path: "/v1/assets/asset:asset_1/versions/3/content",
              supports_ranges: true,
              integrity_header: "x-carrystate-sha256",
            },
          },
        };
      },
      "GET /api/v1/assets/asset%3Aasset_1/versions/3/content": (request: Request) => {
        downloadRequested = true;
        expect(new URL(request.url).searchParams.get("session_id")).toBe(
          "session_asset",
        );
        expect(request.headers.get("authorization")).toBe("Bearer asset-token");
        return new Response(new Uint8Array([0xff, 0xd8, 0xff]), {
          headers: {
            "Content-Type": "image/jpeg",
            "Content-Length": "3",
            "Content-Disposition": 'attachment; filename="train.jpg"',
            "X-CarryState-SHA256": "6e568e1f67fba258184c78181539e5e8fdee447e49bb706fc0ea34fbf12336a5",
          },
        });
      },
    });
    const user = userEvent.setup();
    renderApp("/assets/session_asset", "asset-token");

    expect(
      await screen.findByRole("row", {
        name: "Inspect train.jpg metadata",
      }),
    ).toBeInTheDocument();
    expect(screen.getByText("Openai Responses")).toBeInTheDocument();
    expect(screen.getByText("8 accesses")).toBeInTheDocument();
    expect(screen.getByText("Most used #2")).toBeInTheDocument();

    await user.click(
      screen.getByRole("row", { name: "Inspect train.jpg metadata" }),
    );
    const detailHeading = await screen.findByRole("heading", {
      name: "Asset metadata",
    });
    const detailSection = detailHeading.closest("section");
    expect(detailSection).not.toBeNull();
    expect(screen.getByText("Swiss rail receipt")).toBeInTheDocument();
    expect(
      screen.getByText("revision:asset-snapshot"),
    ).toBeInTheDocument();
    expect(within(detailSection!).getByText("Complete")).toBeInTheDocument();
    expect(within(detailSection!).getByText("8")).toBeInTheDocument();

    await user.click(
      screen.getAllByRole("button", { name: "Download train.jpg" })[0],
    );
    expect(downloadRequested).toBe(true);
    expect(createObjectURL).toHaveBeenCalledOnce();
    expect(anchorClick).toHaveBeenCalledOnce();
  });

  it("keeps asset reads and downloads enabled for a read-only credential", async () => {
    const me = structuredClone(defaultMe);
    me.data.read_only = true;
    me.data.capabilities = ["read", "memory.query", "memory.verify"];
    me.data.active_scope.access = "read_only";
    installApiMock({
      "GET /api/v1/me": me,
      "GET /api/v1/sessions/session_asset": session,
      "GET /api/v1/assets": {
        status: "complete",
        data: { items: [asset], offset: 0, limit: 100, has_more: false },
      },
    });
    renderApp("/assets/session_asset", "readonly-token");

    expect(
      await screen.findByText("Read-only credential active"),
    ).toBeInTheDocument();
    expect(
      await screen.findByRole("button", { name: "Download train.jpg" }),
    ).toBeEnabled();
  });

  it("renders empty and API error states", async () => {
    installApiMock({
      "GET /api/v1/sessions/session_asset": session,
      "GET /api/v1/assets": {
        status: "complete",
        data: { items: [], offset: 0, limit: 100, has_more: false },
      },
    });
    const empty = renderApp("/assets/session_asset", "asset-token");
    expect(
      await screen.findByText("No projected assets"),
    ).toBeInTheDocument();
    empty.unmount();

    installApiMock({
      "GET /api/v1/sessions/session_asset": session,
      "GET /api/v1/assets": {
        status: 503,
        body: {
          error: {
            code: "asset_index_unavailable",
            message: "Asset projection is temporarily unavailable.",
          },
        },
      },
    });
    renderApp("/assets/session_asset", "asset-token");
    expect(
      await screen.findByText("Unable to load assets"),
    ).toBeInTheDocument();
    expect(screen.getByText("asset_index_unavailable")).toBeInTheDocument();
  });
});
