import { useState } from "react";
import type { JsonObject, JsonValue, TaskDetail } from "../lib/types";

function cell(task: TaskDetail, field: string): JsonValue | undefined {
  const value = task.task[field];
  return value && typeof value === "object" && !Array.isArray(value) ? value.value : undefined;
}
function object(value: JsonValue | undefined): JsonObject {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}
function localDateTime(value: JsonValue | undefined): string {
  if (typeof value !== "string") return "";
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "";
  const pad = (part: number) => String(part).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** Independent field saves keep owner changes explicit and use the same CAS as
 * other corrections. Mount with task_ref:version so remote corrections reload. */
export function TaskTimingEditor({ task, pending, save }: {
  task: TaskDetail; pending: boolean; save: (operation: JsonObject) => void;
}) {
  const consequence = object(cell(task, "consequence"));
  const timing = object(consequence.timing);
  const recurrence = object(cell(task, "recurrence"));
  const [softDue, setSoftDue] = useState(String(cell(task, "soft_due") ?? ""));
  const [hardDue, setHardDue] = useState(() => localDateTime(cell(task, "hard_due")));
  const [description, setDescription] = useState(String(consequence.description ?? ""));
  const [severity, setSeverity] = useState(String(consequence.severity ?? "ordinary"));
  const [riskKind, setRiskKind] = useState(String(timing.kind ?? "unknown"));
  const [riskStart, setRiskStart] = useState(String(timing.starts_on ?? ""));
  const [riskEnd, setRiskEnd] = useState(String(timing.ends_on ?? ""));
  const [tolerance, setTolerance] = useState(Number(timing.days ?? 0));
  const [mode, setMode] = useState(String(recurrence.kind === "native" ? recurrence.mode : "none"));
  const [days, setDays] = useState(Number(recurrence.every_days ?? 0));
  const [zone, setZone] = useState(String(recurrence.timezone ?? Intl.DateTimeFormat().resolvedOptions().timeZone));
  const [anchor, setAnchor] = useState(String(recurrence.anchor_on ?? ""));
  const correct = (field: string, value: JsonValue) => save({ type: "correct", source: "owner", field, value });
  return <details className="task-card">
    <summary>Timing and recurrence</summary>
    <p>Choose an intended date, a real cutoff, or a consequence. Unknown timing does not schedule a reminder. Saving a field preserves the others.</p>
    <form className="task-correction-form" onSubmit={event => { event.preventDefault(); correct("soft_due", softDue || null); }}>
      <label>Intended date<input type="date" value={softDue} onChange={e => setSoftDue(e.target.value)} /></label>
      <button className="button secondary" disabled={pending}>Save intended date</button>
    </form>
    <form className="task-correction-form" onSubmit={event => { event.preventDefault(); correct("hard_due", hardDue ? new Date(hardDue).toISOString() : null); }}>
      <label>Actual cutoff (your local time)<input type="datetime-local" value={hardDue} onChange={e => setHardDue(e.target.value)} /></label>
      <button className="button secondary" disabled={pending}>Save cutoff</button>
    </form>
    <form className="task-correction-form" onSubmit={event => {
      event.preventDefault();
      const value: JsonObject = { description: description.trim(), severity };
      if (riskKind === "window") value.timing = { kind: "window", starts_on: riskStart, ends_on: riskEnd || null };
      if (riskKind === "after_due") value.timing = { kind: "after_due", days: tolerance };
      correct("consequence", description.trim() ? value : null);
    }}>
      <label>What happens if it slips?<input maxLength={1000} value={description} onChange={e => setDescription(e.target.value)} /></label>
      <label>Consequence<select value={severity} onChange={e => setSeverity(e.target.value)}><option value="ordinary">Ordinary</option><option value="serious">Serious loss or harm</option></select></label>
      <label>When does risk begin?<select value={riskKind} onChange={e => setRiskKind(e.target.value)}><option value="unknown">Not known yet</option><option value="window">Known date or window</option><option value="after_due">Days after intended date</option></select></label>
      {riskKind === "window" ? <><label>Window starts<input type="date" required value={riskStart} onChange={e => setRiskStart(e.target.value)} /></label><label>Window ends (optional)<input type="date" min={riskStart} value={riskEnd} onChange={e => setRiskEnd(e.target.value)} /></label></> : null}
      {riskKind === "after_due" ? <label>Supported tolerance in days<input type="number" min={0} max={3650} value={tolerance} onChange={e => setTolerance(Number(e.target.value))} /></label> : null}
      <button className="button secondary" disabled={pending}>Save consequence</button>
    </form>
    {recurrence.kind !== undefined && recurrence.kind !== "native" ? <p>Imported recurrence is retained. Use the agent to review that rule before replacing it.</p> : <form className="task-correction-form" onSubmit={event => {
      event.preventDefault();
      const value: JsonObject = { kind: "native", mode, every_days: days, timezone: zone };
      if (mode === "calendar") value.anchor_on = anchor;
      correct("recurrence", mode === "none" ? null : value);
    }}>
      <label>Repeat<select value={mode} onChange={e => setMode(e.target.value)}><option value="none">Does not repeat</option><option value="after_completion">After actual completion</option><option value="calendar">Fixed calendar</option></select></label>
      {mode !== "none" ? <><label>Every (days)<input type="number" min={1} max={3650} value={days} onChange={e => setDays(Number(e.target.value))} /></label><label>Timezone<input required value={zone} onChange={e => setZone(e.target.value)} /></label>{mode === "calendar" ? <label>Calendar anchor<input type="date" required value={anchor} onChange={e => setAnchor(e.target.value)} /></label> : null}</> : null}
      <p>Current due date is the intended date above. Completing creates the next occurrence; Tomorrow does not.</p>
      <button className="button secondary" disabled={pending}>Save recurrence</button>
    </form>}
  </details>;
}
