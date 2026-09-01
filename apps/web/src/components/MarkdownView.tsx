import DOMPurify, { type Config } from "dompurify";
import { Marked } from "marked";
import { type MouseEvent, useMemo } from "react";

const WIKI_ENTRY_QUERY = "?brunn-entry=";
const CUSTOM_ENTRY_QUERY = "?brunn-entry-link=";

function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

const markdownParser = new Marked({
  gfm: true,
  extensions: [
    {
      name: "wikiLink",
      level: "inline",
      start(source) {
        const index = source.indexOf("[[");
        return index >= 0 ? index : undefined;
      },
      tokenizer(source) {
        const match = /^\[\[([^\]\n]+)\]\]/.exec(source);
        if (!match) return undefined;
        const [targetPart, ...labelParts] = match[1].split("|");
        const target = targetPart.trim();
        if (!target) return undefined;
        return {
          type: "wikiLink",
          raw: match[0],
          target,
          label: labelParts.join("|").trim() || target.split("#", 1)[0],
        };
      },
      renderer(token) {
        const target = String(token.target);
        const label = String(token.label);
        return `<a href="${WIKI_ENTRY_QUERY}${encodeURIComponent(target)}">${escapeHtml(label)}</a>`;
      },
    },
  ],
});

const SANITIZE_CONFIG: Config = {
  ALLOWED_TAGS: [
    "a",
    "blockquote",
    "br",
    "code",
    "del",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "li",
    "ol",
    "p",
    "pre",
    "s",
    "strong",
    "sub",
    "sup",
    "table",
    "tbody",
    "td",
    "th",
    "thead",
    "tr",
    "u",
    "ul",
  ],
  ALLOWED_ATTR: ["href", "title"],
  ALLOW_DATA_ATTR: false,
  FORBID_TAGS: ["svg", "math", "style"],
  FORBID_ATTR: ["style"],
};

const EXTERNAL_HREF = /^https?:\/\//i;

export interface MarkdownEntryLink {
  kind: "markdown" | "wiki";
  target: string;
}

function decodeWikiTarget(href: string): string | undefined {
  if (!href.startsWith(WIKI_ENTRY_QUERY)) return undefined;
  try {
    return decodeURIComponent(href.slice(WIKI_ENTRY_QUERY.length));
  } catch {
    return undefined;
  }
}

function rewriteCustomEntryHrefs(html: string): string {
  const template = document.createElement("template");
  template.innerHTML = html;
  for (const anchor of template.content.querySelectorAll<HTMLAnchorElement>("a[href]")) {
    const href = anchor.getAttribute("href") ?? "";
    if (/^(?:entry:|brunn:\/\/entry\/)/i.test(href)) {
      anchor.setAttribute("href", `${CUSTOM_ENTRY_QUERY}${encodeURIComponent(href)}`);
    }
  }
  return template.innerHTML;
}

function isEntryHref(href: string): boolean {
  if (/^(?:entry:|brunn:\/\/entry\/)/i.test(href)) return true;
  if (/^(?:https?:|mailto:|tel:|callto:|sms:|cid:|xmpp:|matrix:)/i.test(href)) {
    return false;
  }
  if (href.startsWith("#") || href.startsWith("?") || href.startsWith("//")) {
    return false;
  }
  const path = href.split(/[?#]/, 1)[0];
  return /\.(?:md|markdown)$/i.test(path) || !path.startsWith("/");
}

function renderMarkdown(markdown: string, stripAnchors: boolean): string {
  // Core marked emits no heading ids; gfm covers tables and strikethrough.
  const html = markdownParser.parse(markdown, { async: false });
  const sanitized = DOMPurify.sanitize(rewriteCustomEntryHrefs(html), SANITIZE_CONFIG);
  const template = document.createElement("template");
  template.innerHTML = sanitized;
  if (stripAnchors) {
    // Unwrap anchors to their children so link-bearing markdown can render
    // inside interactive elements without nesting interactive content.
    for (const anchor of [...template.content.querySelectorAll("a")]) {
      anchor.replaceWith(...anchor.childNodes);
    }
  } else {
    for (const anchor of template.content.querySelectorAll<HTMLAnchorElement>("a[href]")) {
      const href = anchor.getAttribute("href") ?? "";
      const wikiTarget = decodeWikiTarget(href);
      const customTarget = href.startsWith(CUSTOM_ENTRY_QUERY)
        ? decodeWikiTarget(href.replace(CUSTOM_ENTRY_QUERY, WIKI_ENTRY_QUERY))
        : undefined;
      if (wikiTarget) {
        anchor.dataset.entryKind = "wiki";
        anchor.dataset.entryTarget = wikiTarget;
      } else if (customTarget) {
        anchor.dataset.entryKind = "markdown";
        anchor.dataset.entryTarget = customTarget;
      } else if (isEntryHref(href)) {
        anchor.dataset.entryKind = "markdown";
        anchor.dataset.entryTarget = href;
      } else if (EXTERNAL_HREF.test(href)) {
        anchor.setAttribute("target", "_blank");
        anchor.setAttribute("rel", "noreferrer noopener");
      }
    }
  }
  return template.innerHTML;
}

export function MarkdownView({
  markdown,
  className,
  stripAnchors = false,
  onEntryLink,
  ariaLabel,
}: {
  markdown: string;
  className?: string;
  stripAnchors?: boolean;
  onEntryLink?: (link: MarkdownEntryLink) => void;
  ariaLabel?: string;
}) {
  const html = useMemo(
    () => renderMarkdown(markdown, stripAnchors),
    [markdown, stripAnchors],
  );

  function handleClick(event: MouseEvent<HTMLDivElement>) {
    if (!onEntryLink || event.button !== 0) return;
    const clicked = event.target;
    if (!(clicked instanceof Element)) return;
    const anchor = clicked.closest<HTMLAnchorElement>("a[data-entry-target]");
    if (!anchor || !event.currentTarget.contains(anchor)) return;
    const target = anchor.dataset.entryTarget;
    const kind = anchor.dataset.entryKind;
    if (!target || (kind !== "markdown" && kind !== "wiki")) return;
    event.preventDefault();
    onEntryLink({ kind, target });
  }

  return (
    <div
      className={className ? `markdown-view ${className}` : "markdown-view"}
      onClick={handleClick}
      aria-label={ariaLabel}
      // Safe: the markup above is rendered by marked and sanitized by
      // DOMPurify against an explicit allowlist before it reaches React.
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
