import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useApi } from "../lib/auth";
import { newOperationId } from "../lib/workspace";
import type { TaskUpdateData } from "../lib/types";
import { ErrorState } from "./StateViews";

export function TaskDeferralFeedback({ data }: { data?: TaskUpdateData }) {
  const api = useApi();
  const cache = useQueryClient();
  const undo = useMutation({
    mutationFn: () => {
      if (!data) throw new Error("No deferral to undo");
      return api.taskUpdate(data.task.task_ref, { expected_version: data.task.version,
        idempotency_key: newOperationId("undo_deferral"),
        operation: { type: "correct", field: "ready_at", value: data.previous_ready_at ?? null, source: "owner" } });
    },
    onSettled: async () => { await Promise.all([cache.invalidateQueries({ queryKey: ["task-candidates"] }), cache.invalidateQueries({ queryKey: ["task"] })]); },
  });
  if (data?.action !== "snooze") return null;
  return <div role="status">
    <p>{undo.isSuccess ? "Deferral undone." : data.deferral_warning ?? "Deferred. The underlying dates have not changed."}</p>
    {!undo.isSuccess ? <button className="button secondary" type="button" disabled={undo.isPending} onClick={() => undo.mutate()}>Undo deferral</button> : null}
    {undo.isError ? <ErrorState error={undo.error} title="Could not undo; the current task has been reloaded" /> : null}
  </div>;
}
