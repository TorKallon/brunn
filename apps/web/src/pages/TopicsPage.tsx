import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";
import { Layers3, MessageSquareText, Plus, Save } from "lucide-react";
import { type FormEvent, useMemo, useState } from "react";
import { DataTable } from "../components/DataTable";
import { Page, PageHeader, Section } from "../components/Page";
import {
  EmptyState,
  ErrorState,
  LoadingState,
  ReadOnlyNotice,
  StatusBadge,
  type Tone,
} from "../components/StateViews";
import { useApi } from "../lib/auth";
import { useReadOnly } from "../lib/current";
import { humanize } from "../lib/format";
import type { BriefingTopic } from "../lib/types";

const TOPIC_MODES = [
  "every_briefing",
  "on_material_delta",
  "scheduled",
  "paused",
  "muted",
] as const;

const MODE_TONES: Record<string, Tone> = {
  every_briefing: "info",
  on_material_delta: "neutral",
  scheduled: "info",
  paused: "warning",
  muted: "danger",
};

const DEFAULT_SECTION_ORDER = 1_000;
const DEFAULT_FRESHNESS_HOURS = 48;

interface TopicDraft {
  isNew: boolean;
  parseError?: string;
  path: string;
  expectedVersion: number;
  slug: string;
  name: string;
  sectionOrder: string;
  mode: string;
  editions: string;
  schedule: string;
  entities: string;
  symbols: string;
  suppressUnchanged: boolean;
  freshnessHours: string;
  body: string;
}

function emptyDraft(): TopicDraft {
  return {
    isNew: true,
    path: "",
    expectedVersion: 0,
    slug: "",
    name: "",
    sectionOrder: String(DEFAULT_SECTION_ORDER),
    mode: "every_briefing",
    editions: "morning",
    schedule: "",
    entities: "",
    symbols: "",
    suppressUnchanged: true,
    freshnessHours: String(DEFAULT_FRESHNESS_HOURS),
    body: "",
  };
}

function draftFromTopic(topic: BriefingTopic): TopicDraft {
  return {
    isNew: false,
    ...(topic.parse_error !== undefined
      ? { parseError: topic.parse_error }
      : {}),
    path: topic.path,
    expectedVersion: topic.version,
    slug: topic.slug,
    name: topic.name,
    sectionOrder: String(topic.section_order),
    mode: topic.mode,
    editions: topic.editions.join(", "),
    schedule: topic.schedule ?? "",
    entities: topic.entities.join(", "),
    symbols: topic.symbols.join(", "),
    suppressUnchanged: topic.suppress_unchanged,
    freshnessHours: String(topic.freshness_hours),
    body: topic.body,
  };
}

function parseList(value: string): string[] {
  return value
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
}

function parseCount(value: string, fallback: number): number {
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) ? parsed : fallback;
}

/**
 * Serialize a topic draft into the frontmatter document parse_topic_document
 * reads back: fenced key/value lines in the canonical field order, inline
 * `[a, b]` lists, and the instructions body after one blank line.
 */
export function serializeBriefingTopic(draft: TopicDraft): string {
  const schedule = draft.schedule.trim();
  const lines = [
    "---",
    "kind: briefing_topic",
    `slug: ${draft.slug.trim()}`,
    `name: ${draft.name.trim()}`,
    `section_order: ${parseCount(draft.sectionOrder, DEFAULT_SECTION_ORDER)}`,
    `mode: ${draft.mode}`,
    `editions: [${parseList(draft.editions).join(", ")}]`,
    ...(schedule ? [`schedule: ${schedule}`] : []),
    `entities: [${parseList(draft.entities).join(", ")}]`,
    `symbols: [${parseList(draft.symbols).join(", ")}]`,
    `suppress_unchanged: ${draft.suppressUnchanged}`,
    `freshness_hours: ${parseCount(
      draft.freshnessHours,
      DEFAULT_FRESHNESS_HOURS,
    )}`,
    "---",
  ];
  const body = draft.body.replace(/\s+$/, "");
  return body ? `${lines.join("\n")}\n\n${body}\n` : `${lines.join("\n")}\n`;
}

export function TopicsPage() {
  const api = useApi();
  const readOnly = useReadOnly();
  const queryClient = useQueryClient();
  const [draft, setDraft] = useState<TopicDraft | null>(null);
  const topicsQuery = useQuery({
    queryKey: ["briefing-topics"],
    queryFn: () => api.briefingTopics(),
  });
  const saveMutation = useMutation({
    mutationFn: (input: {
      path: string;
      content: string;
      expected_version: number;
    }) =>
      api.workspaceWrite({
        path: input.path,
        content: input.content,
        expected_version: input.expected_version,
      }),
    onSuccess: (response) => {
      setDraft((current) =>
        current
          ? {
              ...current,
              isNew: false,
              path: response.data.path,
              expectedVersion: response.data.version,
            }
          : current,
      );
      void queryClient.invalidateQueries({ queryKey: ["briefing-topics"] });
    },
  });

  const snapshot = topicsQuery.data?.data;
  const topics = snapshot?.topics ?? [];
  const pendingRequests = snapshot?.pending_requests ?? [];
  const feedbackTail = snapshot?.feedback_tail ?? [];

  const columns = useMemo<ColumnDef<BriefingTopic, unknown>[]>(
    () => [
      {
        id: "name",
        header: "Name",
        cell: ({ row }) => (
          <div className="primary-cell">
            <strong>{row.original.name}</strong>
            <code>{row.original.slug}</code>
            {row.original.parse_error ? (
              <span className="status-badge tone-warning">Parse error</span>
            ) : null}
          </div>
        ),
      },
      {
        accessorKey: "mode",
        header: "Mode",
        cell: ({ getValue }) => {
          const mode = getValue<string>();
          return (
            <span
              className={`status-badge tone-${MODE_TONES[mode] ?? "neutral"}`}
            >
              {humanize(mode)}
            </span>
          );
        },
      },
      { accessorKey: "section_order", header: "Order" },
      {
        id: "editions",
        header: "Editions",
        cell: ({ row }) => row.original.editions.join(", "),
      },
      {
        accessorKey: "version",
        header: "Updated",
        cell: ({ getValue }) => `v${getValue<number>()}`,
      },
    ],
    [],
  );

  function openDraft(next: TopicDraft) {
    saveMutation.reset();
    setDraft(next);
  }

  function submitTopic(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (readOnly || !draft || !draft.slug.trim()) return;
    const path = draft.isNew
      ? `Briefings/Topics/${draft.slug.trim()}.md`
      : draft.path;
    const content =
      draft.parseError !== undefined
        ? draft.body
        : serializeBriefingTopic(draft);
    saveMutation.mutate({
      path,
      content,
      expected_version: draft.expectedVersion,
    });
  }

  function updateDraft(patch: Partial<TopicDraft>) {
    setDraft((current) => (current ? { ...current, ...patch } : current));
  }

  return (
    <Page>
      <PageHeader
        title="Topics"
        description="Briefing topics, delivery modes, and agent instructions"
        actions={
          <button
            className="button primary"
            type="button"
            onClick={() => openDraft(emptyDraft())}
            disabled={readOnly}
            title={readOnly ? "Requires a read/write credential" : undefined}
          >
            <Plus size={17} aria-hidden="true" />
            New topic
          </button>
        }
      />
      {readOnly ? <ReadOnlyNotice /> : null}

      <div className="workspace-split">
        <Section
          title="Configured topics"
          meta={topics.length ? `${topics.length} loaded` : undefined}
          actions={<Layers3 size={18} aria-hidden="true" />}
        >
          {topicsQuery.isPending ? (
            <LoadingState label="Loading topics" />
          ) : null}
          {topicsQuery.isError ? (
            <ErrorState
              error={topicsQuery.error}
              retry={() => void topicsQuery.refetch()}
              title="Unable to load topics"
            />
          ) : null}
          {topicsQuery.isSuccess ? (
            <DataTable
              data={topics}
              columns={columns}
              onRowClick={(topic) => openDraft(draftFromTopic(topic))}
              getRowLabel={(topic) => `Edit topic ${topic.name}`}
              emptyTitle="No topics configured"
            />
          ) : null}
        </Section>

        <Section
          title={
            draft
              ? draft.isNew
                ? "New topic"
                : `Edit ${draft.name || draft.slug}`
              : "Topic editor"
          }
          meta={
            draft
              ? draft.isNew
                ? "expects v0"
                : `v${draft.expectedVersion}`
              : undefined
          }
        >
          {!draft ? (
            <EmptyState
              title="No topic selected"
              detail="Select a topic to edit it, or create a new one."
            />
          ) : (
            <form className="form-grid" onSubmit={submitTopic}>
              {draft.parseError !== undefined ? (
                <div className="field-span-2 topic-parse-error" role="status">
                  <span className="status-badge tone-warning">Parse error</span>
                  <span>{draft.parseError}</span>
                </div>
              ) : null}
              <label className="field">
                <span>Slug</span>
                <input
                  value={draft.slug}
                  onChange={(event) =>
                    updateDraft({ slug: event.target.value })
                  }
                  readOnly={!draft.isNew}
                  disabled={readOnly}
                  spellCheck={false}
                  required
                />
              </label>
              {draft.parseError === undefined ? (
                <>
                  <label className="field">
                    <span>Name</span>
                    <input
                      value={draft.name}
                      onChange={(event) =>
                        updateDraft({ name: event.target.value })
                      }
                      disabled={readOnly}
                      required
                    />
                  </label>
                  <label className="field">
                    <span>Section order</span>
                    <input
                      type="number"
                      value={draft.sectionOrder}
                      onChange={(event) =>
                        updateDraft({ sectionOrder: event.target.value })
                      }
                      disabled={readOnly}
                    />
                  </label>
                  <label className="field">
                    <span>Mode</span>
                    <select
                      value={draft.mode}
                      onChange={(event) =>
                        updateDraft({ mode: event.target.value })
                      }
                      disabled={readOnly}
                    >
                      {TOPIC_MODES.map((mode) => (
                        <option key={mode} value={mode}>
                          {humanize(mode)}
                        </option>
                      ))}
                    </select>
                  </label>
                  <label className="field">
                    <span>Editions</span>
                    <input
                      value={draft.editions}
                      onChange={(event) =>
                        updateDraft({ editions: event.target.value })
                      }
                      disabled={readOnly}
                      spellCheck={false}
                    />
                    <span className="field-help">
                      Comma-separated, e.g. morning, health-update
                    </span>
                  </label>
                  <label className="field">
                    <span>Schedule</span>
                    <input
                      value={draft.schedule}
                      onChange={(event) =>
                        updateDraft({ schedule: event.target.value })
                      }
                      disabled={readOnly}
                      spellCheck={false}
                    />
                    <span className="field-help">
                      Optional, e.g. 10:00 America/Los_Angeles
                    </span>
                  </label>
                  <label className="field">
                    <span>Entities</span>
                    <input
                      value={draft.entities}
                      onChange={(event) =>
                        updateDraft({ entities: event.target.value })
                      }
                      disabled={readOnly}
                      spellCheck={false}
                    />
                    <span className="field-help">Comma-separated names</span>
                  </label>
                  <label className="field">
                    <span>Symbols</span>
                    <input
                      value={draft.symbols}
                      onChange={(event) =>
                        updateDraft({ symbols: event.target.value })
                      }
                      disabled={readOnly}
                      spellCheck={false}
                    />
                    <span className="field-help">
                      Comma-separated tickers
                    </span>
                  </label>
                  <label className="field">
                    <span>Freshness hours</span>
                    <input
                      type="number"
                      min={1}
                      value={draft.freshnessHours}
                      onChange={(event) =>
                        updateDraft({ freshnessHours: event.target.value })
                      }
                      disabled={readOnly}
                    />
                  </label>
                  <label className="checkbox-field">
                    <input
                      type="checkbox"
                      checked={draft.suppressUnchanged}
                      onChange={(event) =>
                        updateDraft({
                          suppressUnchanged: event.target.checked,
                        })
                      }
                      disabled={readOnly}
                    />
                    Suppress unchanged items
                  </label>
                  <label className="field field-span-2">
                    <span>Instructions</span>
                    <textarea
                      rows={9}
                      value={draft.body}
                      onChange={(event) =>
                        updateDraft({ body: event.target.value })
                      }
                      disabled={readOnly}
                    />
                    <span className="field-help">
                      Briefing agents read this prose verbatim when
                      researching the topic.
                    </span>
                  </label>
                </>
              ) : (
                <label className="field field-span-2">
                  <span>Raw document</span>
                  <textarea
                    className="code-input"
                    rows={12}
                    value={draft.body}
                    onChange={(event) =>
                      updateDraft({ body: event.target.value })
                    }
                    disabled={readOnly}
                    spellCheck={false}
                  />
                  <span className="field-help">
                    Saved verbatim; restore the frontmatter fences to clear
                    the parse error.
                  </span>
                </label>
              )}
              <div className="form-actions field-span-2">
                <button
                  className="button primary"
                  type="submit"
                  disabled={
                    readOnly || !draft.slug.trim() || saveMutation.isPending
                  }
                >
                  <Save size={17} aria-hidden="true" />
                  {saveMutation.isPending ? "Saving" : "Save topic"}
                </button>
                {saveMutation.isSuccess ? (
                  <span className="success-message">
                    Saved v{saveMutation.data.data.version}
                  </span>
                ) : null}
              </div>
              {saveMutation.isError ? (
                <div className="field-span-2">
                  <ErrorState
                    error={saveMutation.error}
                    title="Unable to save topic"
                  />
                </div>
              ) : null}
            </form>
          )}
        </Section>
      </div>

      <Section
        title="Pending requests"
        meta={
          pendingRequests.length ? `${pendingRequests.length} open` : undefined
        }
      >
        {pendingRequests.length ? (
          <div className="pending-list">
            {pendingRequests.map((request) => (
              <div key={request.path}>
                <StatusBadge status="pending" />
                <div>
                  <strong>{request.item_id ?? request.path}</strong>
                  <code>{request.edition_ref ?? request.path}</code>
                  {request.note ? <span>{request.note}</span> : null}
                </div>
                <span>{request.date ?? ""}</span>
              </div>
            ))}
          </div>
        ) : (
          <EmptyState title="No pending requests" />
        )}
      </Section>

      <Section
        title="Feedback tail"
        meta={
          snapshot?.feedback_path ? (
            <code>{snapshot.feedback_path}</code>
          ) : undefined
        }
        actions={<MessageSquareText size={18} aria-hidden="true" />}
      >
        {feedbackTail.length ? (
          <pre className="markdown-source compact-source">
            {feedbackTail.join("\n")}
          </pre>
        ) : (
          <EmptyState title="No feedback recorded" />
        )}
      </Section>
    </Page>
  );
}
