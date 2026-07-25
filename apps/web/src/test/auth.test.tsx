import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { AUTH_SESSION_KEY } from "../lib/auth";
import { installApiMock, renderApp } from "./renderApp";

describe("token authentication", () => {
  it("keeps the access token in session storage and authenticates requests", async () => {
    const fetchMock = installApiMock();
    const user = userEvent.setup();
    renderApp("/work");

    expect(await screen.findByRole("heading", { name: "Straylight" })).toBeInTheDocument();
    await user.type(screen.getByLabelText("Access token"), "session-secret");
    await user.click(screen.getByRole("button", { name: "Connect" }));

    expect(await screen.findByRole("heading", { name: "Work" })).toBeInTheDocument();
    expect(window.sessionStorage.getItem(AUTH_SESSION_KEY)).toBe("session-secret");
    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    const meCall = fetchMock.mock.calls.find(([input]) => new URL(String(input), window.location.origin).pathname === "/api/v1/me");
    expect(new Headers(meCall?.[1]?.headers).get("Authorization")).toBe("Bearer session-secret");
  });
});
