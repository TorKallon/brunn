import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import {
  ArrowRight,
  Database,
  FileText,
  HardDrive,
  KeyRound,
  Search,
  ShieldCheck,
  Sunrise,
} from "lucide-react";
import type { CSSProperties, ReactNode } from "react";
import { MarkdownView } from "../components/MarkdownView";
import { Page } from "../components/Page";
import {
  ErrorState,
  LoadingState,
  StatusBadge,
} from "../components/StateViews";
import { useApi } from "../lib/auth";
import { useCurrent } from "../lib/current";
import {
  formatBytes,
  formatDate,
  formatRelative,
  humanize,
  shortId,
} from "../lib/format";
import type {
  BriefingListRow,
  DashboardActivityPoint,
  DashboardAccessClient,
} from "../lib/types";

const numberFormat = new Intl.NumberFormat();

export function DashboardPage() {
  const api = useApi();
  const current = useCurrent();
  const timezone = resolvedTimezone();
  const today = localDateKey(timezone);
  const dashboardQuery = useQuery({
    queryKey: ["workspace-dashboard", timezone],
    queryFn: () => api.workspaceDashboard(timezone),
    refetchInterval: 60_000,
  });
  const briefingQuery = useQuery({
    queryKey: ["briefings", "dashboard"],
    queryFn: () => api.briefingsList(7),
  });

  const editions = briefingQuery.data?.data.editions ?? [];
  const todayEdition = editions.find((edition) => edition.date === today);
  const featuredEdition = todayEdition ?? editions[0];
  const dashboard = dashboardQuery.data?.data;
  const activeClients = dashboard?.access.filter(
    (client) => client.status === "active",
  );

  return (
    <Page>
      <section className="dashboard-hero" aria-labelledby="dashboard-heading">
        <div className="dashboard-hero-copy">
          <span className="dashboard-kicker">{formatDayLabel(today)}</span>
          <h1 id="dashboard-heading">
            {current.data.user.display_name}&rsquo;s Straylight
          </h1>
          <p>
            A quiet view of what your memory is holding, how it is being used,
            and which clients can reach it.
          </p>
          <div className="dashboard-hero-actions">
            {featuredEdition ? (
              <Link
                className="button primary dashboard-primary-action"
                to="/briefings/$date"
                params={{ date: featuredEdition.date }}
                search={{ edition: featuredEdition.edition }}
              >
                <Sunrise size={17} aria-hidden="true" />
                {todayEdition ? "Read today’s briefing" : "Read latest briefing"}
                <ArrowRight size={16} aria-hidden="true" />
              </Link>
            ) : (
              <Link className="button primary" to="/briefings">
                <Sunrise size={17} aria-hidden="true" />
                Open briefings
              </Link>
            )}
            <Link className="button secondary" to="/briefings">
              All briefings
            </Link>
            <Link className="button dashboard-search-link" to="/explore">
              <Search size={16} aria-hidden="true" />
              Search memory
            </Link>
          </div>
        </div>
        <BriefingSpotlight
          edition={featuredEdition}
          loading={briefingQuery.isPending}
          error={briefingQuery.isError}
          isToday={Boolean(todayEdition)}
        />
      </section>

      {dashboardQuery.isPending ? (
        <LoadingState label="Loading workspace overview" />
      ) : null}
      {dashboardQuery.isError ? (
        <ErrorState
          error={dashboardQuery.error}
          retry={() => void dashboardQuery.refetch()}
          title="Unable to load the workspace overview"
        />
      ) : null}

      {dashboard ? (
        <>
          <section className="dashboard-section" aria-labelledby="storage-heading">
            <div className="dashboard-section-heading">
              <div>
                <span className="dashboard-eyebrow">Storage</span>
                <h2 id="storage-heading">What Straylight is holding</h2>
              </div>
              <span>Generation {numberFormat.format(dashboard.workspace_generation)}</span>
            </div>
            <div className="storage-grid">
              <StorageCard
                icon={<FileText size={19} aria-hidden="true" />}
                label="Text artifacts"
                count={dashboard.storage.text.count}
                size={dashboard.storage.text.size_bytes}
                detail="Current Markdown and text entries"
              />
              <StorageCard
                icon={<HardDrive size={19} aria-hidden="true" />}
                label="Referenced binaries"
                count={dashboard.storage.binary.count}
                size={dashboard.storage.binary.size_bytes}
                detail="Current S3-backed entries"
              />
            </div>
          </section>

          <section className="dashboard-section" aria-labelledby="activity-heading">
            <div className="dashboard-section-heading">
              <div>
                <span className="dashboard-eyebrow">Activity</span>
                <h2 id="activity-heading">A rough pulse of daily usage</h2>
              </div>
              <span>{dashboard.timezone}</span>
            </div>
            <div className="today-metrics">
              <TodayMetric
                label="Reads today"
                value={dashboard.today.read_operations}
                detail={`${formatBytes(dashboard.today.read_bytes)} returned`}
                tone="read"
              />
              <TodayMetric
                label="Writes today"
                value={dashboard.today.write_operations}
                detail={`${formatBytes(dashboard.today.write_bytes)} committed`}
                tone="write"
              />
              <TodayMetric
                label="Active access"
                value={activeClients?.length ?? 0}
                detail={`${dashboard.access.length} credential${dashboard.access.length === 1 ? "" : "s"} listed`}
                tone="neutral"
              />
            </div>
            <div className="chart-grid">
              <UsageChart
                title="Operations"
                detail="Successful content reads and committed writes"
                points={dashboard.activity}
                readValue={(point) => point.read_operations}
                writeValue={(point) => point.write_operations}
                formatValue={(value) => numberFormat.format(value)}
              />
              <UsageChart
                title="Data moved"
                detail="Response bytes returned and artifact bytes committed"
                points={dashboard.activity}
                readValue={(point) => point.read_bytes}
                writeValue={(point) => point.write_bytes}
                formatValue={formatBytes}
              />
            </div>
            <p className="dashboard-coverage-note">
              Activity is product telemetry for tracked workspace operations;
              dashboard and control-page refreshes are excluded.
              {dashboard.activity_tracking_started_at
                ? ` Tracking began ${formatDate(dashboard.activity_tracking_started_at)}.`
                : ""}
            </p>
          </section>

          <section className="dashboard-section" aria-labelledby="access-heading">
            <div className="dashboard-section-heading">
              <div>
                <span className="dashboard-eyebrow">Access</span>
                <h2 id="access-heading">API credentials</h2>
              </div>
              <Link className="button secondary" to="/control">
                Manage access
                <ArrowRight size={15} aria-hidden="true" />
              </Link>
            </div>
            <div className="access-list">
              {dashboard.access.map((client) => (
                <AccessRow
                  key={client.id}
                  client={client}
                  current={client.id === current.data.credential_id}
                />
              ))}
              {!dashboard.access.length ? (
                <div className="dashboard-empty-row">
                  <ShieldCheck size={20} aria-hidden="true" />
                  <span>No API credentials are visible to this account.</span>
                </div>
              ) : null}
            </div>
          </section>
        </>
      ) : null}
    </Page>
  );
}

function BriefingSpotlight({
  edition,
  loading,
  error,
  isToday,
}: {
  edition?: BriefingListRow;
  loading: boolean;
  error: boolean;
  isToday: boolean;
}) {
  if (loading) {
    return (
      <aside className="briefing-spotlight briefing-spotlight-loading">
        <Sunrise size={20} aria-hidden="true" />
        <span>Finding the latest briefing…</span>
      </aside>
    );
  }
  if (error) {
    return (
      <aside className="briefing-spotlight briefing-spotlight-loading">
        <Sunrise size={20} aria-hidden="true" />
        <strong>Briefings are temporarily unavailable</strong>
        <span>The workspace overview will continue loading independently.</span>
      </aside>
    );
  }
  if (!edition) {
    return (
      <aside className="briefing-spotlight">
        <span className="dashboard-eyebrow">Briefings</span>
        <h2>Your daily thread will appear here</h2>
        <p>Published editions are kept together in Briefings.</p>
      </aside>
    );
  }
  return (
    <aside className="briefing-spotlight">
      <div className="briefing-spotlight-heading">
        <span className="dashboard-eyebrow">
          {isToday ? "Today’s briefing" : "Latest briefing"}
        </span>
        <span>{edition.date}</span>
      </div>
      <h2>{humanize(edition.edition)}</h2>
      {edition.summary_md[0] ? (
        <MarkdownView
          className="briefing-spotlight-summary"
          markdown={edition.summary_md[0]}
        />
      ) : (
        <p>{edition.item_count} source-backed updates are ready.</p>
      )}
      <div className="briefing-spotlight-meta">
        <span>{edition.item_count} items</span>
        <span>{edition.section_titles.length} sections</span>
        <span>{formatRelative(edition.generated_at)}</span>
      </div>
    </aside>
  );
}

function StorageCard({
  icon,
  label,
  count,
  size,
  detail,
}: {
  icon: ReactNode;
  label: string;
  count: number;
  size: number;
  detail: string;
}) {
  return (
    <article className="storage-card">
      <div className="storage-card-icon">{icon}</div>
      <div className="storage-card-copy">
        <span>{label}</span>
        <strong>{numberFormat.format(count)}</strong>
        <small>{detail}</small>
      </div>
      <div className="storage-card-size">
        <span>Logical size</span>
        <strong>{formatBytes(size)}</strong>
      </div>
    </article>
  );
}

function TodayMetric({
  label,
  value,
  detail,
  tone,
}: {
  label: string;
  value: number;
  detail: string;
  tone: "read" | "write" | "neutral";
}) {
  return (
    <article className={`today-metric tone-${tone}`}>
      <span>{label}</span>
      <strong>{numberFormat.format(value)}</strong>
      <small>{detail}</small>
    </article>
  );
}

function UsageChart({
  title,
  detail,
  points,
  readValue,
  writeValue,
  formatValue,
}: {
  title: string;
  detail: string;
  points: DashboardActivityPoint[];
  readValue: (point: DashboardActivityPoint) => number;
  writeValue: (point: DashboardActivityPoint) => number;
  formatValue: (value: number) => string;
}) {
  const ceiling = Math.max(
    1,
    ...points.flatMap((point) => [readValue(point), writeValue(point)]),
  );
  return (
    <article className="usage-chart">
      <header>
        <div>
          <h3>{title}</h3>
          <p>{detail}</p>
        </div>
        <div className="chart-legend" aria-hidden="true">
          <span className="legend-read">Reads</span>
          <span className="legend-write">Writes</span>
        </div>
      </header>
      <div
        className="bar-chart"
        aria-hidden="true"
      >
        {points.map((point) => {
          const reads = readValue(point);
          const writes = writeValue(point);
          return (
            <div className="bar-chart-column" key={point.date}>
              <div className="bar-chart-values" aria-hidden="true">
                <span
                  className="bar-read"
                  style={barStyle(reads, ceiling)}
                  title={`${formatValue(reads)} read`}
                />
                <span
                  className="bar-write"
                  style={barStyle(writes, ceiling)}
                  title={`${formatValue(writes)} written`}
                />
              </div>
              <span aria-hidden="true">{shortDay(point.date)}</span>
            </div>
          );
        })}
      </div>
      <table className="sr-only">
        <caption>{title} over the last {points.length} days</caption>
        <thead>
          <tr>
            <th>Date</th>
            <th>Reads</th>
            <th>Writes</th>
          </tr>
        </thead>
        <tbody>
          {points.map((point) => (
            <tr key={point.date}>
              <th scope="row">{point.date}</th>
              <td>{formatValue(readValue(point))}</td>
              <td>{formatValue(writeValue(point))}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </article>
  );
}

function AccessRow({
  client,
  current,
}: {
  client: DashboardAccessClient;
  current: boolean;
}) {
  const operationsToday =
    client.read_operations_today + client.write_operations_today;
  return (
    <article className={`access-row ${client.status !== "active" ? "is-revoked" : ""}`}>
      <div className="access-row-icon">
        {client.kind === "web_session" ? (
          <Database size={18} aria-hidden="true" />
        ) : (
          <KeyRound size={18} aria-hidden="true" />
        )}
      </div>
      <div className="access-row-primary">
        <div>
          <strong>{client.name}</strong>
          {current ? <span className="current-client">This client</span> : null}
        </div>
        <code>{shortId(client.id, 22)}</code>
      </div>
      <div className="access-row-activity">
        <span>Last activity</span>
        <strong>
          {client.last_used_at
            ? formatRelative(client.last_used_at)
            : "No usage recorded"}
        </strong>
        <small>
          {operationsToday
            ? `${numberFormat.format(operationsToday)} operation${operationsToday === 1 ? "" : "s"} today`
            : "No tracked activity today"}
          {client.last_operation
            ? ` · Last operation: ${humanize(client.last_operation)}`
            : ""}
        </small>
      </div>
      <div className="access-row-scope">
        <span>Access</span>
        <strong>{humanize(client.access)}</strong>
        <small>
          {client.scope_ids.length
            ? client.scope_ids.map((scope) => shortId(scope, 16)).join(", ")
            : "Root scope"}
          {client.capabilities?.length
            ? " · " + client.capabilities.length + " capabilities"
            : ""}
        </small>
      </div>
      <StatusBadge status={client.status} />
    </article>
  );
}

function resolvedTimezone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
}

function localDateKey(timezone: string): string {
  try {
    const parts = new Intl.DateTimeFormat("en-CA", {
      timeZone: timezone,
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
    }).formatToParts(new Date());
    const value = Object.fromEntries(parts.map((part) => [part.type, part.value]));
    return `${value.year}-${value.month}-${value.day}`;
  } catch {
    return new Date().toISOString().slice(0, 10);
  }
}

function formatDayLabel(date: string): string {
  const parsed = new Date(`${date}T12:00:00`);
  if (Number.isNaN(parsed.valueOf())) return date;
  return new Intl.DateTimeFormat(undefined, {
    weekday: "long",
    month: "long",
    day: "numeric",
  }).format(parsed);
}

function shortDay(date: string): string {
  const parsed = new Date(`${date}T12:00:00`);
  if (Number.isNaN(parsed.valueOf())) return date.slice(5);
  return new Intl.DateTimeFormat(undefined, { weekday: "short" }).format(parsed);
}

function barStyle(value: number, ceiling: number): CSSProperties {
  const percent = value <= 0 ? 0 : Math.max(4, (value / ceiling) * 100);
  return { height: `${percent}%` };
}
