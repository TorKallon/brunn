import DOMPurify, { type Config } from "dompurify";
import { marked } from "marked";
import { useMemo } from "react";

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

function renderMarkdown(markdown: string, stripAnchors: boolean): string {
  // Core marked emits no heading ids; gfm covers tables and strikethrough.
  const html = marked.parse(markdown, { async: false, gfm: true });
  const sanitized = DOMPurify.sanitize(html, SANITIZE_CONFIG);
  const template = document.createElement("template");
  template.innerHTML = sanitized;
  if (stripAnchors) {
    // Unwrap anchors to their children so link-bearing markdown can render
    // inside interactive elements without nesting interactive content.
    for (const anchor of [...template.content.querySelectorAll("a")]) {
      anchor.replaceWith(...anchor.childNodes);
    }
  } else {
    for (const anchor of template.content.querySelectorAll("a[href]")) {
      if (EXTERNAL_HREF.test(anchor.getAttribute("href") ?? "")) {
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
}: {
  markdown: string;
  className?: string;
  stripAnchors?: boolean;
}) {
  const html = useMemo(
    () => renderMarkdown(markdown, stripAnchors),
    [markdown, stripAnchors],
  );
  return (
    <div
      className={className ? `markdown-view ${className}` : "markdown-view"}
      // Safe: the markup above is rendered by marked and sanitized by
      // DOMPurify against an explicit allowlist before it reaches React.
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
