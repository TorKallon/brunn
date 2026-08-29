import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { MoonStar } from "lucide-react";
import { useEffect } from "react";
import { Section } from "./Page";
import { ErrorState, LoadingState, StatusBadge } from "./StateViews";
import { useApi } from "../lib/auth";
import { useCapability } from "../lib/current";
import { formatDate } from "../lib/format";

interface ControlView {
  enabled?: boolean;
  mode?: string;
  advance_after?: string;
  reason?: string;
}

interface RuntimeView {
  account?: string;
  plan?: string;
  connected_at?: string;
  verified_at?: string;
  codex_version?: string;
  last_attempt_date?: string;
  last_attempt_result?: string;
  last_attempt_detail?: string;
  last_run_date?: string;
}

interface ConnectView {
  state?: string;
  url?: string;
  code?: string;
  account?: string;
  plan?: string;
  detail?: string;
}

interface DreamingStatusData {
  control?: ControlView;
  dreamer?: {
    unavailable?: boolean;
    connect?: ConnectView;
    runtime?: RuntimeView;
  };
}

export function DreamingSettings() {
  const api = useApi();
  const queryClient = useQueryClient();
  const isOwner = useCapability("credential:manage");
  const statusQuery = useQuery({
    queryKey: ["dreaming-status"],
    queryFn: () => api.dreamingStatus(),
    enabled: isOwner,
    refetchInterval: 60_000,
  });

  const refresh = () =>
    queryClient.invalidateQueries({ queryKey: ["dreaming-status"] });
  const connectStart = useMutation({
    mutationFn: () => api.dreamingConnectStart(),
    onSuccess: refresh,
  });
  const disconnect = useMutation({
    mutationFn: () => api.dreamingDisconnect(),
    onSuccess: refresh,
  });
  const pause = useMutation({
    mutationFn: () => api.dreamingPause(),
    onSuccess: refresh,
  });
  const resume = useMutation({
    mutationFn: () => api.dreamingResume(),
    onSuccess: refresh,
  });

  const data = (statusQuery.data?.data ?? {}) as DreamingStatusData;
  const control = data.control ?? {};
  const dreamer = data.dreamer ?? {};
  const runtime = dreamer.runtime ?? {};
  const connect = dreamer.connect ?? {};
  const connectPending = connect.state === "pending";

  // While a device-code login is pending, poll connect/wait so completion is
  // observed and finalized (vault capture + live verification).
  useEffect(() => {
    if (!connectPending) return;
    const timer = setInterval(() => {
      api
        .dreamingConnectWait()
        .then((envelope) => {
          const state = (envelope.data as ConnectView | undefined)?.state;
          if (state && state !== "pending") refresh();
        })
        .catch(() => {});
    }, 3_000);
    return () => clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connectPending]);

  if (!isOwner) return null;
  if (statusQuery.isPending) {
    return (
      <Section title="Dreaming">
        <LoadingState label="Loading dreaming status" />
      </Section>
    );
  }
  if (statusQuery.isError) {
    return (
      <Section title="Dreaming">
        <ErrorState error={statusQuery.error} title="Dreaming status failed" />
      </Section>
    );
  }

  const connected =
    connect.state === "connected" || Boolean(runtime.connected_at);
  const connectionStatus = connectPending
    ? "pending"
    : connect.state === "verifying"
      ? "in_progress"
      : connected
        ? "active"
        : dreamer.unavailable
          ? "unavailable"
          : "disconnected";
  const paused = control.enabled === false;

  return (
    <Section
      title="Dreaming"
      meta="Nightly memory consolidation on the dedicated Codex account"
    >
      <div className="dreaming-status-card">
        <p>
          <StatusBadge status={connectionStatus} />{" "}
          <StatusBadge status={paused ? "paused" : (control.mode ?? "report-only")} />
        </p>
        <dl className="dreaming-facts">
          <div>
            <dt>Account</dt>
            <dd>
              {runtime.account ?? "Not connected"}
              {runtime.plan ? ` (${runtime.plan})` : ""}
            </dd>
          </div>
          <div>
            <dt>Token health</dt>
            <dd>
              {runtime.verified_at
                ? `Verified ${formatDate(runtime.verified_at)}`
                : "Never verified"}
            </dd>
          </div>
          <div>
            <dt>Last run</dt>
            <dd>
              {runtime.last_attempt_date
                ? `${runtime.last_attempt_date} — ${runtime.last_attempt_result ?? "unknown"}`
                : "No runs yet"}
            </dd>
          </div>
          {control.enabled && control.advance_after ? (
            <div>
              <dt>Full mode after</dt>
              <dd>{control.advance_after}</dd>
            </div>
          ) : null}
        </dl>
      </div>

      {connectPending && connect.url ? (
        <p className="dreaming-device-code" role="status">
          To connect, visit <a href={connect.url}>{connect.url}</a> and enter{" "}
          <strong>{connect.code}</strong>. Waiting for the login to complete…
        </p>
      ) : null}
      {connect.state === "failed" ? (
        <p className="dreaming-connect-failed" role="alert">
          Connect failed: {connect.detail ?? "unknown error"}
        </p>
      ) : null}

      <div className="dreaming-actions">
        {connected ? (
          <button
            className="button"
            type="button"
            onClick={() => disconnect.mutate()}
            disabled={disconnect.isPending}
          >
            Disconnect
          </button>
        ) : (
          <button
            className="button primary"
            type="button"
            onClick={() => connectStart.mutate()}
            disabled={connectStart.isPending || connectPending}
          >
            <MoonStar size={15} aria-hidden="true" /> Connect
          </button>
        )}
        {paused ? (
          <button
            className="button"
            type="button"
            onClick={() => resume.mutate()}
            disabled={resume.isPending}
          >
            Resume
          </button>
        ) : (
          <button
            className="button"
            type="button"
            onClick={() => pause.mutate()}
            disabled={pause.isPending}
          >
            Pause
          </button>
        )}
      </div>
      {connectStart.isError ? (
        <ErrorState error={connectStart.error} title="Connect failed" />
      ) : null}
      {pause.isError ? (
        <ErrorState error={pause.error} title="Pause failed" />
      ) : null}
      {resume.isError ? (
        <ErrorState error={resume.error} title="Resume failed" />
      ) : null}
      {disconnect.isError ? (
        <ErrorState error={disconnect.error} title="Disconnect failed" />
      ) : null}
    </Section>
  );
}
