import type { ReactNode } from "react";
import {
  AlertCircle,
  CheckCircle2,
  CircleDashed,
  Inbox,
  LoaderCircle,
  RefreshCw,
  TriangleAlert,
} from "lucide-react";
import { ApiError } from "../lib/api";
import { humanize } from "../lib/format";

export type Tone = "neutral" | "success" | "warning" | "danger" | "info";

export function toneForStatus(status?: string): Tone {
  if (!status) return "neutral";
  const normalized = status.toLowerCase();
  if (
    [
      "complete",
      "done",
      "success",
      "healthy",
      "ready",
      "active",
      "passed",
      "supported",
      "committed",
    ].includes(
      normalized,
    )
  )
    return "success";
  if (
    ["failed", "error", "unavailable", "contradicted", "rejected", "revoked"].includes(
      normalized,
    )
  )
    return "danger";
  if (
    ["partial", "degraded", "stale", "warning", "pending", "needs_review", "ambiguous"].includes(
      normalized,
    )
  )
    return "warning";
  if (["running", "processing", "queued", "in_progress"].includes(normalized)) return "info";
  return "neutral";
}

export function StatusBadge({ status }: { status?: string }) {
  return <span className={`status-badge tone-${toneForStatus(status)}`}>{humanize(status)}</span>;
}

export function LoadingState({ label = "Loading" }: { label?: string }) {
  return (
    <div className="state-view" role="status">
      <LoaderCircle className="spin" size={22} aria-hidden="true" />
      <span>{label}</span>
    </div>
  );
}

export function EmptyState({
  title,
  detail,
  action,
}: {
  title: string;
  detail?: string;
  action?: ReactNode;
}) {
  return (
    <div className="state-view state-view-empty">
      <Inbox size={24} aria-hidden="true" />
      <strong>{title}</strong>
      {detail ? <span>{detail}</span> : null}
      {action}
    </div>
  );
}

export function ErrorState({
  error,
  retry,
  title = "Unable to load",
}: {
  error: unknown;
  retry?: () => void;
  title?: string;
}) {
  const apiError = error instanceof ApiError ? error : null;
  const detail =
    error instanceof Error ? error.message : "The request could not be completed.";
  return (
    <div className="state-view state-view-error" role="alert">
      <AlertCircle size={24} aria-hidden="true" />
      <strong>{title}</strong>
      <span>{detail}</span>
      {apiError?.code ? <code>{apiError.code}</code> : null}
      {retry ? (
        <button className="button secondary" type="button" onClick={retry}>
          <RefreshCw size={16} aria-hidden="true" />
          Retry
        </button>
      ) : null}
    </div>
  );
}

export function ProtocolNotice({
  status,
  gaps = 0,
  conflicts = 0,
}: {
  status?: string;
  gaps?: number;
  conflicts?: number;
}) {
  if (!status || (status === "complete" && gaps === 0 && conflicts === 0)) return null;
  const Icon = status === "failed" || conflicts > 0 ? TriangleAlert : CircleDashed;
  return (
    <div className={`protocol-notice tone-${toneForStatus(status)}`} role="status">
      <Icon size={17} aria-hidden="true" />
      <span>
        {humanize(status)}
        {gaps ? `, ${gaps} gap${gaps === 1 ? "" : "s"}` : ""}
        {conflicts ? `, ${conflicts} conflict${conflicts === 1 ? "" : "s"}` : ""}
      </span>
    </div>
  );
}

export function ReadOnlyNotice() {
  return (
    <div className="readonly-notice" role="status">
      <CheckCircle2 size={17} aria-hidden="true" />
      Read-only credential active
    </div>
  );
}
