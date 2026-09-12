import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TaskDetail } from "../lib/types";
import { defaultMe, installApiMock, renderApp } from "./renderApp";
import {
  candidate,
  doneToday,
  nextCandidates,
  taskDetail,
  taskProjectState,
  taskProjects,
  todoistStatus,
  urgentCandidates,
} from "./taskFixtures";

describe("agent-first task surfaces", () => {
  beforeEach(() => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
  });
  afterEach(() => vi.mocked(window.confirm).mockRestore());
  it("keeps the dashboard bounded, explains ranking, and expands only explicitly", async () => {
    const requests: URL[] = [];
    installApiMock({
      "GET /api/v1/workspace/tasks/candidates": (request: Request) => {
        const url = new URL(request.url);
        requests.push(url);
        const view = url.searchParams.get("view");
        const limit = Number(url.searchParams.get("limit") ?? "5");
        const items =
          view === "urgent" ? urgentCandidates.slice(0, 3) : nextCandidates;
        return {
          status: "complete",
          data: {
            view,
            as_of: "2026-08-27T11:00:00Z",
            contexts_available: url.searchParams.getAll("contexts_available"),
            items: items.slice(0, limit),
            urgent_total: view === "urgent" ? items.length : urgentCandidates.length,
            next_remaining: Math.max(0, items.length - limit),
            backlog_total: 32,
            next_cursor: null,
          },
        };
      },
      "GET /api/v1/workspace/tasks/done-summary": {
        status: "complete",
        data: {
          ...doneToday,
          count: 8,
          done_today_count: 8,
          items: Array.from({ length: 8 }, (_, index) => ({
            ...doneToday.items[0],
            task_ref: `019d3f10-000${index}-7000-8000-00000000000${index}`,
            title: `Done task ${index + 1}`,
          })),
        },
      },
      "GET /api/v1/workspace/projects": {
        status: "complete",
        data: {
          ...taskProjects,
          projects: Array.from({ length: 8 }, (_, index) => ({
            ...taskProjects.projects[0],
            slug: `project-${index + 1}`,
            title: `Project ${index + 1}`,
          })),
        },
      },
    });
    const user = userEvent.setup();
    renderApp("/dashboard");

    expect(
      within(
        await screen.findByRole("navigation", { name: "Primary navigation" }),
      ).queryByRole("link", { name: "Tasks" }),
    ).not.toBeInTheDocument();
    const urgent = await screen.findByRole("region", { name: "Urgent tasks" });
    const next = screen.getByRole("region", { name: "Next tasks" });
    expect(within(urgent).getAllByTestId("task-row")).toHaveLength(3);
    expect(within(next).getAllByTestId("task-row")).toHaveLength(2);
    expect([
      ...within(urgent).getAllByTestId("task-row"),
      ...within(next).getAllByTestId("task-row"),
    ]).toHaveLength(5);
    const doneRegion = screen.getByRole("region", { name: "Done today" });
    expect(within(doneRegion).getByText("8")).toBeInTheDocument();
    expect(
      within(doneRegion).queryByRole("listitem"),
    ).not.toBeInTheDocument();
    const projectLinks = within(
      screen.getByRole("region", { name: "Task projects" }),
    ).getAllByRole("link");
    expect(projectLinks).toHaveLength(5);
    expect(within(projectLinks[0]).getByRole("img", { name: "Status green" })).toHaveClass(
      "status-green",
    );
    expect(within(next).queryByText("Next task 6")).not.toBeInTheDocument();
    expect(
      within(urgent).getAllByText("hard deadline in 2 days (est.)").length,
    ).toBeGreaterThan(0);
    expect(
      within(urgent).getByLabelText("Inferred by agent:codex"),
    ).toBeInTheDocument();
    expect(screen.getByText("32 tasks held out of view")).toBeInTheDocument();

    await user.click(
      within(doneRegion).getByRole("button", { name: "Show completed tasks" }),
    );
    expect(within(doneRegion).getAllByRole("listitem")).toHaveLength(5);

    await user.click(within(next).getByRole("button", { name: "5 more" }));
    expect(await within(next).findByText("Next task 10")).toBeInTheDocument();
    expect(within(next).getAllByTestId("task-row")).toHaveLength(7);
    expect([
      ...within(urgent).getAllByTestId("task-row"),
      ...within(next).getAllByTestId("task-row"),
    ]).toHaveLength(10);
    expect(
      requests.some(
        (url) =>
          url.searchParams.get("view") === "next" &&
          url.searchParams.get("limit") === "10" &&
          url.searchParams.getAll("contexts_available").includes("online"),
      ),
    ).toBe(true);
    expect(within(next).getByRole("link", { name: "Show all" })).toHaveAttribute(
      "href",
      "/tasks",
    );
  });

  it("allows at most two pinned rows above the default five-task union", async () => {
    const pinned = Array.from({ length: 3 }, (_, index) =>
      candidate(index + 20, {
        title: `Pinned task ${index + 1}`,
        pinned: true,
      }),
    );
    installApiMock({
      "GET /api/v1/workspace/tasks/candidates": (request: Request) => {
        const view = new URL(request.url).searchParams.get("view");
        const items = view === "urgent" ? pinned : nextCandidates;
        return {
          status: "complete",
          data: {
            view,
            as_of: "2026-08-27T11:00:00Z",
            contexts_available: ["online"],
            items,
            urgent_total: pinned.length,
            next_remaining: 5,
            backlog_total: 13,
            next_cursor: null,
          },
        };
      },
    });
    renderApp("/dashboard");

    const urgent = await screen.findByRole("region", { name: "Urgent tasks" });
    const next = screen.getByRole("region", { name: "Next tasks" });
    expect(within(urgent).getAllByTestId("task-row")).toHaveLength(2);
    expect(within(next).getAllByTestId("task-row")).toHaveLength(5);
    expect([
      ...within(urgent).getAllByTestId("task-row"),
      ...within(next).getAllByTestId("task-row"),
    ]).toHaveLength(7);
  });

  it("renders the urgent empty state without an empty Urgent card", async () => {
    installApiMock({
      "GET /api/v1/workspace/tasks/candidates": (request: Request) => {
        const view = new URL(request.url).searchParams.get("view");
        return {
          status: "complete",
          data: {
            view,
            as_of: "2026-08-27T11:00:00Z",
            contexts_available: ["online"],
            items: view === "urgent" ? [] : nextCandidates.slice(0, 5),
            urgent_total: 0,
            next_remaining: 5,
            backlog_total: 10,
            next_cursor: null,
          },
        };
      },
    });
    renderApp("/dashboard");

    expect(await screen.findByText("Nothing urgent")).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Urgent tasks" })).not.toBeInTheDocument();
  });

  it("captures one raw sentence without pretending the Web enriched it", async () => {
    let captureBody: unknown;
    installApiMock({
      "POST /api/v1/workspace/tasks/capture": async (request: Request) => {
        captureBody = await request.json();
        return {
          status: "committed",
          data: {
            items: [
              {
                task_ref: candidate(20).task_ref,
                entry_ref: candidate(20).entry_ref,
                version: 1,
                title: "Call the dealer tomorrow",
                enrichment: {},
                context_suggestions: [],
              },
            ],
            suggested_existing: [],
            replayed: false,
          },
        };
      },
    });
    const user = userEvent.setup();
    renderApp("/dashboard");
    const input = await screen.findByRole("textbox", { name: "Capture a task" });

    await user.type(input, "Call the dealer tomorrow");
    await user.click(screen.getByRole("button", { name: "Capture" }));

    await waitFor(() => expect(input).toHaveValue(""));
    expect(captureBody).toEqual({
      idempotency_key: expect.stringMatching(/^web_task_capture_/),
      items: [
        {
          raw_text: "Call the dealer tomorrow",
          captured_from: "web:dashboard",
        },
      ],
    });
  });

  it("performs active-row actions only with task.write", async () => {
    const bodies: unknown[] = [];
    installApiMock({
      [`PATCH /api/v1/workspace/tasks/${nextCandidates[0].task_ref}`]: async (
        request: Request,
      ) => {
        const body = await request.json();
        bodies.push(body);
        return {
          status: "committed",
          data: {
            ...taskDetail,
            action: (body as { operation: { type: string } }).operation.type,
            correction_ref: null,
            done_today_count: 3,
            replayed: false,
          },
        };
      },
    });
    const user = userEvent.setup();
    renderApp("/dashboard");
    const urgentRegion = await screen.findByRole("region", { name: "Urgent tasks" });
    const row = await within(urgentRegion).findByTestId(
      `task-row-${nextCandidates[0].task_ref}`,
    );

    await user.click(within(row).getByRole("button", { name: "Complete" }));
    await user.click(within(row).getByRole("button", { name: "Snooze one day" }));
    await user.click(within(row).getByRole("button", { name: "Confirm hard deadline" }));
    await user.click(within(row).getByRole("button", { name: "Delete task" }));
    await waitFor(() => expect(bodies).toHaveLength(4));
    expect(bodies).toEqual([
      expect.objectContaining({
        expected_version: 1,
        operation: { type: "complete", source: "owner", completed_via: "web" },
      }),
      expect.objectContaining({
        expected_version: 1,
        operation: { type: "snooze", source: "owner", days: 1 },
      }),
      expect.objectContaining({
        expected_version: 1,
        operation: { type: "confirm_hard", source: "owner" },
      }),
      expect.objectContaining({
        expected_version: 1,
        operation: {
          type: "drop",
          source: "owner",
          reason: "Deleted from Web",
        },
      }),
    ]);

    const noTaskWrite = structuredClone(defaultMe);
    noTaskWrite.data.capabilities = noTaskWrite.data.capabilities.filter(
      (capability) => capability !== "task.write",
    );
    installApiMock({ "GET /api/v1/me": noTaskWrite });
    const viewOnlyApp = renderApp("/dashboard");
    const viewOnlyRow = (await within(viewOnlyApp.container).findAllByTestId(
      `task-row-${nextCandidates[0].task_ref}`,
    )).at(-1)!;
    expect(within(viewOnlyRow).queryByRole("button", { name: "Complete" })).not.toBeInTheDocument();
    expect(within(viewOnlyRow).queryByRole("button", { name: "Delete task" })).not.toBeInTheDocument();
    expect(within(viewOnlyRow).getByText("View only")).toBeInTheDocument();
  });

  it("cancels deletion, retains a failed task, then removes it from active lists after success", async () => {
    let deleted = false;
    let attempts = 0;
    const item = nextCandidates[0];
    installApiMock({
      "GET /api/v1/workspace/tasks/candidates": (request: Request) => ({
        status: "complete",
        data: {
          view: new URL(request.url).searchParams.get("view"),
          items: deleted ? [] : [item], urgent_total: 1, next_remaining: 0, backlog_total: deleted ? 0 : 1,
        },
      }),
      [`PATCH /api/v1/workspace/tasks/${item.task_ref}`]: () => {
        attempts += 1;
        if (attempts === 1) return { status: 503, body: { error: { code: "unavailable", message: "Please try again" } } };
        deleted = true;
        return { status: "committed", data: { task: { ...taskDetail.task, status: "dropped" }, action: "drop", replayed: false } };
      },
    });
    const user = userEvent.setup();
    renderApp("/dashboard");
    const row = await screen.findByTestId(`task-row-${item.task_ref}`);
    vi.mocked(window.confirm).mockReturnValueOnce(false);
    await user.click(within(row).getByRole("button", { name: "Delete task" }));
    expect(attempts).toBe(0);
    expect(row).toBeInTheDocument();
    expect(window.confirm).toHaveBeenCalledWith(expect.stringContaining(item.title));
    await user.click(within(row).getByRole("button", { name: "Delete task" }));
    expect(await screen.findByText("Please try again")).toBeInTheDocument();
    expect(row).toBeInTheDocument();
    await user.click(within(row).getByRole("button", { name: "Delete task" }));
    await waitFor(() => expect(screen.queryByTestId(`task-row-${item.task_ref}`)).not.toBeInTheDocument());
    expect(screen.getByText("Task deleted")).toBeInTheDocument();
    expect(attempts).toBe(2);
  });

  it("reloads a conflicting detail version before retry and restores deleted history", async () => {
    let current: TaskDetail = structuredClone(taskDetail.task);
    const operations: Array<{ expected_version: number; operation: { type: string } }> = [];
    installApiMock({
      [`GET /api/v1/workspace/tasks/${current.task_ref}`]: () => ({ status: "complete", data: { task: current } }),
      [`PATCH /api/v1/workspace/tasks/${current.task_ref}`]: async (request: Request) => {
        const body = await request.json();
        operations.push(body);
        if (operations.length === 1) {
          current = { ...current, version: 4, title: "Updated elsewhere" };
          return { status: 409, body: { error: { code: "version_conflict", message: "Task changed elsewhere" } } };
        }
        current = { ...current, version: current.version + 1, status: body.operation.type === "drop" ? "dropped" : "open" };
        return { status: "committed", data: { task: current, action: body.operation.type, replayed: false } };
      },
    });
    const user = userEvent.setup();
    renderApp(`/tasks/${current.task_ref}`);
    const deleteButton = await screen.findByRole("button", { name: "Delete task" });
    vi.mocked(window.confirm).mockReturnValueOnce(false);
    await user.click(deleteButton);
    expect(operations).toHaveLength(0);
    await user.click(deleteButton);
    expect(await screen.findByText("Task changed elsewhere")).toBeInTheDocument();
    await screen.findByRole("heading", { name: "Updated elsewhere" });
    await user.click(deleteButton);
    const restore = await screen.findByRole("button", { name: "Restore task" });
    expect(screen.queryByRole("button", { name: "Delete task" })).not.toBeInTheDocument();
    expect(screen.getAllByText("Deleted").length).toBeGreaterThan(0);
    await user.click(restore);
    expect(await screen.findByRole("button", { name: "Delete task" })).toBeInTheDocument();
    expect(operations.map((item) => [item.expected_version, item.operation.type])).toEqual([[3, "drop"], [4, "drop"], [5, "reopen"]]);
  });

  it("opens the deliberate paginated list and applies every supported filter", async () => {
    const seen: URL[] = [];
    installApiMock({
      "GET /api/v1/workspace/tasks/candidates": (request: Request) => {
        const url = new URL(request.url);
        seen.push(url);
        const cursor = url.searchParams.get("cursor");
        let items = cursor
          ? [
              candidate(8, {
                title: "Second page phone task",
                status: "waiting",
                required_contexts: ["phone"],
                provenance_markers: ["todoist"],
              }),
            ]
          : [
              candidate(1, { title: "Owner hard task", tier: 1 }),
              candidate(2, {
                title: "Todoist waiting task",
                status: "waiting",
                tier: 3,
                provenance_markers: ["todoist"],
                required_contexts: ["phone"],
              }),
            ];
        if (
          !cursor &&
          (url.searchParams.get("status") === "waiting" ||
            url.searchParams.get("source") === "todoist")
        ) {
          items = items.filter((item) => item.status === "waiting");
        }
        return {
          status: "complete",
          data: {
            view: "all",
            as_of: "2026-08-27T11:00:00Z",
            contexts_available: [],
            items,
            urgent_total: 1,
            next_remaining: cursor ? 0 : 1,
            backlog_total: 3,
            next_cursor: cursor ? null : candidate(2).task_ref,
            filters: {
              status: url.searchParams.get("status"),
              project: url.searchParams.get("project"),
              context: url.searchParams.get("context"),
              date_type: url.searchParams.get("date_type"),
              source: url.searchParams.get("source"),
              include_waiting: url.searchParams.get("include_waiting") === "true",
              include_parked: url.searchParams.get("include_parked") === "true",
            },
          },
        };
      },
    });
    const user = userEvent.setup();
    renderApp("/tasks");

    expect(await screen.findByRole("heading", { name: "All tasks" })).toBeInTheDocument();
    expect(
      within(screen.getByLabelText("Status")).getAllByRole("option").map(
        (option) => option.getAttribute("value"),
      ),
    ).toEqual(["all", "open", "waiting", "done", "dropped"]);
    expect(
      within(screen.getByLabelText("Date type")).getAllByRole("option").map(
        (option) => option.getAttribute("value"),
      ),
    ).toEqual(["all", "hard", "cost", "soft", "none"]);
    expect(
      within(screen.getByLabelText("Source")).getAllByRole("option").map(
        (option) => option.getAttribute("value"),
      ),
    ).toEqual(["all", "owner", "agent", "derived", "todoist"]);
    expect(
      screen.queryByText(/source and date filters apply to this server page/i),
    ).not.toBeInTheDocument();
    await user.selectOptions(screen.getByLabelText("Status"), "waiting");
    await user.selectOptions(screen.getByLabelText("Project"), "brunn");
    await user.selectOptions(screen.getByLabelText("Context"), "phone");
    await user.selectOptions(screen.getByLabelText("Date type"), "soft");
    await user.selectOptions(screen.getByLabelText("Source"), "todoist");
    await user.click(screen.getByRole("checkbox", { name: "Include parked" }));
    await user.click(screen.getByRole("checkbox", { name: "Include waiting" }));
    expect(await screen.findByText("Todoist waiting task")).toBeInTheDocument();
    expect(screen.queryByText("Owner hard task")).not.toBeInTheDocument();
    expect(screen.getByText("Page 1 · 3 matching tasks · 1 after this page")).toBeInTheDocument();

    await user.selectOptions(screen.getByLabelText("Date type"), "all");
    await user.click(screen.getByRole("button", { name: "Next page" }));
    expect(await screen.findByText("Second page phone task")).toBeInTheDocument();
    expect(
      seen.some(
        (url) =>
          url.searchParams.get("view") === "all" &&
          url.searchParams.get("deliberate_all") === "true" &&
          url.searchParams.get("include_waiting") === "true" &&
          url.searchParams.get("include_parked") === "true" &&
          url.searchParams.get("project") === "brunn" &&
          url.searchParams.get("context") === "phone" &&
          url.searchParams.get("status") === "waiting" &&
          url.searchParams.get("date_type") === "soft" &&
          url.searchParams.get("source") === "todoist",
      ),
    ).toBe(true);
    expect(
      seen.some(
        (url) =>
          url.searchParams.get("status") === "all" &&
          url.searchParams.get("date_type") === "all" &&
          url.searchParams.get("source") === "all",
      ),
    ).toBe(true);
  });

  it("exposes only status-valid actions for done and dropped rows", async () => {
    const done = candidate(40, {
      title: "Completed terminal task",
      status: "done",
      reason: "completed Aug 27",
    });
    const dropped = candidate(41, {
      title: "Dropped terminal task",
      status: "dropped",
      reason: "dropped Aug 27",
    });
    const bodies: Array<{ taskRef: string; body: unknown }> = [];
    const recordAction = (taskRef: string) => async (request: Request) => {
      const body = await request.json();
      bodies.push({ taskRef, body });
      return {
        status: "committed",
        data: {
          task: { ...taskDetail.task, task_ref: taskRef },
          action: (body as { operation: { type: string } }).operation.type,
          replayed: false,
        },
      };
    };
    installApiMock({
      "GET /api/v1/workspace/tasks/candidates": {
        status: "complete",
        data: {
          view: "all",
          as_of: "2026-08-27T11:00:00Z",
          contexts_available: [],
          items: [done, dropped],
          urgent_total: 0,
          next_remaining: 0,
          backlog_total: 2,
          next_cursor: null,
          filters: {
            status: "all",
            project: null,
            context: null,
            date_type: "all",
            source: "all",
            include_waiting: false,
            include_parked: false,
          },
        },
      },
      [`PATCH /api/v1/workspace/tasks/${done.task_ref}`]: recordAction(done.task_ref),
      [`PATCH /api/v1/workspace/tasks/${dropped.task_ref}`]: recordAction(dropped.task_ref),
    });
    const user = userEvent.setup();
    renderApp("/tasks");

    const doneRow = await screen.findByTestId(`task-row-${done.task_ref}`);
    const droppedRow = screen.getByTestId(`task-row-${dropped.task_ref}`);
    expect(within(doneRow).getByRole("button", { name: "Reopen" })).toBeInTheDocument();
    expect(within(doneRow).getByRole("button", { name: "Delete task" })).toBeInTheDocument();
    expect(within(doneRow).queryByRole("button", { name: "Complete" })).not.toBeInTheDocument();
    expect(within(doneRow).queryByRole("button", { name: "Snooze one day" })).not.toBeInTheDocument();
    expect(within(droppedRow).getByRole("button", { name: "Restore task" })).toBeInTheDocument();
    expect(within(droppedRow).queryByRole("button", { name: "Delete task" })).not.toBeInTheDocument();

    await user.click(within(doneRow).getByRole("button", { name: "Delete task" }));
    await waitFor(() => expect(bodies).toHaveLength(1));
    await user.click(within(droppedRow).getByRole("button", { name: "Restore task" }));
    await waitFor(() => expect(bodies).toHaveLength(2));
    expect(bodies).toEqual([
      {
        taskRef: done.task_ref,
        body: expect.objectContaining({
          operation: {
            type: "drop",
            source: "owner",
            reason: "Deleted from Web",
          },
        }),
      },
      {
        taskRef: dropped.task_ref,
        body: expect.objectContaining({
          operation: { type: "reopen", source: "owner" },
        }),
      },
    ]);
  });

  it("shows task detail, project checkpoint state, and exact provenance", async () => {
    let dropBody: unknown;
    installApiMock({
      [`GET /api/v1/workspace/tasks/${nextCandidates[0].task_ref}`]: {
        status: "complete",
        data: {
          task: { ...taskDetail.task, status: "done" },
        },
      },
      [`PATCH /api/v1/workspace/tasks/${nextCandidates[0].task_ref}`]: async (
        request: Request,
      ) => {
        dropBody = await request.json();
        return {
          status: "committed",
          data: { task: taskDetail.task, action: "drop", replayed: false },
        };
      },
      "GET /api/v1/workspace/projects/brunn/state": {
        status: "complete",
        data: taskProjectState,
      },
    });
    const user = userEvent.setup();
    renderApp(`/tasks/${nextCandidates[0].task_ref}`);
    expect(await screen.findByRole("heading", { name: "Next task 1" })).toBeInTheDocument();
    expect(screen.getByText("inferred from renewal language")).toBeInTheDocument();
    expect(screen.getAllByText("agent:codex").length).toBeGreaterThan(0);
    expect(screen.getByRole("button", { name: "Reopen" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Snooze tomorrow" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Delete task" }));
    await waitFor(() =>
      expect(dropBody).toEqual(
        expect.objectContaining({
          expected_version: 3,
          operation: {
            type: "drop",
            source: "owner",
            reason: "Deleted from Web",
          },
        }),
      ),
    );

    renderApp("/projects/brunn");
    expect(await screen.findByRole("heading", { name: "Brunn" })).toBeInTheDocument();
    expect(screen.getByText("No checkpoint in 9 days · as of 2026-08-20")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "Status yellow" })).toHaveClass("status-yellow");
    expect(screen.getByText("Ship agent-first tasks")).toBeInTheDocument();
    expect(screen.getByText("Finish gate 12c")).toBeInTheDocument();
    expect(screen.getByText("Wait for review")).toBeInTheDocument();
  });
});

describe("task settings and trust boundaries", () => {
  it("configures Todoist from its own section with dual truth", async () => {
    let configureBody: unknown;
    installApiMock({
      "PUT /api/v1/workspace/integrations/todoist/config": async (request: Request) => {
        configureBody = await request.json();
        return { status: "committed", data: { replayed: false } };
      },
    });
    const user = userEvent.setup();
    renderApp("/settings");

    expect(await screen.findByRole("heading", { name: "Todoist" })).toBeInTheDocument();
    for (const heading of ["Contexts", "Readiness engine", "Task operations"]) {
      expect(screen.queryByRole("heading", { name: heading })).not.toBeInTheDocument();
    }
    expect(screen.getByText("Saved mode").parentElement).toHaveTextContent("Pull");
    expect(screen.getByText("Effective mode").parentElement).toHaveTextContent("Off");
    expect(screen.getByText("Environment kill switch is off")).toBeInTheDocument();
    expect(screen.queryByText(/api token/i)).not.toBeInTheDocument();
    await user.selectOptions(screen.getByLabelText("Todoist mode"), "off");
    await user.click(screen.getByRole("button", { name: "Save Todoist mode" }));
    expect(screen.getByRole("button", { name: "Pull now" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Pull now" })).toHaveAttribute(
      "title",
      "Enable the environment gate, credential, and a pull mode first",
    );

    await waitFor(() =>
      expect(configureBody).toEqual(
        expect.objectContaining({ expected_generation: 3, mode: "off" }),
      ),
    );
  });

  it("queues a manual Todoist pull only when the effective inlet is enabled", async () => {
    let pullBody: unknown;
    installApiMock({
      "GET /api/v1/workspace/integrations/todoist/status": {
        status: "complete",
        data: {
          ...todoistStatus,
          environment_enabled: true,
          effective_mode: "pull",
        },
      },
      "POST /api/v1/workspace/integrations/todoist/pull": async (request: Request) => {
        pullBody = await request.json();
        return { status: "committed", data: { queued: true, replayed: false } };
      },
    });
    const user = userEvent.setup();
    renderApp("/settings");

    await user.click(await screen.findByRole("button", { name: "Pull now" }));
    await waitFor(() =>
      expect(pullBody).toEqual(
        expect.objectContaining({ idempotency_key: expect.stringMatching(/^web_todoist_pull_/) }),
      ),
    );
  });

  it("keeps Todoist configuration behind integration.manage", async () => {
    const me = structuredClone(defaultMe);
    me.data.capabilities = me.data.capabilities.filter(
      (capability) => capability !== "integration.manage",
    );
    installApiMock({ "GET /api/v1/me": me });
    renderApp("/settings");

    expect(
      await screen.findByText("Todoist configuration is owner-only"),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save Todoist mode" })).not.toBeInTheDocument();
  });
});
