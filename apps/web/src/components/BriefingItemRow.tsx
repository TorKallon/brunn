import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  BellOff,
  Check,
  ChevronDown,
  Repeat2,
  ThumbsUp,
  ZoomIn,
  type LucideIcon,
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
  const detailMarkdown = item.detail_md ?? item.body_md;
  const urls = item.story?.urls ?? [];
  const times = (
    [
      ["Published", item.times?.published_at],
      ["Event", item.times?.event_at],
      ["First seen", item.times?.first_seen_at],
    ] as const
  ).filter(([, value]) => Boolean(value));
  const actions: Array<{
    label: string;
    icon: LucideIcon;
    input: BriefingItemActionInput;
  }> = [
    {
      label: "Mark read",
      icon: Check,
      input: { action: "read", edition_ref: editionRef, item_id: item.id },
    },
    {
      label: "Go deeper",
      icon: ZoomIn,
      input: { action: "expand", edition_ref: editionRef, item_id: item.id },
    },
    {
      label: "Useful",
      icon: ThumbsUp,
      input: {
        action: "feedback",
        edition_ref: editionRef,
        item_id: item.id,
        verdict: "useful",
      },
    },
    {
      label: "Repeated",
      icon: Repeat2,
      input: {
        action: "feedback",
        edition_ref: editionRef,
        item_id: item.id,
        verdict: "repeated",
      },
    },
    {
      label: "Mute topic",
      icon: BellOff,
      input: { action: "mute_topic", topic_slug: topicSlug },
    },
  ];

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
          {detailMarkdown ? <MarkdownView markdown={detailMarkdown} /> : null}
          {item.what_changed ? (
            <p className="what-changed">
              <em>What changed:</em> {item.what_changed}
            </p>
          ) : null}
          {urls.length ? (
            <div className="briefing-sources">
              {urls.map((url) => (
                <a
                  key={url}
                  href={url}
                  target="_blank"
                  rel="noreferrer noopener"
                >
                  {sourceLabel(url)}
                </a>
              ))}
              {times.length ? (
                <span>
                  {times
                    .map(([label, value]) => `${label} ${formatDate(value)}`)
                    .join(" · ")}
                </span>
              ) : null}
            </div>
          ) : null}
          <div className="item-actions">
            {actions.map(({ label, icon: Icon, input }) => (
              <button
                key={label}
                className="button secondary"
                type="button"
                disabled={readOnly || actionMutation.isPending}
                onClick={() => actionMutation.mutate(input)}
              >
                <Icon size={16} aria-hidden="true" />
                {label}
              </button>
            ))}
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
