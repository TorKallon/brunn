import type {
  JsonObject,
  JsonValue,
  WorkspaceManifestEntry,
  WorkspaceSearchCandidate,
} from "./types";

export function workspaceEntryRef(
  candidate: Pick<
    WorkspaceSearchCandidate,
    "reference" | "entry_id" | "entry_ref"
  >,
): string {
  return (
    candidate.reference ??
    candidate.entry_ref ??
    (candidate.entry_id ? `entry:${candidate.entry_id}` : "")
  );
}

export function metadataObject(value: JsonValue | undefined): JsonObject {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const object = value as JsonObject;
  const client = object.client;
  return client && typeof client === "object" && !Array.isArray(client)
    ? (client as JsonObject)
    : object;
}

export function workspaceEntryKind(
  entry: Pick<WorkspaceManifestEntry, "path" | "metadata">,
): string {
  const metadata = metadataObject(entry.metadata);
  const kind = metadata.kind ?? metadata.brunn_kind;
  if (typeof kind === "string") return kind;
  if (entry.path.startsWith(".brunn/checkpoints/")) return "checkpoint";
  if (entry.path.startsWith(".brunn/proposals/")) return "proposal";
  if (entry.path.startsWith(".brunn/binaries/")) {
    return "binary_description";
  }
  if (entry.path.startsWith("Briefings/Topics/")) return "briefing_topic";
  if (entry.path.startsWith("Briefings/Requests/")) return "briefing_request";
  if (entry.path.startsWith("Briefings/")) return "briefing_edition";
  return "entry";
}

export function isProposalEntry(entry: WorkspaceManifestEntry): boolean {
  const kind = workspaceEntryKind(entry).toLowerCase();
  return (
    kind.includes("proposal") ||
    entry.path.startsWith(".brunn/proposals/") ||
    entry.path.startsWith("Inbox/Proposals/")
  );
}

export function workspaceManifestTimestamp(
  entry: Pick<WorkspaceManifestEntry, "created_at" | "updated_at">,
): string | undefined {
  return entry.updated_at ?? entry.created_at;
}

export function newOperationId(prefix: string): string {
  const suffix =
    globalThis.crypto?.randomUUID?.() ??
    `${Date.now()}_${Math.random().toString(16).slice(2)}`;
  return `${prefix}_${suffix}`;
}

export function parseNonEmptyLines(value: string): string[] {
  return value
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
}
