import { FileText } from "lucide-react";
import { useState } from "react";
import { useNavigate } from "@tanstack/react-router";
import { JsonView } from "./JsonView";
import { MarkdownView, type MarkdownEntryLink } from "./MarkdownView";
import { DefinitionList } from "./Page";
import { StatusBadge } from "./StateViews";
import { formatDate, shortId } from "../lib/format";
import type { WorkspaceReadItem } from "../lib/types";

export interface WorkspaceEntryNavigationTarget {
  ref?: string;
  path?: string;
  alternatePaths?: string[];
  linkTarget?: string;
  fallbackQuery?: string;
}

export interface WorkspaceEntryNavigationSearch {
  entryRef?: string;
  entryPath?: string;
  alternatePaths?: string;
  linkTarget?: string;
  fallbackQuery?: string;
}

export function workspaceEntryNavigationSearch(
  target: WorkspaceEntryNavigationTarget,
): WorkspaceEntryNavigationSearch {
  return {
    entryRef: target.ref,
    entryPath: target.path,
    alternatePaths: target.alternatePaths?.join("\n"),
    linkTarget: target.linkTarget,
    fallbackQuery: target.fallbackQuery,
  };
}

function alternateSourcePaths(path: string): string[] | undefined {
  if (path.startsWith("sources/") || path.startsWith(".brunn/")) {
    return undefined;
  }
  return [`sources/${path}`];
}

function decodeTarget(value: string): string | undefined {
  try {
    return decodeURIComponent(value);
  } catch {
    return undefined;
  }
}

function stripTargetSuffix(value: string): string {
  return value.split(/[?#]/, 1)[0].trim();
}

function hasMarkdownExtension(value: string): boolean {
  return /\.(?:md|markdown)$/i.test(value);
}

function normalizePath(basePath: string, targetPath: string): string | undefined {
  const parts = targetPath.startsWith("/")
    ? []
    : basePath.split("/").slice(0, -1);
  for (const part of targetPath.replace(/^\/+/, "").split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (!parts.length) return undefined;
      parts.pop();
      continue;
    }
    parts.push(part);
  }
  return parts.length ? parts.join("/") : undefined;
}

function customEntryTarget(value: string): WorkspaceEntryNavigationTarget | undefined {
  const decodedValue = decodeTarget(value);
  if (!decodedValue) return undefined;
  const target = stripTargetSuffix(decodedValue);
  if (target.startsWith("entry:")) return { ref: target };
  if (!target.toLowerCase().startsWith("brunn://entry/")) return undefined;
  try {
    const url = new URL(target);
    if (url.protocol !== "brunn:" || url.hostname !== "entry") return undefined;
    const decoded = url.pathname.replace(/^\/+/, "");
    if (!decoded) return undefined;
    if (decoded.startsWith("entry:")) return { ref: decoded };
    if (/^[0-9a-f-]{32,36}$/i.test(decoded)) return { ref: `entry:${decoded}` };
    if (hasMarkdownExtension(decoded)) {
      return {
        path: decoded,
        alternatePaths: alternateSourcePaths(decoded),
        fallbackQuery: decoded,
      };
    }
  } catch {
    return undefined;
  }
  return undefined;
}

export function resolveWorkspaceEntryLink(
  sourcePath: string,
  link: MarkdownEntryLink,
): WorkspaceEntryNavigationTarget | undefined {
  const custom = customEntryTarget(link.target);
  if (custom) return custom;

  const decodedTarget = decodeTarget(link.target);
  const decoded = decodedTarget ? stripTargetSuffix(decodedTarget) : undefined;
  if (!decoded || /^[a-z][a-z\d+.\-]*:/i.test(decoded)) return undefined;

  if (link.kind === "wiki") {
    const withExtension = hasMarkdownExtension(decoded) ? decoded : `${decoded}.md`;
    const isExplicitRelative = /^(?:\.\.?\/)/.test(withExtension);
    const isBareWikiLink = !isExplicitRelative && !decoded.startsWith("/")
      && !decoded.includes("/");
    const path = normalizePath(
      sourcePath,
      decoded.startsWith("/") || (!isExplicitRelative && decoded.includes("/"))
        ? `/${withExtension}`
        : withExtension,
    );
    if (!path) return undefined;
    const alternatePaths = [
      ...(alternateSourcePaths(path) ?? []),
      ...(isBareWikiLink
        ? [
            normalizePath("", `/${withExtension}`),
            normalizePath("", `/sources/${withExtension}`),
          ]
        : []),
    ].filter((candidate): candidate is string => Boolean(candidate) && candidate !== path);
    return {
      path,
      alternatePaths: [...new Set(alternatePaths)],
      ...(isBareWikiLink ? { linkTarget: decoded } : {}),
      fallbackQuery: decoded.replace(/^\//, ""),
    };
  }

  const path = normalizePath(sourcePath, decoded);
  return path
    ? {
        path,
        alternatePaths: alternateSourcePaths(path),
        fallbackQuery: path,
      }
    : undefined;
}

export function WorkspaceEntryView({
  entry,
  onEntryLink,
}: {
  entry: WorkspaceReadItem;
  onEntryLink?: (target: WorkspaceEntryNavigationTarget) => void;
}) {
  const navigate = useNavigate();
  const [formatMarkdown, setFormatMarkdown] = useState(true);
  const canFormatMarkdown = entry.media_type === "text/markdown";

  function openEntryLink(link: MarkdownEntryLink) {
    const target = resolveWorkspaceEntryLink(entry.path, link);
    if (!target) return;
    if (onEntryLink) {
      onEntryLink(target);
      return;
    }
    void navigate({ to: "/explore", search: workspaceEntryNavigationSearch(target) });
  }

  return (
    <div className="workspace-entry-view">
      <header>
        <div>
          <FileText size={18} aria-hidden="true" />
          <div>
            <strong>{entry.title}</strong>
            <code>{entry.path}</code>
          </div>
        </div>
        <div className="entry-header-actions">
          <StatusBadge status={entry.view} />
          {canFormatMarkdown ? (
            <div className="entry-view-toggle" aria-label="Entry display" role="group">
              <button
                type="button"
                aria-pressed={formatMarkdown}
                onClick={() => setFormatMarkdown(true)}
              >
                Formatted
              </button>
              <button
                type="button"
                aria-pressed={!formatMarkdown}
                onClick={() => setFormatMarkdown(false)}
              >
                Source
              </button>
            </div>
          ) : null}
        </div>
      </header>
      <DefinitionList
        items={[
          { label: "Reference", value: <code>{entry.reference}</code> },
          { label: "Version", value: `v${entry.version}` },
          {
            label: "Hash",
            value: (
              <code title={entry.content_hash}>
                {shortId(entry.content_hash, 18)}
              </code>
            ),
          },
          { label: "Updated", value: formatDate(entry.updated_at) },
        ]}
      />
      {formatMarkdown && canFormatMarkdown ? (
        <MarkdownView
          markdown={entry.text}
          className="entry-markdown"
          onEntryLink={openEntryLink}
          ariaLabel={`${entry.title} content`}
        />
      ) : (
        <pre
          className="markdown-source"
          aria-label={`${entry.title} content`}
        >
          {entry.text}
        </pre>
      )}
      {entry.metadata &&
      typeof entry.metadata === "object" &&
      Object.keys(entry.metadata).length ? (
        <JsonView value={entry.metadata} label="Entry metadata" />
      ) : null}
    </div>
  );
}
