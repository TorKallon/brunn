import { describe, expect, it, vi } from "vitest";
import { createApiClient } from "../lib/api";
import { publishedDocumentFixture } from "./documentFixtures";

function installFetch() {
  const fetchMock = vi.fn(
    async (_input: RequestInfo | URL, _init?: RequestInit) =>
      new Response(JSON.stringify(publishedDocumentFixture), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
  );
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function requestOf(fetchMock: ReturnType<typeof installFetch>): Request {
  const [input, init] = fetchMock.mock.calls[0];
  return input instanceof Request
    ? input
    : new Request(new URL(String(input), "https://brunn.test"), init);
}

describe("published document api client", () => {
  it("fetches the current document through the authenticated workspace API", async () => {
    const fetchMock = installFetch();

    const envelope = await createApiClient().documentGet("switzerland-itinerary");
    const request = requestOf(fetchMock);
    const url = new URL(request.url);

    expect(request.method).toBe("GET");
    expect(url.pathname).toBe(
      "/api/v1/workspace/documents/switzerland-itinerary",
    );
    expect(url.search).toBe("");
    expect(request.credentials).toBe("same-origin");
    expect(request.headers.get("Authorization")).toBeNull();
    expect(envelope.data.title).toBe("Switzerland summer itinerary");
  });

  it("encodes the slug and pins an explicitly requested version", async () => {
    const fetchMock = installFetch();

    await createApiClient().documentGet("summer plan", 2);
    const url = new URL(requestOf(fetchMock).url);

    expect(url.pathname).toBe("/api/v1/workspace/documents/summer%20plan");
    expect(url.searchParams.get("version")).toBe("2");
  });
});
