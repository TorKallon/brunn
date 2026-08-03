import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { scheduleSessionExpiry } from "../components/AuthBoundary";
import { createApiClient } from "../lib/api";
import { defaultMe, installApiMock, renderApp } from "./renderApp";

const unauthenticated = {
  status: 401,
  body: {
    error: {
      code: "authentication_required",
      message: "A valid web session is required.",
    },
  },
};

const sessionEnvelope = {
  status: "complete",
  data: {
    user: {
      id: "user_1",
      display_name: "Aether",
      username: "aether",
      email: "aether@example.com",
    },
    expires_at: "2099-07-12T18:00:00Z",
  },
};

describe("web authentication", () => {
  it("signs in with an email and password without browser token storage", async () => {
    window.sessionStorage.setItem("straylight.access_token", "legacy-bearer-secret");
    const requests: Request[] = [];
    installApiMock({
      "GET /api/v1/auth/session": unauthenticated,
      "POST /api/v1/auth/login": async (request: Request) => {
        requests.push(request);
        expect(await request.json()).toEqual({
          email: "aether@example.com",
          password: "correct horse battery staple",
        });
        return sessionEnvelope;
      },
      "GET /api/v1/me": (request: Request) => {
        requests.push(request);
        return defaultMe;
      },
    });
    const user = userEvent.setup();
    const { queryClient } = renderApp("/login");

    expect(
      await screen.findByRole("heading", { name: "Sign in" }),
    ).toBeInTheDocument();
    queryClient.setQueryData(["previous-user", "private"], { secret: true });
    await user.type(screen.getByLabelText("Email"), "aether@example.com");
    await user.type(
      screen.getByLabelText("Password"),
      "correct horse battery staple",
    );
    await user.click(screen.getByRole("button", { name: "Sign in" }));

    expect(
      await screen.findByRole("heading", { name: "Aether’s Straylight" }),
    ).toBeInTheDocument();
    expect(window.sessionStorage).toHaveLength(0);
    expect(queryClient.getQueryData(["previous-user", "private"])).toBeUndefined();
    expect(queryClient.getMutationCache().getAll()).toHaveLength(0);
    expect(requests).not.toHaveLength(0);
    for (const request of requests) {
      expect(request.credentials).toBe("same-origin");
      expect(request.headers.get("Authorization")).toBeNull();
    }
  });

  it("shows a generic error for invalid credentials", async () => {
    installApiMock({
      "GET /api/v1/auth/session": unauthenticated,
      "POST /api/v1/auth/login": {
        status: 401,
        body: {
          error: {
            code: "invalid_credentials",
            message: "Internal account lookup detail",
          },
        },
      },
    });
    const user = userEvent.setup();
    renderApp("/work");

    await user.type(await screen.findByLabelText("Email"), "unknown@example.com");
    await user.type(screen.getByLabelText("Password"), "wrong-password");
    await user.click(screen.getByRole("button", { name: "Sign in" }));

    expect(
      await screen.findByRole("alert"),
    ).toHaveTextContent("The email or password is incorrect.");
    expect(screen.queryByText("Internal account lookup detail")).not.toBeInTheDocument();
  });

  it("does not describe invalid email input as an outage", async () => {
    installApiMock({
      "GET /api/v1/auth/session": unauthenticated,
      "POST /api/v1/auth/login": {
        status: 400,
        body: {
          error: {
            code: "invalid_request",
            message: "Internal validation detail",
          },
        },
      },
    });
    const user = userEvent.setup();
    renderApp("/work");

    await user.type(await screen.findByLabelText("Email"), "owner@example.com");
    await user.type(screen.getByLabelText("Password"), "wrong-password");
    await user.click(screen.getByRole("button", { name: "Sign in" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Enter a valid email address.",
    );
    expect(screen.queryByText("Internal validation detail")).not.toBeInTheDocument();
  });

  it("requests recovery without revealing whether an account exists", async () => {
    const recoveryRequest = vi.fn(async (request: Request) => {
      expect(await request.json()).toEqual({ identifier: "aether@example.com" });
      return { status: "complete", data: { status: "accepted" } };
    });
    installApiMock({
      "POST /api/v1/auth/forgot-password": recoveryRequest,
    });
    const user = userEvent.setup();
    renderApp("/forgot-password");

    await user.type(
      await screen.findByLabelText("Email"),
      "aether@example.com",
    );
    await user.click(screen.getByRole("button", { name: "Send reset link" }));

    expect(await screen.findByText("Check your email")).toBeInTheDocument();
    expect(
      screen.getByText(/If an account matches that information/),
    ).toBeInTheDocument();
    expect(recoveryRequest).toHaveBeenCalledOnce();
  });

  it("captures a reset token from the fragment, scrubs it, and sends CSRF", async () => {
    window.history.replaceState({}, "", "/reset-password#token=reset-secret");
    document.cookie = "straylight_csrf=csrf-value; Path=/";
    const resetRequest = vi.fn(async (request: Request) => {
      expect(new URL(request.url).pathname).toBe("/api/v1/auth/reset-password");
      expect(request.url).not.toContain("reset-secret");
      expect(request.credentials).toBe("same-origin");
      expect(request.headers.get("Authorization")).toBeNull();
      expect(request.headers.get("X-CSRF-Token")).toBe("csrf-value");
      expect(await request.json()).toEqual({
        token: "reset-secret",
        password: "a long replacement password",
      });
      return { status: "complete", data: { status: "complete" } };
    });
    installApiMock({
      "POST /api/v1/auth/reset-password": resetRequest,
    });
    const user = userEvent.setup();
    const { container, queryClient } = renderApp(
      "/reset-password",
      undefined,
      { strict: true },
    );
    queryClient.setQueryData(["previous-user", "private"], { secret: true });

    await waitFor(() => expect(window.location.hash).toBe(""));
    expect(container).not.toHaveTextContent("reset-secret");
    await user.type(
      screen.getByLabelText("New password"),
      "a long replacement password",
    );
    await user.type(
      screen.getByLabelText("Confirm new password"),
      "a long replacement password",
    );
    await user.click(screen.getByRole("button", { name: "Update password" }));

    expect(
      await screen.findByRole("heading", { name: "Password updated" }),
    ).toBeInTheDocument();
    expect(queryClient.getQueryData(["previous-user", "private"])).toBeUndefined();
    expect(queryClient.getMutationCache().getAll()).toHaveLength(0);
    expect(resetRequest).toHaveBeenCalledOnce();
  });

  it("does not render a reset form without a fragment token", async () => {
    installApiMock();
    renderApp("/reset-password");

    expect(
      await screen.findByRole("heading", { name: "Reset link unavailable" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Update password" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: "Request a new link" }),
    ).toBeInTheDocument();
  });

  it("matches the server's normalized Unicode character limits", async () => {
    window.history.replaceState({}, "", "/reset-password#token=reset-secret");
    const resetRequest = vi.fn();
    installApiMock({ "POST /api/v1/auth/reset-password": resetRequest });
    const user = userEvent.setup();
    renderApp("/reset-password");

    const password = await screen.findByLabelText("New password");
    const confirmation = screen.getByLabelText("Confirm new password");
    const tooFewScalars = "🪐".repeat(8);
    fireEvent.change(password, { target: { value: tooFewScalars } });
    fireEvent.change(confirmation, { target: { value: tooFewScalars } });
    await user.click(screen.getByRole("button", { name: "Update password" }));
    expect(screen.getByRole("alert")).toHaveTextContent("at least 15 characters");

    const tooManyScalars = "界".repeat(1025);
    fireEvent.change(password, { target: { value: tooManyScalars } });
    fireEvent.change(confirmation, { target: { value: tooManyScalars } });
    await user.click(screen.getByRole("button", { name: "Update password" }));
    expect(screen.getByRole("alert")).toHaveTextContent("no more than 1024 characters");
    expect(resetRequest).not.toHaveBeenCalled();
  });

  it("re-arms a 30-day session across the browser timer ceiling", () => {
    const dayMs = 24 * 60 * 60 * 1000;
    let now = 0;
    const invalidated = vi.fn();
    const scheduled: Array<{
      callback: () => void;
      delay: number;
      id: number;
    }> = [];
    const schedule = vi.fn((callback: () => void, delay: number) => {
      const id = scheduled.length + 1;
      scheduled.push({ callback, delay, id });
      return id;
    });
    const clear = vi.fn();

    const dispose = scheduleSessionExpiry(
      30 * dayMs,
      invalidated,
      () => now,
      schedule,
      clear,
    );
    expect(scheduled[0]?.delay).toBe(2_147_000_000);

    now = 25 * dayMs;
    scheduled[0]?.callback();
    expect(invalidated).not.toHaveBeenCalled();
    expect(scheduled[1]?.delay).toBe(5 * dayMs);

    now = 30 * dayMs + 1;
    scheduled[1]?.callback();
    expect(invalidated).toHaveBeenCalledOnce();
    dispose();
    expect(clear).toHaveBeenCalledWith(2);
  });

  it("clears private caches and returns to sign in at session expiry", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2030-01-01T00:00:00Z"));
    const expiresAt = new Date(Date.now() + 10_000).toISOString();
    installApiMock({
      "GET /api/v1/auth/session": {
        status: "complete",
        data: { ...sessionEnvelope.data, expires_at: expiresAt },
      },
    });
    const rendered = renderApp("/work");
    try {
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1_000);
      });
      expect(screen.getByRole("heading", { name: "Workspace" })).toBeInTheDocument();
      rendered.queryClient.setQueryData(["private", "cached"], { secret: true });

      await act(async () => {
        await vi.advanceTimersByTimeAsync(9_001);
      });
      expect(screen.getByRole("heading", { name: "Sign in" })).toBeInTheDocument();
      expect(rendered.queryClient.getQueryData(["private", "cached"])).toBeUndefined();
    } finally {
      rendered.unmount();
      vi.useRealTimers();
    }
  });

  it("invalidates the visible session when any protected request returns 401", async () => {
    installApiMock();
    const { queryClient } = renderApp("/work");
    expect(await screen.findByRole("heading", { name: "Workspace" })).toBeInTheDocument();
    queryClient.setQueryData(["private", "cached"], { secret: true });
    installApiMock({ "GET /api/v1/status": unauthenticated });

    await expect(createApiClient().status()).rejects.toMatchObject({ status: 401 });

    expect(await screen.findByRole("heading", { name: "Sign in" })).toBeInTheDocument();
    expect(queryClient.getQueryData(["private", "cached"])).toBeUndefined();
  });

  it("signs out through the session endpoint with CSRF protection", async () => {
    document.cookie = "straylight_csrf=logout-csrf; Path=/";
    const logoutRequest = vi.fn((request: Request) => {
      expect(request.credentials).toBe("same-origin");
      expect(request.headers.get("X-CSRF-Token")).toBe("logout-csrf");
      expect(request.headers.get("Authorization")).toBeNull();
      return { status: "complete", data: { status: "complete" } };
    });
    installApiMock({
      "POST /api/v1/auth/logout": logoutRequest,
    });
    const user = userEvent.setup();
    renderApp("/work");

    await user.click(
      await screen.findByRole("button", { name: "User menu for Aether" }),
    );
    await user.click(screen.getByRole("menuitem", { name: "Sign out" }));

    await waitFor(() => expect(logoutRequest).toHaveBeenCalledOnce());
    expect(
      await screen.findByRole("heading", { name: "Sign in" }),
    ).toBeInTheDocument();
  });
});
