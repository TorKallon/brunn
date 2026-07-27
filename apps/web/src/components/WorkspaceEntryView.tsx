import { FileText } from "lucide-react";
import { JsonView } from "./JsonView";
import { DefinitionList } from "./Page";
import { StatusBadge } from "./StateViews";
import { formatDate, shortId } from "../lib/format";
import type { WorkspaceReadItem } from "../lib/types";

export function WorkspaceEntryView({ entry }: { entry: WorkspaceReadItem }) {
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
        <StatusBadge status={entry.view} />
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
      <pre
        className="markdown-source"
        aria-label={`${entry.title} content`}
      >
        {entry.text}
      </pre>
      {entry.metadata &&
      typeof entry.metadata === "object" &&
      Object.keys(entry.metadata).length ? (
        <JsonView value={entry.metadata} label="Entry metadata" />
      ) : null}
    </div>
  );
}
