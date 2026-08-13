import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import {
  historicalPublishedDocumentFixture,
  publishedDocumentFixture,
} from "./documentFixtures";
import { defaultMe, installApiMock, renderApp } from "./renderApp";

const documentPath = "/api/v1/workspace/documents/switzerland-itinerary";

describe("request-directed published documents", () => {
  it("opens a stable human route with readable Markdown and provenance", async () => {
    installApiMock({ [`GET ${documentPath}`]: publishedDocumentFixture });
    const { container } = renderApp("/documents/switzerland-itinerary");

    expect(
      await screen.findByRole("heading", {
        level: 1,
        name: "Switzerland summer itinerary",
      }),
    ).toBeInTheDocument();
    expect(screen.getByText(/^Published .+ · Updated .+$/)).toBeInTheDocument();
    expect(screen.getByText(/polished two-week plan/)).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { level: 2, name: "First week" }),
    ).toBeInTheDocument();
    expect(screen.getAllByText("Zürich", { exact: false })).toHaveLength(2);
    expect(container.querySelector("script")).toBeNull();
    expect(container.innerHTML).not.toContain("window.pwned");

    const bodyLink = screen.getByRole("link", { name: "Check service" });
    expect(bodyLink).toHaveAttribute("target", "_blank");
    expect(bodyLink).toHaveAttribute("rel", "noreferrer noopener");

    const sources = screen.getByRole("region", { name: "Sources" });
    expect(
      within(sources).getByRole("link", { name: "Swiss Federal Railways" }),
    ).toHaveAttribute("href", "https://www.sbb.ch/en");
    expect(
      within(sources).getByRole("link", { name: "Trip planning notes" }),
    ).toHaveAttribute(
      "href",
      "/explore?entryRef=entry%3A11111111-1111-4111-8111-111111111111",
    );
    expect(
      within(sources).queryByRole("link", { name: "Unsafe source" }),
    ).toBeNull();
    expect(
      within(sources).queryByRole("link", {
        name: "Credential-bearing source",
      }),
    ).toBeNull();
    expect(screen.queryByText(/entry:11111111/)).toBeNull();
    expect(screen.queryByText(/Documents\/switzerland-itinerary/)).toBeNull();
  });

  it("pins a historical version and links back to the stable latest URL", async () => {
    let request: Request | undefined;
    installApiMock({
      [`GET ${documentPath}`]: (received: Request) => {
        request = received;
        return historicalPublishedDocumentFixture(2);
      },
    });
    renderApp("/documents/switzerland-itinerary?version=2");

    expect(
      await screen.findByText("Viewing version 2 of 3"),
    ).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open latest" })).toHaveAttribute(
      "href",
      "/documents/switzerland-itinerary",
    );
    await waitFor(() => expect(request).toBeDefined());
    expect(new URL(request!.url).searchParams.get("version")).toBe("2");
  });

  it("ignores a non-positive version and loads the current document", async () => {
    let request: Request | undefined;
    installApiMock({
      [`GET ${documentPath}`]: (received: Request) => {
        request = received;
        return publishedDocumentFixture;
      },
    });
    renderApp("/documents/switzerland-itinerary?version=0");

    expect(
      await screen.findByRole("heading", { name: "Switzerland summer itinerary" }),
    ).toBeInTheDocument();
    await waitFor(() => expect(request).toBeDefined());
    expect(new URL(request!.url).searchParams.has("version")).toBe(false);
  });

  it("shows a document-specific error instead of falling back to Search", async () => {
    installApiMock({
      [`GET ${documentPath}`]: {
        status: 404,
        body: {
          error: {
            code: "document_not_found",
            message: "This published document is unavailable.",
          },
        },
      },
    });
    renderApp("/documents/switzerland-itinerary");

    expect(
      await screen.findByText("This published document is unavailable."),
    ).toBeInTheDocument();
    expect(screen.getByText("document_not_found")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Search" })).toBeNull();
  });

  it("returns to the exact document deep link after signing in", async () => {
    let authenticated = false;
    let documentRequest: Request | undefined;
    const session = {
      status: "complete",
      data: {
        user: defaultMe.data.user,
        expires_at: "2099-08-08T19:30:00Z",
      },
    };
    installApiMock({
      "GET /api/v1/auth/session": () =>
        authenticated
          ? session
          : {
              status: 401,
              body: {
                error: {
                  code: "authentication_required",
                  message: "A valid web session is required.",
                },
              },
            },
      "POST /api/v1/auth/login": () => {
        authenticated = true;
        return session;
      },
      [`GET ${documentPath}`]: (received: Request) => {
        documentRequest = received;
        return historicalPublishedDocumentFixture(2);
      },
    });
    const user = userEvent.setup();
    const { router } = renderApp(
      "/documents/switzerland-itinerary?version=2",
    );

    await user.type(await screen.findByLabelText("Email"), "aether@example.com");
    expect(router.state.location.pathname).toBe("/login");
    expect(router.state.location.search).toMatchObject({
      redirect: "/documents/switzerland-itinerary?version=2",
    });
    await user.type(screen.getByLabelText("Password"), "a sufficiently long password");
    await user.click(screen.getByRole("button", { name: "Sign in" }));

    expect(
      await screen.findByRole("heading", { name: "Switzerland summer itinerary" }),
    ).toBeInTheDocument();
    await waitFor(() => expect(documentRequest).toBeDefined());
    expect(new URL(documentRequest!.url).searchParams.get("version")).toBe("2");
    expect(router.state.location.pathname).toBe(
      "/documents/switzerland-itinerary",
    );
    expect(router.state.location.search).toMatchObject({ version: 2 });
  });
});
