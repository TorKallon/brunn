import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import {
  ArrowLeft,
  Bell,
  Check,
  ChevronRight,
  CircleAlert,
  ExternalLink,
  FileText,
  Inbox,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { MarkdownView } from "../components/MarkdownView";
import { DefinitionList, Page, PageHeader, Section } from "../components/Page";
import {
  EmptyState,
  ErrorState,
  LoadingState,
  StatusBadge,
} from "../components/StateViews";
import { WorkspaceEntryView } from "../components/WorkspaceEntryView";
import { useApi } from "../lib/auth";
import { formatDate, formatRelative, humanize } from "../lib/format";
import type {
  ApiEnvelope,
  NotificationDetailData,
  NotificationItem,
  NotificationTarget,
} from "../lib/types";
import "../notifications.css";

const ALERTS_PAGE_SIZE = 30;
type AlertFilter = "all" | "important" | "unread";

export function AlertsPage() {
  const api = useApi();
  const [filter, setFilter] = useState<AlertFilter>("all");
  const listQuery = useInfiniteQuery({
    queryKey: ["notifications", filter],
    queryFn: ({ pageParam }: { pageParam: string | undefined }) =>
      api.notificationsList(
        ALERTS_PAGE_SIZE,
        pageParam,
        filter === "unread" ? true : undefined,
        filter === "important" ? "important" : undefined,
      ),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (page) => page.data.next_cursor ?? undefined,
  });
  const items = listQuery.data?.pages.flatMap((page) => page.data.items) ?? [];
  const unreadCount = listQuery.data?.pages[0]?.data.unread_count ?? 0;

  return (
    <Page>
      <PageHeader
        title="Alerts"
        description="Durable notifications from Codex and Aether"
        actions={<Bell size={19} aria-hidden="true" />}
      />

      <div className="alert-filter-bar" role="group" aria-label="Alert filters">
        {(["all", "important", "unread"] as const).map((value) => (
          <button
            key={value}
            type="button"
            className={filter === value ? "active" : undefined}
            aria-pressed={filter === value}
            onClick={() => setFilter(value)}
          >
            {humanize(value)}
            {value === "unread" && unreadCount > 0 ? (
              <span>{unreadCount}</span>
            ) : null}
          </button>
        ))}
      </div>

      {listQuery.isPending ? <LoadingState label="Loading alerts" /> : null}
      {listQuery.isError ? (
        <ErrorState
          error={listQuery.error}
          retry={() => void listQuery.refetch()}
          title="Unable to load alerts"
        />
      ) : null}
      {listQuery.isSuccess && !items.length ? (
        <EmptyState
          title={filter === "unread" ? "You’re caught up" : "No alerts yet"}
          detail="Morning briefings and material updates will appear here."
        />
      ) : null}

      {items.length ? (
        <Section
          title="Inbox"
          meta={`${items.length} loaded`}
          actions={unreadCount ? <span className="alert-unread-summary">{unreadCount} unread</span> : null}
        >
          <div className="alert-list">
            {items.map((item) => <AlertCard key={item.notification_ref} item={item} />)}
          </div>
          {listQuery.hasNextPage ? (
            <div className="section-footer">
              <button
                className="button secondary"
                type="button"
                onClick={() => void listQuery.fetchNextPage()}
                disabled={listQuery.isFetchingNextPage}
              >
                {listQuery.isFetchingNextPage ? "Loading" : "Load more"}
              </button>
            </div>
          ) : null}
        </Section>
      ) : null}
    </Page>
  );
}

function AlertCard({ item }: { item: NotificationItem }) {
  const unread = !item.opened_at;
  return (
    <Link
      to="/alerts/$notificationRef"
      params={{ notificationRef: item.notification_ref }}
      className={`alert-card${unread ? " is-unread" : ""}`}
    >
      <span className="alert-signal-edge" aria-hidden="true" />
      <div className="alert-card-copy">
        <header>
          <div>
            <span className={`alert-kind alert-kind-${item.kind}`}>{humanize(item.kind)}</span>
            {item.importance === "important" ? (
              <span className="alert-importance">Important</span>
            ) : null}
          </div>
          <time dateTime={item.occurred_at}>{formatRelative(item.occurred_at)}</time>
        </header>
        <h3>{item.title}</h3>
        <p className="alert-card-body">{plainExcerpt(item.body)}</p>
        <footer>
          {unread ? <span className="alert-unread-dot">Unread</span> : <span>Opened</span>}
          {item.acknowledged_at ? <span>Acknowledged</span> : null}
        </footer>
      </div>
      <ChevronRight className="alert-card-chevron" size={18} aria-hidden="true" />
    </Link>
  );
}

export function AlertDetailPage() {
  const { notificationRef } = useParams({ from: "/authenticated/alerts/$notificationRef" });
  const api = useApi();
  const queryClient = useQueryClient();
  const openedSent = useRef(false);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const detailQuery = useQuery({
    queryKey: ["notification", notificationRef],
    queryFn: () => api.notificationGet(notificationRef),
  });
  const receiptMutation = useMutation({
    mutationFn: (kind: "opened" | "acknowledged") =>
      api.notificationReceipt(notificationRef, kind),
    onSuccess: (response) => {
      queryClient.setQueryData<ApiEnvelope<NotificationDetailData>>(
        ["notification", notificationRef],
        (current) => current ? {
          ...current,
          data: {
            notification: {
              ...current.data.notification,
              opened_at: response.data.opened_at ?? current.data.notification.opened_at,
              acknowledged_at:
                response.data.acknowledged_at ?? current.data.notification.acknowledged_at,
            },
          },
        } : current,
      );
      void queryClient.invalidateQueries({ queryKey: ["notifications"] });
    },
  });
  const notification = detailQuery.data?.data.notification;

  useEffect(() => {
    if (!notification || notification.opened_at || openedSent.current) return;
    openedSent.current = true;
    receiptMutation.mutate("opened");
  }, [notification, receiptMutation]);

  useEffect(() => {
    if (notification) headingRef.current?.focus();
  }, [notification]);

  return (
    <Page>
      <div className="alert-detail-back">
        <Link className="button secondary" to="/alerts">
          <ArrowLeft size={16} aria-hidden="true" />
          Alerts
        </Link>
      </div>
      {detailQuery.isPending ? <LoadingState label="Loading alert detail" /> : null}
      {detailQuery.isError ? (
        <ErrorState
          error={detailQuery.error}
          retry={() => void detailQuery.refetch()}
          title="Unable to load alert"
        />
      ) : null}

      {notification ? (
        <article className="alert-detail">
          <header className="alert-detail-heading">
            <div className="alert-detail-kicker">
              <span className={`alert-kind alert-kind-${notification.kind}`}>
                {humanize(notification.kind)}
              </span>
              {notification.importance === "important" ? (
                <span className="alert-importance">Important</span>
              ) : null}
              {notification.acknowledged_at ? <StatusBadge status="acknowledged" /> : null}
            </div>
            <h1 ref={headingRef} tabIndex={-1}>{notification.title}</h1>
            <p>
              <time dateTime={notification.occurred_at}>{formatDate(notification.occurred_at)}</time>
              {notification.expires_at ? ` · Expires ${formatDate(notification.expires_at)}` : ""}
            </p>
          </header>

          <section className="alert-detail-body" aria-label="Alert detail">
            <MarkdownView markdown={notification.body} />
          </section>

          <NotificationTargetAction notification={notification} />

          {notification.source ? (
            <Section title="Pinned source" actions={<FileText size={18} aria-hidden="true" />}>
              <DefinitionList
                items={[
                  { label: "Type", value: humanize(notification.source.type) },
                  { label: "Reference", value: <code>{notification.source.ref}</code> },
                  {
                    label: "Version",
                    value: notification.source.version_ref ? <code>{notification.source.version_ref}</code> : "Current",
                  },
                ]}
              />
            </Section>
          ) : null}

          <Section title="Delivery trace" meta="Provider acceptance is not device display">
            {notification.deliveries.length ? (
              <div className="alert-delivery-list">
                {notification.deliveries.map((delivery) => (
                  <div key={delivery.delivery_ref}>
                    <Inbox size={17} aria-hidden="true" />
                    <div>
                      <code>{delivery.delivery_ref}</code>
                      <span>{deliveryTimestamp(delivery)}</span>
                      {delivery.last_error_code ? <span>{delivery.last_error_code}</span> : null}
                    </div>
                    <StatusBadge status={delivery.state} />
                  </div>
                ))}
              </div>
            ) : (
              <EmptyState title="No push delivery attached" detail="The alert remains available in this inbox." />
            )}
          </Section>

          <div className="alert-detail-actions">
            <button
              className="button primary"
              type="button"
              disabled={Boolean(notification.acknowledged_at) || receiptMutation.isPending}
              onClick={() => receiptMutation.mutate("acknowledged")}
            >
              <Check size={16} aria-hidden="true" />
              {notification.acknowledged_at ? "Acknowledged" : "Acknowledge"}
            </button>
            {receiptMutation.isError ? (
              <span className="field-error" role="alert">
                The receipt could not be recorded. Try again.
              </span>
            ) : null}
          </div>
        </article>
      ) : null}
    </Page>
  );
}

function NotificationTargetAction({ notification }: { notification: NotificationItem }) {
  const target = notification.target;
  if (target.type === "notification") return null;
  if (target.type === "today") {
    return (
      <section className="alert-target-card">
        <CircleAlert size={21} aria-hidden="true" />
        <div><strong>Current briefing</strong><span>Go to the overview’s current briefing card.</span></div>
        <Link className="button primary" to="/dashboard">Open overview</Link>
      </section>
    );
  }
  if (target.type === "briefing") {
    return (
      <section className="alert-target-card">
        <Bell size={21} aria-hidden="true" />
        <div>
          <strong>{humanize(target.edition)} briefing · {target.date}</strong>
          <span>{target.item_id ? `Item ${target.item_id}` : "Pinned briefing edition"}</span>
        </div>
        <Link
          className="button primary"
          to="/briefings/$date"
          params={{ date: target.date }}
          search={{ edition: target.edition, item: target.item_id ?? undefined }}
        >
          Open briefing
        </Link>
      </section>
    );
  }
  if (target.type === "task") {
    return (
      <section className="alert-target-card">
        <CircleAlert size={21} aria-hidden="true" />
        <div>
          <strong>Task</strong>
          <span>Open the authenticated task detail and current actions.</span>
        </div>
        <Link
          className="button primary"
          to="/tasks/$taskRef"
          params={{ taskRef: target.task_ref }}
        >
          Open task
        </Link>
      </section>
    );
  }
  return <EntryTarget target={target} />;
}

function EntryTarget({
  target,
}: {
  target: Extract<NotificationTarget, { type: "entry" }>;
}) {
  const api = useApi();
  const entryQuery = useQuery({
    queryKey: ["notification-entry-target", target.entry_ref],
    queryFn: () => api.workspaceRead({ requests: [{ ref: target.entry_ref, view: "full" }] }),
  });
  const entry = entryQuery.data?.data.items[0];
  return (
    <Section title="Linked entry" actions={<ExternalLink size={18} aria-hidden="true" />}>
      {entryQuery.isPending ? <LoadingState label="Loading linked entry" /> : null}
      {entryQuery.isError ? (
        <ErrorState error={entryQuery.error} retry={() => void entryQuery.refetch()} title="Unable to load linked entry" />
      ) : null}
      {entry ? <WorkspaceEntryView entry={entry} /> : null}
    </Section>
  );
}

function deliveryTimestamp(delivery: NotificationItem["deliveries"][number]): string {
  if (delivery.state === "suppressed") return "Not sent to the push provider";
  if (delivery.accepted_at) return `Accepted by provider ${formatDate(delivery.accepted_at)}`;
  if (delivery.failed_at) return `Failed ${formatDate(delivery.failed_at)}`;
  return "No provider outcome recorded";
}

function plainExcerpt(markdown: string): string {
  return markdown
    .replace(/!\[([^\]]*)\]\([^)]*\)/gu, "$1")
    .replace(/\[([^\]]+)\]\([^)]*\)/gu, "$1")
    .replace(/<[^>]*>/gu, " ")
    .replace(/[`*_>#~|-]+/gu, " ")
    .replace(/\s+/gu, " ")
    .trim();
}
