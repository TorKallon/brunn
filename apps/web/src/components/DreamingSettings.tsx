import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ArrowUpRight, Check, Copy, MoonStar } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Section } from "./Page";
import { ErrorState, LoadingState, StatusBadge } from "./StateViews";
import { useApi } from "../lib/auth";
import { useCapability } from "../lib/current";
import { formatDate } from "../lib/format";

interface ControlView {
  enabled?: boolean;
  mode?: string;
  auto_apply_after_hours?: number | null;
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
  const authorizationTitle = useRef<HTMLHeadingElement>(null);
  const [copyStatus, setCopyStatus] = useState<"idle" | "copied" | "failed">("idle");
  const [waitError, setWaitError] = useState(false);
  const statusQuery = useQuery({
    queryKey: ["dreaming-status"],
    queryFn: () => api.dreamingStatus(),
    enabled: isOwner,
    refetchInterval: 60_000,
  });

  const refresh = useCallback(() =>
    queryClient.invalidateQueries({ queryKey: ["dreaming-status"] }), [queryClient]);
  const updateConnect = useCallback((connect: ConnectView) => {
    queryClient.setQueryData<{ data: DreamingStatusData }>(["dreaming-status"], (old) => old ? ({
      ...old,
      data: { ...old.data, dreamer: { ...old.data.dreamer, connect } },
    }) : old);
  }, [queryClient]);
  const connectStart = useMutation({
    mutationFn: () => api.dreamingConnectStart(),
    onSuccess: (envelope) => {
      // Show the returned authorization step immediately, without waiting for
      // another status request before revealing the link and code.
      const connect = envelope.data as ConnectView;
      updateConnect(connect);
      setCopyStatus("idle");
      setWaitError(false);
      if (connect.state !== "pending" && connect.state !== "verifying") void refresh();
    },
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
  const connectVerifying = connect.state === "verifying";
  const awaitingConnection = connectPending || connectVerifying;

  useEffect(() => {
    if (connectPending && connectStart.isSuccess) authorizationTitle.current?.focus();
  }, [connectPending, connectStart.isSuccess]);

  // While a device-code login is pending, poll connect/wait so completion is
  // observed and finalized. Wait for each request before scheduling another.
  useEffect(() => {
    if (!awaitingConnection) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const envelope = await api.dreamingConnectWait();
        if (cancelled) return;
        const next = envelope.data as ConnectView;
        updateConnect(next);
        setWaitError(false);
        if (next.state !== "pending" && next.state !== "verifying") {
          void refresh();
          return;
        }
      } catch {
        if (cancelled) return;
        setWaitError(true);
      }
      timer = setTimeout(poll, 3_000);
    };
    timer = setTimeout(poll, 3_000);
    return () => { cancelled = true; clearTimeout(timer); };
  }, [api, awaitingConnection, refresh, updateConnect]);

  const copyCode = async () => {
    if (!connect.code) return;
    try {
      await navigator.clipboard.writeText(connect.code);
      setCopyStatus("copied");
    } catch {
      setCopyStatus("failed");
    }
  };

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
      actions={<Link className="button secondary" to="/dreams">Open Review</Link>}
    >
      {connectPending && connect.url && connect.code ? (
        <div className="dreaming-authorization" role="region" aria-labelledby="dreaming-authorization-title">
          <h3 id="dreaming-authorization-title" tabIndex={-1} ref={authorizationTitle}>Finish connecting Dreamer</h3>
          <p>Authorize your dedicated ChatGPT account in a new tab.</p>
          <ol className="dreaming-authorization-steps">
            <li>
              <strong>Copy your code</strong>
              <div className="dreaming-code-row">
                <code className="dreaming-code">{connect.code}</code>
                <button className="button secondary" type="button" onClick={() => void copyCode()}>
                  {copyStatus === "copied" ? <Check size={16} aria-hidden="true" /> : <Copy size={16} aria-hidden="true" />}
                  {copyStatus === "copied" ? "Copied" : "Copy code"}
                </button>
              </div>
              <span className="dreaming-copy-feedback" role="status">
                {copyStatus === "copied" ? "Code copied." : copyStatus === "failed" ? "Copy wasn’t available. Select and copy the code above." : ""}
              </span>
            </li>
            <li>
              <strong>Open ChatGPT and enter the code</strong>
              <a className="button primary dreaming-authorize-button" href={connect.url} target="_blank" rel="noopener noreferrer">
                Authorize in ChatGPT <ArrowUpRight size={17} aria-hidden="true" />
              </a>
            </li>
            <li><strong>Return here</strong><p>Keep this page open. We’ll confirm when your account is connected.</p></li>
          </ol>
          <p className="dreaming-connection-progress" role="status">Waiting for your approval in ChatGPT.</p>
        </div>
      ) : null}
      {connectStart.isPending || connectVerifying || (connectPending && (!connect.url || !connect.code)) ? (
        <p className="dreaming-connection-progress" role="status">
          {connectVerifying ? "Checking your connection…" : "Preparing your sign-in link…"}
        </p>
      ) : null}
      {waitError && awaitingConnection ? (
        <p className="dreaming-connect-failed" role="alert">We couldn’t check your connection. Keep this page open; we’re retrying automatically.</p>
      ) : null}
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
            <dt>Connection verified</dt>
            <dd>
              {runtime.verified_at
                ? `Verified ${formatDate(runtime.verified_at)}`
                : "Never verified"}
            </dd>
          </div>
          <div>
            <dt>Last attempt</dt>
            <dd>
              {runtime.last_attempt_date
                ? `${runtime.last_attempt_date} — ${runtime.last_attempt_result ?? "unknown"}`
                : "No runs yet"}
            </dd>
          </div>
          {control.enabled && control.advance_after ? (
            <div>
              <dt>Earliest mode eligibility</dt>
              <dd>{control.advance_after}</dd>
            </div>
          ) : null}
        </dl>
      </div>

      <p className="settings-note">{control.mode === "full" && control.auto_apply_after_hours ? `Proposals apply on the first run after ${control.auto_apply_after_hours} hours unless you reject, defer, or request a correction in Review. Each revision starts a new review window.` : "Review proposals and questions in the Review inbox. Report-only holds approvals; full mode publishes approved changes."}</p>

      {connect.state === "failed" ? (
        <p className="dreaming-connect-failed" role="alert">
          Connect failed: {connect.detail ?? "unknown error"}
        </p>
      ) : null}

      <div className="dreaming-actions">
        {connected ? (
          <button
            className="button secondary"
            type="button"
            onClick={() => disconnect.mutate()}
            disabled={disconnect.isPending}
          >
            Disconnect
          </button>
        ) : !awaitingConnection && !connectStart.isPending ? (
          <button
            className="button primary"
            type="button"
            onClick={() => connectStart.mutate()}
          >
            <MoonStar size={15} aria-hidden="true" /> Connect
          </button>
        ) : null}
        {paused ? (
          <button
            className="button secondary"
            type="button"
            onClick={() => resume.mutate()}
            disabled={resume.isPending}
          >
            Resume
          </button>
        ) : (
          <button
            className="button secondary"
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
