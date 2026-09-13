import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { TaskCandidate, TaskDetail } from "../lib/types";
import { installApiMock, renderApp } from "./renderApp";
import { candidate, taskDetail } from "./taskFixtures";

describe("task timing experience", () => {
  it("keeps a serious fourth item visible, Next independent, Quick at three, and Today deduplicated", async () => {
    const urgent = Array.from({ length: 6 }, (_, n) => candidate(n + 1, { tier: 1, title: `Attention ${n + 1}`, must_show: n === 3 }));
    const next = Array.from({ length: 5 }, (_, n) => candidate(n + 1, { task_ref: `next-${n}`, tier: 5, title: `Next ${n}`, today_since: n === 0 ? "2026-09-12T00:00:00Z" : null }));
    const quick = Array.from({ length: 5 }, (_, n) => candidate(n + 1, { task_ref: `quick-${n}`, tier: 5, title: `Quick ${n}`, estimate_minutes: 3 }));
    installApiMock({ "GET /api/v1/workspace/tasks/candidates": (request: Request) => {
      const view = new URL(request.url).searchParams.get("view")!;
      const items: Record<string, TaskCandidate[]> = { urgent, available: next, today: [next[0]], quick };
      return { status: "complete", data: { view, items: items[view] ?? [], urgent_total: 6, next_remaining: 0, backlog_total: 20 } };
    } });
    const user = userEvent.setup();
    renderApp("/dashboard");
    const attention = await screen.findByRole("region", { name: "Needs attention" });
    const nextRegion = screen.getByRole("region", { name: "Next tasks" });
    const quickRegion = screen.getByRole("region", { name: "Quick tasks" });
    expect(within(attention).getAllByTestId("task-row")).toHaveLength(4);
    expect(within(attention).getByText("Attention 4")).toBeInTheDocument();
    expect(within(nextRegion).getAllByTestId("task-row")).toHaveLength(5);
    expect(within(nextRegion).getByText("Today")).toBeInTheDocument();
    expect(screen.getAllByRole("link", { name: "Next 0" })).toHaveLength(1);
    expect(within(quickRegion).getAllByTestId("task-row")).toHaveLength(3);
    expect(nextRegion.compareDocumentPosition(quickRegion) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    await user.click(within(attention).getByRole("button", { name: "Show all 6" }));
    expect(within(attention).getAllByTestId("task-row")).toHaveLength(6);
    await user.click(within(quickRegion).getByRole("button", { name: "More quick tasks" }));
    expect(within(quickRegion).getAllByTestId("task-row")).toHaveLength(5);
  });

  it("opens the timing view without hiding deferred or unknown records behind an active-queue filter", async () => {
    const requests: URL[] = [];
    installApiMock({ "GET /api/v1/workspace/tasks/candidates": (request: Request) => {
      const url = new URL(request.url); requests.push(url);
      return { status: "complete", data: { view: "timing", items: [candidate(3, { title: "Care timing to clarify", ready_at: "2099-01-01T08:00:00Z", timing: { needs_attention: false, serious: false, timing_unknown: true, risk_on: null, due_on: null, reason: "Timing needs clarification" } })], next_cursor: null } };
    } });
    renderApp("/tasks?timing=true");
    expect(await screen.findByRole("link", { name: "Care timing to clarify" })).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: /Timing-sensitive/ })).toBeChecked();
    expect(screen.getByRole("combobox", { name: "Status" })).toBeDisabled();
    expect(requests.at(-1)?.searchParams.get("view")).toBe("timing");
    expect(requests.at(-1)?.searchParams.has("status")).toBe(false);
  });

  it("saves a sourced consequence without inventing timing, then defers and undoes using the returned version", async () => {
    let current: TaskDetail = structuredClone(taskDetail.task);
    const writes: Array<{ expected_version: number; operation: Record<string, unknown> }> = [];
    installApiMock({
      [`GET /api/v1/workspace/tasks/${current.task_ref}`]: () => ({ status: "complete", data: { task: current } }),
      [`PATCH /api/v1/workspace/tasks/${current.task_ref}`]: async (request: Request) => {
        const input = await request.json(); writes.push(input);
        current = { ...current, version: current.version + 1,
          task: input.operation.type === "correct" ? { ...current.task,
            [input.operation.field]: { value: input.operation.value, source: "owner", set_at: "2026-09-12T20:00:00Z" } } : current.task };
        return { status: "committed", data: { task: current, action: input.operation.type, previous_ready_at: null, deferral_warning: "Tomorrow is after the real cutoff." } };
      },
    });
    const user = userEvent.setup();
    renderApp(`/tasks/${current.task_ref}`);
    await screen.findByRole("heading", { name: current.title });
    await user.click(screen.getByText("Timing and recurrence"));
    await user.type(screen.getByLabelText("What happens if it slips?"), "Loss of plants");
    await user.selectOptions(screen.getByLabelText("Consequence"), "serious");
    await user.click(screen.getByRole("button", { name: "Save consequence" }));
    await waitFor(() => expect(writes).toHaveLength(1));
    expect(writes[0]).toMatchObject({ expected_version: 3, operation: { type: "correct", field: "consequence", source: "owner", value: { description: "Loss of plants", severity: "serious" } } });
    await screen.findAllByText("Serious · Loss of plants · Risk timing not set");
    await waitFor(() => expect(screen.getByRole("button", { name: "Tomorrow" })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: "Tomorrow" }));
    const undo = await screen.findByRole("button", { name: "Undo deferral" });
    expect(writes[1]).toMatchObject({ expected_version: 4, operation: { type: "snooze", tomorrow: true } });
    expect(screen.getByText("Tomorrow is after the real cutoff.")).toBeInTheDocument();
    await user.click(undo);
    await screen.findByText("Deferral undone.");
    expect(writes[2]).toMatchObject({ expected_version: 5, operation: { type: "correct", field: "ready_at", value: null, source: "owner" } });
  });

  it("uses a local date/time picker for the actual cutoff", async () => {
    let operation: unknown;
    installApiMock({
      [`GET /api/v1/workspace/tasks/${taskDetail.task.task_ref}`]: { status: "complete", data: taskDetail },
      [`PATCH /api/v1/workspace/tasks/${taskDetail.task.task_ref}`]: async (request: Request) => {
      operation = (await request.json()).operation;
      return { status: "committed", data: { task: taskDetail.task, action: "correct" } };
    } });
    const user = userEvent.setup();
    renderApp(`/tasks/${taskDetail.task.task_ref}`);
    await user.click(await screen.findByText("Timing and recurrence"));
    fireEvent.change(screen.getByLabelText("Actual cutoff (your local time)"), { target: { value: "2026-09-15T17:30" } });
    await user.click(screen.getByRole("button", { name: "Save cutoff" }));
    await waitFor(() => expect(operation).toEqual({ type: "correct", field: "hard_due", source: "owner", value: new Date("2026-09-15T17:30").toISOString() }));
  });
});
