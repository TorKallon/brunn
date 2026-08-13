import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate, useRouterState } from "@tanstack/react-router";
import { RefreshCw } from "lucide-react";
import { type PropsWithChildren, useCallback, useEffect, useRef } from "react";
import { ApiError, SESSION_INVALIDATED_EVENT } from "../lib/api";
import { AUTH_SESSION_QUERY_KEY, useApi } from "../lib/auth";
import { CurrentProvider } from "../lib/current";
import { AppShell } from "./AppShell";
import { ErrorState, LoadingState } from "./StateViews";

const MAX_SESSION_LIFETIME_MS = 30 * 24 * 60 * 60 * 1000;
const MAX_TIMER_DELAY_MS = 2_147_000_000;

type ScheduleTimer = (callback: () => void, delay: number) => number;
type ClearTimer = (timer: number) => void;

export function scheduleSessionExpiry(
  deadline: number,
  invalidateSession: () => void,
  now: () => number = () => Date.now(),
  schedule: ScheduleTimer = (callback, delay) => window.setTimeout(callback, delay),
  clear: ClearTimer = (timer) => window.clearTimeout(timer),
): () => void {
  let timer: number | undefined;
  const scheduleNext = () => {
    const remaining = deadline - now();
    if (remaining <= 0) {
      invalidateSession();
      return;
    }
    timer = schedule(scheduleNext, Math.min(remaining, MAX_TIMER_DELAY_MS));
  };
  scheduleNext();
  return () => {
    if (timer !== undefined) clear(timer);
  };
}

export function AuthBoundary({ children }: PropsWithChildren) {
  const api = useApi();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const currentHref = useRouterState({ select: (state) => state.location.href });
  const redirectHref = useRef(currentHref);
  const redirecting = useRef(false);
  redirectHref.current = currentHref;
  const sessionQuery = useQuery({
    queryKey: AUTH_SESSION_QUERY_KEY,
    queryFn: () => api.authSession(),
    retry: retryUnlessUnauthenticated,
    staleTime: 30_000,
  });
  const meQuery = useQuery({
    queryKey: ["me"],
    queryFn: () => api.me(),
    enabled: sessionQuery.isSuccess,
    retry: retryUnlessUnauthenticated,
  });
  const sessionUnauthenticated = sessionQuery.isError && isUnauthenticated(sessionQuery.error);
  const meUnauthenticated = meQuery.isError && isUnauthenticated(meQuery.error);
  const invalidateSession = useCallback(() => {
    if (redirecting.current) return;
    redirecting.current = true;
    queryClient.clear();
    void navigate({
      to: "/login",
      search: { redirect: redirectHref.current },
      replace: true,
    });
  }, [navigate, queryClient]);

  useEffect(() => {
    if (sessionUnauthenticated || meUnauthenticated) {
      invalidateSession();
    }
  }, [invalidateSession, meUnauthenticated, sessionUnauthenticated]);

  useEffect(() => {
    window.addEventListener(SESSION_INVALIDATED_EVENT, invalidateSession);
    return () => window.removeEventListener(SESSION_INVALIDATED_EVENT, invalidateSession);
  }, [invalidateSession]);

  useEffect(() => {
    const expiresAt = sessionQuery.data?.data.expires_at;
    if (!expiresAt) return;
    const now = Date.now();
    const parsedExpiry = Date.parse(expiresAt);
    const expiryDelay = parsedExpiry - now;
    if (!Number.isFinite(expiryDelay) || expiryDelay <= 0) {
      invalidateSession();
      return;
    }
    const deadline = Math.min(parsedExpiry, now + MAX_SESSION_LIFETIME_MS);
    return scheduleSessionExpiry(deadline, invalidateSession);
  }, [invalidateSession, sessionQuery.data?.data.expires_at]);

  if (sessionQuery.isPending) {
    return <ConnectionState label="Checking your Straylight session" />;
  }
  if (sessionQuery.isError) {
    if (sessionUnauthenticated) {
      return <ConnectionState label="Returning to sign in" />;
    }
    return (
      <main className="full-state">
        <ErrorState
          error={sessionQuery.error}
          retry={() => void sessionQuery.refetch()}
          title="Connection failed"
        />
      </main>
    );
  }
  if (meQuery.isPending) {
    return <ConnectionState label="Opening your Straylight workspace" />;
  }
  if (meQuery.isError) {
    if (meUnauthenticated) {
      return <ConnectionState label="Returning to sign in" />;
    }
    return (
      <main className="full-state">
        <ErrorState error={meQuery.error} retry={() => void meQuery.refetch()} title="Connection failed" />
        <button className="button secondary" type="button" onClick={() => void sessionQuery.refetch()}>
          <RefreshCw size={16} aria-hidden="true" />
          Check session again
        </button>
      </main>
    );
  }

  return (
    <CurrentProvider value={meQuery.data}>
      <AppShell>{children}</AppShell>
    </CurrentProvider>
  );
}

function ConnectionState({ label }: { label: string }) {
  return (
    <main className="full-state">
      <LoadingState label={label} />
    </main>
  );
}

function isUnauthenticated(error: unknown): boolean {
  return error instanceof ApiError && error.status === 401;
}

function retryUnlessUnauthenticated(count: number, error: unknown): boolean {
  return !isUnauthenticated(error) && count < 2;
}
