import type {
  TaskCandidate,
  TaskContextListData,
  TaskDoneSummaryData,
  TaskGuardStatusData,
  TaskProjectListData,
  TaskSettingsData,
  TodoistStatusData,
} from "../lib/types";

const now = "2026-08-27T11:00:00Z";

export function candidate(
  index: number,
  overrides: Partial<TaskCandidate> = {},
): TaskCandidate {
  return {
    task_ref: `019d3f00-000${index}-7000-8000-00000000000${index}`,
    entry_ref: `entry:019d3f00-000${index}-7000-8000-00000000000${index}`,
    version: 1,
    title: `Task ${index}`,
    status: "open",
    project: "straylight",
    required_contexts: ["online"],
    tier: index <= 2 ? index : 5,
    reason: index === 1 ? "hard deadline in 2 days (est.)" : "ready since Aug 20",
    provenance_markers: index === 1 ? ["agent:codex"] : [],
    pinned: false,
    ...overrides,
  };
}

export const urgentCandidates = Array.from({ length: 7 }, (_, index) =>
  candidate(index + 1, {
    tier: index % 2 === 0 ? 1 : 2,
    title: `Urgent task ${index + 1}`,
    reason:
      index % 2 === 0
        ? "hard deadline in 2 days (est.)"
        : "~$12/day since Aug 12, ~$180 so far",
    provenance_markers: index === 0 ? ["agent:codex"] : [],
  }),
);

export const nextCandidates = Array.from({ length: 10 }, (_, index) =>
  candidate(index + 1, {
    title: `Next task ${index + 1}`,
    tier: index === 0 ? 1 : index === 1 ? 3 : 5,
    reason:
      index === 0
        ? "hard deadline in 2 days (est.)"
        : index === 1
          ? "should do by Fri"
          : "ready since Aug 20",
    provenance_markers: index === 0 ? ["agent:codex"] : [],
  }),
);

export const taskContexts: TaskContextListData = {
  contexts: [
    {
      slug: "online",
      display_name: "Online",
      aliases: ["web"],
      description: "Can be completed online",
      archived: false,
      created_by: "owner",
      version: 2,
      active_task_count: 4,
      created_at: "2026-05-01T00:00:00Z",
      updated_at: "2026-08-20T00:00:00Z",
    },
    {
      slug: "phone",
      display_name: "Phone",
      aliases: ["call"],
      description: "Needs a phone",
      archived: false,
      created_by: "agent:codex",
      version: 3,
      active_task_count: 0,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
    },
  ],
  surface_defaults: {
    web: {
      contexts_available: ["online"],
      version: 2,
      updated_at: "2026-08-20T00:00:00Z",
    },
  },
  next_cursor: null,
};

export const taskSettings: TaskSettingsData = {
  settings: {
    timezone: "America/Los_Angeles",
    hard_lead_days: 7,
    hard_second_lead_hours: 48,
    due_day_local_time: "07:00:00",
    soft_window_days: 3,
    triage_after_days: 7,
    waiting_followup_days: 7,
    quiet_hours_start: "22:00:00",
    quiet_hours_end: "07:00:00",
    quiet_override_enabled: true,
    quiet_override_within_hours: 24,
    surface_defaults: taskContexts.surface_defaults,
    version: 4,
    updated_at: now,
  },
};

export const todoistStatus: TodoistStatusData = {
  environment_enabled: false,
  saved_mode: "pull",
  effective_mode: "off",
  token_configured: true,
  configuration_generation: 3,
  last_run_at: "2026-08-27T10:55:00Z",
  last_outcome: "success",
  last_error_code: null,
  next_run_at: null,
};

export const taskGuardStatus: TaskGuardStatusData = {
  environment_enabled: true,
  effective_enabled: true,
  last_run_at: "2026-08-27T10:57:00Z",
  last_outcome: "complete",
  last_error_code: null,
  next_run_at: "2026-08-27T11:02:00Z",
};

export const taskProjects: TaskProjectListData = {
  projects: [
    {
      slug: "straylight",
      title: "Straylight",
      description: "Durable memory",
      aliases: ["memory"],
      hub_path: "sources/Projects/Straylight/Straylight.md",
      repo_path: "/Volumes/NyxFastData/dev/projects/straylight",
      interest: "hot",
      interest_override: "hot",
      interest_set_by: "owner",
      interest_set_at: "2026-08-26T00:00:00Z",
      last_activity_at: now,
      archived: false,
      open_task_count: 8,
      last_checkpoint_at: "2026-08-27T10:00:00Z",
      version: 2,
      created_by: "owner",
    },
  ],
  as_of: now,
  next_cursor: null,
};

export const doneToday: TaskDoneSummaryData = {
  from: "2026-08-27",
  through: "2026-08-27",
  timezone: "America/Los_Angeles",
  as_of: now,
  count: 2,
  done_today_count: 2,
  items: [
    {
      task_ref: "019d3f10-0001-7000-8000-000000000001",
      entry_ref: "entry:019d3f10-0001-7000-8000-000000000001",
      version: 2,
      title: "Finished release note",
      done_at: "2026-08-27T10:45:00Z",
      completed_via: "web",
    },
    {
      task_ref: "019d3f10-0002-7000-8000-000000000002",
      entry_ref: "entry:019d3f10-0002-7000-8000-000000000002",
      version: 2,
      title: "Closed old alert",
      done_at: "2026-08-27T10:30:00Z",
      completed_via: "agent:codex",
    },
  ],
  next_cursor: null,
};

export const taskDetail = {
  task: {
    task_ref: nextCandidates[0].task_ref,
    entry_ref: nextCandidates[0].entry_ref,
    version: 3,
    title: nextCandidates[0].title,
    status: "open" as const,
    task: {
      id: nextCandidates[0].task_ref,
      title: nextCandidates[0].title,
      project: {
        value: "straylight",
        source: "owner",
        set_at: "2026-08-20T00:00:00Z",
      },
      hard_due: {
        value: "2026-08-29T18:00:00Z",
        source: "agent:codex",
        set_at: "2026-08-20T00:00:00Z",
        note: "inferred from renewal language",
      },
      required_contexts: {
        value: ["online"],
        source: "agent:codex",
        set_at: "2026-08-20T00:00:00Z",
      },
      parked: { value: false, source: "owner", set_at: now },
    },
    provenance: { title_source: "owner" },
    source_timestamps: { title: "2026-08-20T00:00:00Z" },
    created_at: "2026-08-20T00:00:00Z",
    updated_at: now,
  },
};

export const taskProjectState = {
  project: {
    slug: "straylight",
    title: "Straylight",
    interest: "hot" as const,
    last_activity_at: "2026-08-27T11:00:00Z",
    version: 2,
  },
  checkpoint: {
    entry_ref: "entry:checkpoint",
    version: 4,
    attribution: "explicit",
    matched_path: null,
    checkpoint_at: "2026-08-27T10:00:00Z",
    linked_at: "2026-08-27T10:00:01Z",
    state: {
      objective: "Ship agent-first tasks",
      current_state: ["Web work is in progress"],
      next_actions: ["Finish gate 12c"],
      open_questions: ["Production smoke timing"],
    },
  },
  urgent_count: 1,
  next: nextCandidates.slice(0, 3),
  waiting: [
    {
      task_ref: candidate(9).task_ref,
      title: "Wait for review",
      waiting_on: { who_or_what: "Rourke" },
      since: "2026-08-20T00:00:00Z",
      age_days: 7,
    },
  ],
  waiting_total: 1,
  waiting_remaining: 0,
  parked_count: 2,
  rollups: { open: 8, waiting: 1, done: 12, dropped: 1 },
  as_of: "2026-08-27T11:00:00Z",
};

export function defaultTaskRoutes(): Record<string, unknown> {
  return {
    "GET /api/v1/workspace/tasks/candidates": (request: Request) => {
      const query = new URL(request.url).searchParams;
      const view = query.get("view") ?? "next";
      const limit = Number(query.get("limit") ?? "5");
      const source = view === "urgent" ? urgentCandidates : nextCandidates;
      return {
        status: "complete",
        data: {
          view,
          as_of: now,
          contexts_available: query.getAll("contexts_available"),
          items: source.slice(0, limit),
          urgent_total: urgentCandidates.length,
          next_remaining: Math.max(0, source.length - limit),
          backlog_total: 32,
          next_cursor: null,
        },
      };
    },
    "GET /api/v1/workspace/tasks/done-summary": {
      status: "complete",
      data: doneToday,
    },
    "GET /api/v1/workspace/contexts": {
      status: "complete",
      data: taskContexts,
    },
    "GET /api/v1/workspace/projects": {
      status: "complete",
      data: taskProjects,
    },
    "GET /api/v1/workspace/tasks/settings": {
      status: "complete",
      data: taskSettings,
    },
    "GET /api/v1/workspace/tasks/guard/status": {
      status: "complete",
      data: taskGuardStatus,
    },
    "GET /api/v1/workspace/integrations/todoist/status": {
      status: "complete",
      data: todoistStatus,
    },
  };
}
