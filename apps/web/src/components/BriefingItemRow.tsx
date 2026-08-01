import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  BellOff,
  Check,
  ChevronDown,
  MessageSquareText,
  ZoomIn,
} from "lucide-react";
import { useId, useState } from "react";
import { useApi } from "../lib/auth";
import { formatDate } from "../lib/format";
import type { BriefingItemActionInput, BriefingItemData } from "../lib/types";
import { MarkdownView } from "./MarkdownView";
import type { Tone } from "./StateViews";

const DELTA_CHIPS: Record<string, { label: string; tone: Tone }> = {
  new: { label: "New", tone: "info" },
  update: { label: "Update", tone: "warning" },
  corroboration: { label: "Seen", tone: "neutral" },
};

const FEEDBACK_VERDICTS = [
  { verdict: "useful", label: "Useful" },
  { verdict: "not_important", label: "Not important" },
  { verdict: "already_knew", label: "Already knew" },
  { verdict: "repeated", label: "Repeated" },
  { verdict: "wrong", label: "Wrong" },
  { verdict: "follow_closer", label: "Follow closer" },
] as const;

const DATE_ONLY = /^\d{4}-\d{2}-\d{2}$/;

/**
 * Date-only values (event dates) are calendar dates, not instants; format
 * them without a timezone conversion so they never shift a day or grow a
 * fabricated clock time. Full timestamps keep the shared formatDate.
 */
function formatTimeValue(value?: string | null): string {
  if (value && DATE_ONLY.test(value)) {
    const [year, month, day] = value.split("-").map(Number);
    return new Intl.DateTimeFormat(undefined, { dateStyle: "medium" }).format(
      new Date(year, month - 1, day),
    );
  }
  return formatDate(value);
}

/** Stored payloads are not guaranteed sanitized; only link http(s) URLs. */
function isHttpUrl(url: string): boolean {
  try {
    const protocol = new URL(url).protocol;
    return protocol === "http:" || protocol === "https:";
  } catch {
    return false;
  }
}

function sourceLabel(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return url;
  }
}

export function BriefingItemRow({
  item,
  sectionTitle,
  topicSlug,
  editionRef,
  readOnly,
}: {
  item: BriefingItemData;
  sectionTitle: string;
  topicSlug: string;
  editionRef: string;
  readOnly: boolean;
}) {
  const api = useApi();
  const queryClient = useQueryClient();
  const detailId = useId();
  const [open, setOpen] = useState(false);
  const [feedbackOpen, setFeedbackOpen] = useState(false);
  const actionMutation = useMutation({
    mutationFn: (input: BriefingItemActionInput) =>
      api.briefingItemAction(input),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["briefings"] });
      void queryClient.invalidateQueries({ queryKey: ["briefing-topics"] });
    },
  });

  const chip = item.delta ? DELTA_CHIPS[item.delta] : undefined;
  // House-style headlines are bold links; the collapsed row unwraps them to
  // plain text, so the linked headline must stay reachable in the detail.
  const headlineHasLink = /\[[^\]]*\]\([^)]*\)/.test(item.headline_md);
  // The API serializes these as empty strings, never null; check content.
  const bodyMarkdown = (item.body_md ?? "").trim();
  const detailMarkdown = (item.detail_md ?? "").trim();
  const urls = item.story?.urls ?? [];
  const times = (
    [
      ["Published", item.times?.published_at],
      ["Event", item.times?.event_at],
      ["First seen", item.times?.first_seen_at],
    ] as const
  ).filter(([, value]) => Boolean(value));

  function act(input: BriefingItemActionInput) {
    setFeedbackOpen(false);
    actionMutation.mutate(input);
  }

  return (
    <div className="briefing-item">
      <button
        type="button"
        className="briefing-index-row"
        aria-expanded={open}
        aria-controls={open ? detailId : undefined}
        onClick={() => setOpen((value) => !value)}
      >
        <span className="briefing-kicker">{sectionTitle}</span>
        <MarkdownView
          className="briefing-headline"
          markdown={item.headline_md}
          stripAnchors
        />
        {chip ? (
          <span className={`state-chip tone-${chip.tone}`}>{chip.label}</span>
        ) : null}
        <ChevronDown
          size={16}
          className="briefing-chevron"
          aria-hidden="true"
        />
      </button>
      {open ? (
        <div
          className="expanded-detail"
          id={detailId}
          role="region"
          aria-label={`${sectionTitle} item detail`}
        >
          {headlineHasLink ? (
            <MarkdownView
              className="briefing-detail-headline"
              markdown={item.headline_md}
            />
          ) : null}
          {bodyMarkdown ? <MarkdownView markdown={bodyMarkdown} /> : null}
          {detailMarkdown ? <MarkdownView markdown={detailMarkdown} /> : null}
          {item.what_changed ? (
            <p className="what-changed">
              <em>What changed:</em> {item.what_changed}
            </p>
          ) : null}
          {urls.length || times.length ? (
            <div className="briefing-sources">
              {urls.map((url) =>
                isHttpUrl(url) ? (
                  <a
                    key={url}
                    href={url}
                    target="_blank"
                    rel="noreferrer noopener"
                  >
                    {sourceLabel(url)}
                  </a>
                ) : (
                  <span key={url}>{url}</span>
                ),
              )}
              {times.length ? (
                <span>
                  {times
                    .map(
                      ([label, value]) => `${label} ${formatTimeValue(value)}`,
                    )
                    .join(" · ")}
                </span>
              ) : null}
            </div>
          ) : null}
          <div className="item-actions">
            <button
              className="button secondary"
              type="button"
              disabled={readOnly || actionMutation.isPending}
              onClick={() =>
                act({
                  action: "read",
                  edition_ref: editionRef,
                  item_id: item.id,
                })
              }
            >
              <Check size={16} aria-hidden="true" />
              Mark read
            </button>
            <button
              className="button secondary"
              type="button"
              disabled={readOnly || actionMutation.isPending}
              onClick={() =>
                act({
                  action: "expand",
                  edition_ref: editionRef,
                  item_id: item.id,
                })
              }
            >
              <ZoomIn size={16} aria-hidden="true" />
              Go deeper
            </button>
            <div className="feedback-menu-wrap">
              <button
                className="button secondary"
                type="button"
                aria-haspopup="menu"
                aria-expanded={feedbackOpen}
                disabled={readOnly || actionMutation.isPending}
                onClick={() => setFeedbackOpen((value) => !value)}
              >
                <MessageSquareText size={16} aria-hidden="true" />
                Feedback
                <ChevronDown size={14} aria-hidden="true" />
              </button>
              {feedbackOpen ? (
                <div className="feedback-menu">
                  {FEEDBACK_VERDICTS.map(({ verdict, label }) => (
                    <button
                      key={verdict}
                      type="button"
                      disabled={readOnly || actionMutation.isPending}
                      onClick={() =>
                        act({
                          action: "feedback",
                          edition_ref: editionRef,
                          item_id: item.id,
                          verdict,
                        })
                      }
                    >
                      {label}
                    </button>
                  ))}
                </div>
              ) : null}
            </div>
            <button
              className="button secondary"
              type="button"
              disabled={readOnly || actionMutation.isPending}
              onClick={() =>
                act({ action: "mute_topic", topic_slug: topicSlug })
              }
            >
              <BellOff size={16} aria-hidden="true" />
              Mute topic
            </button>
          </div>
          {actionMutation.isError ? (
            <span className="field-error" role="alert">
              {actionMutation.error instanceof Error
                ? actionMutation.error.message
                : "The action could not be recorded."}
            </span>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
