import type { JsonObject } from "./types";

export type TaskQuickAction =
  | "complete"
  | "snooze"
  | "confirm_hard"
  | "drop"
  | "reopen";

export function confirmTaskDeletion(title: string): boolean {
  return window.confirm(
    `Delete “${title}”?\n\nThis removes this task from active lists, not its history. You can restore it from All tasks → Deleted. Other recurring occurrences are unchanged.`,
  );
}

export function taskQuickOperation(action: TaskQuickAction): JsonObject {
  if (action === "complete") {
    return { type: "complete", source: "owner", completed_via: "web" };
  }
  if (action === "snooze") return { type: "snooze", source: "owner", days: 1 };
  if (action === "confirm_hard") return { type: "confirm_hard", source: "owner" };
  if (action === "drop") {
    return {
      type: "drop",
      source: "owner",
      reason: "Deleted from Web",
    };
  }
  return { type: "reopen", source: "owner" };
}
