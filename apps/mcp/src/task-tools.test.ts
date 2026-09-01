import assert from "node:assert/strict";
import test from "node:test";

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";

import { BrunnApiClient } from "./api-client.js";
import { createBrunnMcpServer } from "./index.js";

interface RecordedCall {
  url: string;
  method: string;
  body: string | undefined;
}

const taskRef = "019f8800-0000-7000-8000-000000000001";
const asOf = "2026-08-27T16:00:00-07:00";
const tooManyContexts = Array.from({ length: 21 }, (_, index) => `context-${index + 1}`);
const tooManySurfaceDefaults = Object.fromEntries(
  Array.from({ length: 21 }, (_, index) => [`surface-${index + 1}`, ["online"]]),
);
const taskToolNames = [
  "project.list",
  "project.register",
  "project.set_interest",
  "project.state",
  "task.candidates",
  "task.capture",
  "task.contexts",
  "task.corrections",
  "task.done_summary",
  "task.settings",
  "task.sync_status",
  "task.update",
] as const;

async function connectedPair(
  calls: RecordedCall[],
  options: { surface?: "local" | "remote"; status?: number; response?: Record<string, unknown> } = {},
): Promise<{ client: Client; close: () => Promise<void> }> {
  const fetchImpl: typeof fetch = async (input, init) => {
    calls.push({
      url: String(input),
      method: init?.method ?? "GET",
      body: typeof init?.body === "string" ? init.body : undefined,
    });
    return new Response(JSON.stringify(options.response ?? { status: "complete" }), {
      status: options.status ?? 200,
      headers: { "content-type": "application/json" },
    });
  };
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  const server = createBrunnMcpServer(
    new BrunnApiClient("https://api.invalid", "test-token", fetchImpl),
    { surface: options.surface ?? "local", includeStructuredContent: true },
  );
  const client = new Client({ name: "task-tools-test", version: "0.1.0" });
  await server.connect(serverTransport);
  await client.connect(clientTransport);
  return {
    client,
    close: async () => {
      await client.close().catch(() => undefined);
      await server.close().catch(() => undefined);
    },
  };
}

async function callOk(client: Client, name: string, arguments_: Record<string, unknown>) {
  const result = await client.callTool({ name, arguments: arguments_ });
  assert.notEqual(result.isError, true, JSON.stringify(result.content));
  return result;
}

test("both MCP profiles expose the exact public task surface with safe annotations", async () => {
  for (const surface of ["local", "remote"] as const) {
    const { client, close } = await connectedPair([], { surface });
    try {
      const tools = await client.listTools();
      const taskTools = tools.tools
        .filter((tool) => tool.name.startsWith("task.") || tool.name.startsWith("project."))
        .sort((left, right) => left.name.localeCompare(right.name));
      assert.deepEqual(taskTools.map((tool) => tool.name), [...taskToolNames]);

      const byName = new Map(taskTools.map((tool) => [tool.name, tool]));
      for (const name of [
        "task.candidates",
        "task.corrections",
        "task.done_summary",
        "project.list",
        "project.state",
        "task.sync_status",
      ]) {
        assert.equal(byName.get(name)?.annotations?.readOnlyHint, true, name);
        assert.equal(byName.get(name)?.annotations?.idempotentHint, true, name);
      }
      for (const name of [
        "task.capture",
        "task.update",
        "task.contexts",
        "task.settings",
        "project.register",
        "project.set_interest",
      ]) {
        assert.equal(byName.get(name)?.annotations?.readOnlyHint, false, name);
        assert.equal(byName.get(name)?.annotations?.idempotentHint, true, name);
        assert.equal(byName.get(name)?.annotations?.destructiveHint, false, name);
        assert.equal(byName.get(name)?.annotations?.openWorldHint, false, name);
      }

      const capture = byName.get("task.capture");
      assert.ok(capture);
      assert.match(capture.description ?? "", /consult task\.corrections/i);
      assert.match(capture.description ?? "", /project registry/i);
      assert.match(capture.description ?? "", /contexts/i);
      assert.match(capture.description ?? "", /call.*phone/i);
      assert.match(capture.description ?? "", /buy.*errands/i);
      assert.match(capture.description ?? "", /Nyx.*home/i);
      assert.match(capture.description ?? "", /at most one clarifying question/i);
      assert.match(capture.description ?? "", /hard.*soft ambiguity/i);
      assert.match(capture.description ?? "", /never overwrite.*owner/i);
      assert.match(capture.description ?? "", /never returns? the backlog/i);
      assert.deepEqual([...(capture.inputSchema.required ?? [])].sort(), [
        "idempotency_key",
        "items",
      ]);
      const items = capture.inputSchema.properties?.items as {
        minItems?: number;
        maxItems?: number;
      } | undefined;
      assert.equal(items?.minItems, 1);
      assert.equal(items?.maxItems, 25);

      const candidates = byName.get("task.candidates");
      assert.match(candidates?.description ?? "", /defaults? to.*next.*five/i);
      assert.match(candidates?.description ?? "", /AND semantics/i);
      assert.match(candidates?.description ?? "", /all.*explicit owner request/i);
      assert.match(candidates?.description ?? "", /status.*context.*date_type.*source/i);
      assert.match(candidates?.description ?? "", /only.*view=all/i);
      assert.match(candidates?.description ?? "", /as_of.*deterministic testing/i);
      const candidateProperties = candidates?.inputSchema.properties as Record<
        string,
        { enum?: string[]; pattern?: string }
      > | undefined;
      assert.deepEqual(candidateProperties?.status?.enum, [
        "all",
        "open",
        "waiting",
        "done",
        "dropped",
      ]);
      assert.deepEqual(candidateProperties?.date_type?.enum, [
        "all",
        "hard",
        "cost",
        "soft",
        "none",
      ]);
      assert.deepEqual(candidateProperties?.source?.enum, [
        "all",
        "owner",
        "agent",
        "derived",
        "todoist",
      ]);
      assert.equal(candidateProperties?.context?.pattern, "^[a-z0-9]+(?:-[a-z0-9]+)*$");

      const contexts = byName.get("task.contexts");
      assert.match(contexts?.description ?? "", /suggested_existing/);
      assert.match(contexts?.description ?? "", /confirm_new/);
      assert.match(contexts?.description ?? "", /never.*automatic.*merge/i);
      assert.match(contexts?.description ?? "", /list first.*expected.*version/i);

      const state = byName.get("project.state");
      assert.match(state?.description ?? "", /latest linked checkpoint/i);
      assert.match(state?.description ?? "", /next three/i);
      assert.match(state?.description ?? "", /waiting/i);
    } finally {
      await close();
    }
  }
});

test("task tools preserve exact methods, paths, repeated candidate contexts, and bodies", async () => {
  const calls: RecordedCall[] = [];
  const { client, close } = await connectedPair(calls);
  const source = "agent:aether";
  try {
    await callOk(client, "task.capture", {
      idempotency_key: "capture:call-insurer",
      items: [{
        client_ref: "spoken:1",
        raw_text: "Call the insurer before Friday or coverage lapses",
        project: { value: "personal-admin", source },
        required_contexts: { value: ["phone", "online"], source },
        hard_due: { value: "2026-08-28T17:00:00-07:00", source },
      }],
    });
    await callOk(client, "task.candidates", {
      view: "next",
      limit: 5,
      contexts_available: ["phone", "online"],
      project: "personal-admin",
      include_waiting: false,
      include_parked: false,
      as_of: asOf,
    });
    await callOk(client, "task.update", {
      task_ref: taskRef,
      expected_version: 2,
      idempotency_key: "complete:call-insurer:v2",
      operation: { type: "complete", source, completed_via: source },
    });
    await callOk(client, "task.corrections", {
      task_ref: taskRef,
      limit: 20,
      cursor: "correction:next",
    });
    await callOk(client, "task.done_summary", {
      from: "2026-08-25",
      through: "2026-08-27",
      as_of: asOf,
      limit: 25,
      cursor: "done:next",
    });
    await callOk(client, "task.sync_status", {});

    assert.deepEqual(calls, [
      {
        url: "https://api.invalid/v1/workspace/tasks/capture",
        method: "POST",
        body: JSON.stringify({
          idempotency_key: "capture:call-insurer",
          items: [{
            client_ref: "spoken:1",
            raw_text: "Call the insurer before Friday or coverage lapses",
            project: { value: "personal-admin", source },
            hard_due: { value: "2026-08-28T17:00:00-07:00", source },
            required_contexts: { value: ["phone", "online"], source },
          }],
        }),
      },
      {
        url: "https://api.invalid/v1/workspace/tasks/candidates?view=next&limit=5&contexts_available=phone&contexts_available=online&project=personal-admin&include_waiting=false&include_parked=false&as_of=2026-08-27T16%3A00%3A00-07%3A00",
        method: "GET",
        body: undefined,
      },
      {
        url: `https://api.invalid/v1/workspace/tasks/${taskRef}`,
        method: "PATCH",
        body: JSON.stringify({
          expected_version: 2,
          idempotency_key: "complete:call-insurer:v2",
          operation: { type: "complete", source, completed_via: source },
        }),
      },
      {
        url: `https://api.invalid/v1/workspace/tasks/corrections?task_ref=${taskRef}&limit=20&cursor=correction%3Anext`,
        method: "GET",
        body: undefined,
      },
      {
        url: "https://api.invalid/v1/workspace/tasks/done-summary?from=2026-08-25&through=2026-08-27&as_of=2026-08-27T16%3A00%3A00-07%3A00&limit=25&cursor=done%3Anext",
        method: "GET",
        body: undefined,
      },
      {
        url: "https://api.invalid/v1/workspace/integrations/todoist/status",
        method: "GET",
        body: undefined,
      },
    ]);
  } finally {
    await close();
  }
});

test("task.contexts maps every operation to its frozen HTTP route", async () => {
  const calls: RecordedCall[] = [];
  const { client, close } = await connectedPair(calls);
  try {
    await callOk(client, "task.contexts", {
      operation: { type: "list", include_archived: true, limit: 50, cursor: "next-page" },
    });
    await callOk(client, "task.contexts", {
      operation: {
        type: "create",
        slug: "on-phone",
        display_name: "On phone",
        aliases: ["telephone"],
        description: "Calls and phone-only work",
        source: "agent:aether",
        confirm_new: true,
        idempotency_key: "context:create:on-phone",
      },
    });
    await callOk(client, "task.contexts", {
      operation: {
        type: "merge",
        from: "on-phone",
        into: "phone",
        expected_from_version: 2,
        expected_into_version: 5,
        source: "agent:aether",
        reason: "Use the existing canonical context",
        idempotency_key: "context:merge:on-phone:phone",
      },
    });
    await callOk(client, "task.contexts", {
      operation: {
        type: "archive",
        slug: "on-phone",
        archived: true,
        expected_version: 3,
        source: "agent:aether",
        idempotency_key: "context:archive:on-phone",
      },
    });
    await callOk(client, "task.contexts", {
      operation: {
        type: "set_available",
        surface: "agent",
        contexts_available: ["phone", "online"],
        expected_version: 0,
        source: "agent:aether",
        idempotency_key: "context:available:agent",
      },
    });

    assert.deepEqual(calls, [
      {
        url: "https://api.invalid/v1/workspace/contexts?include_archived=true&limit=50&cursor=next-page",
        method: "GET",
        body: undefined,
      },
      {
        url: "https://api.invalid/v1/workspace/contexts",
        method: "POST",
        body: JSON.stringify({
          slug: "on-phone",
          display_name: "On phone",
          aliases: ["telephone"],
          description: "Calls and phone-only work",
          source: "agent:aether",
          confirm_new: true,
          idempotency_key: "context:create:on-phone",
        }),
      },
      {
        url: "https://api.invalid/v1/workspace/contexts/merge",
        method: "POST",
        body: JSON.stringify({
          from: "on-phone",
          into: "phone",
          expected_from_version: 2,
          expected_into_version: 5,
          source: "agent:aether",
          reason: "Use the existing canonical context",
          idempotency_key: "context:merge:on-phone:phone",
        }),
      },
      {
        url: "https://api.invalid/v1/workspace/contexts/on-phone",
        method: "PATCH",
        body: JSON.stringify({
          archived: true,
          expected_version: 3,
          source: "agent:aether",
          idempotency_key: "context:archive:on-phone",
        }),
      },
      {
        url: "https://api.invalid/v1/workspace/contexts/available/agent",
        method: "PUT",
        body: JSON.stringify({
          contexts_available: ["phone", "online"],
          expected_version: 0,
          source: "agent:aether",
          idempotency_key: "context:available:agent",
        }),
      },
    ]);
  } finally {
    await close();
  }
});

test("task.contexts uses bounded list defaults and requires optimistic registry versions", async () => {
  const calls: RecordedCall[] = [];
  const { client, close } = await connectedPair(calls);
  try {
    await callOk(client, "task.contexts", { operation: { type: "list" } });
    assert.deepEqual(calls, [{
      url: "https://api.invalid/v1/workspace/contexts?include_archived=false&limit=50",
      method: "GET",
      body: undefined,
    }]);
  } finally {
    await close();
  }
});

test("urgent candidates are not capped and context near matches are successful review handshakes", async () => {
  const urgentCalls: RecordedCall[] = [];
  const urgentPair = await connectedPair(urgentCalls);
  try {
    await callOk(urgentPair.client, "task.candidates", {
      view: "urgent",
      contexts_available: ["online"],
    });
    assert.deepEqual(urgentCalls, [{
      url: "https://api.invalid/v1/workspace/tasks/candidates?view=urgent&contexts_available=online&include_waiting=false&include_parked=false",
      method: "GET",
      body: undefined,
    }]);
    assert.equal(urgentCalls[0]?.url.includes("limit="), false);
  } finally {
    await urgentPair.close();
  }

  const reviewCalls: RecordedCall[] = [];
  const review = {
    status: "needs_review",
    suggested_existing: [{ slug: "phone", reason: "small_edit" }],
  };
  const reviewPair = await connectedPair(reviewCalls, { response: review });
  try {
    const result = await reviewPair.client.callTool({
      name: "task.contexts",
      arguments: {
        operation: {
          type: "create",
          slug: "phne",
          display_name: "Phne",
          source: "agent:aether",
          confirm_new: false,
          idempotency_key: "context:create:phne",
        },
      },
    });
    assert.notEqual(result.isError, true);
    assert.deepEqual(result.structuredContent, review);
    assert.deepEqual(reviewCalls, [{
      url: "https://api.invalid/v1/workspace/contexts",
      method: "POST",
      body: JSON.stringify({
        slug: "phne",
        display_name: "Phne",
        source: "agent:aether",
        confirm_new: false,
        idempotency_key: "context:create:phne",
      }),
    }]);
  } finally {
    await reviewPair.close();
  }
});

test("candidate pagination accepts only raw task refs", async () => {
  const calls: RecordedCall[] = [];
  const { client, close } = await connectedPair(calls);
  try {
    await callOk(client, "task.candidates", {
      view: "all",
      deliberate_all: true,
      limit: 25,
      cursor: taskRef,
    });
    assert.deepEqual(calls, [{
      url: `https://api.invalid/v1/workspace/tasks/candidates?view=all&limit=25&include_waiting=false&include_parked=false&cursor=${taskRef}&deliberate_all=true`,
      method: "GET",
      body: undefined,
    }]);
  } finally {
    await close();
  }
});

test("deliberate all candidate filters use the frozen backend query contract", async () => {
  const calls: RecordedCall[] = [];
  const { client, close } = await connectedPair(calls);
  try {
    await callOk(client, "task.candidates", {
      view: "all",
      deliberate_all: true,
      limit: 25,
      project: "brunn",
      context: "phone",
      status: "done",
      date_type: "hard",
      source: "agent",
      include_waiting: true,
      include_parked: true,
      as_of: asOf,
      cursor: taskRef,
    });
    assert.deepEqual(calls, [{
      url: "https://api.invalid/v1/workspace/tasks/candidates?view=all&limit=25&project=brunn&context=phone&status=done&date_type=hard&source=agent&include_waiting=true&include_parked=true&as_of=2026-08-27T16%3A00%3A00-07%3A00&cursor=019f8800-0000-7000-8000-000000000001&deliberate_all=true",
      method: "GET",
      body: undefined,
    }]);
  } finally {
    await close();
  }
});

test("task.settings and project tools map get and optimistic mutations exactly", async () => {
  const calls: RecordedCall[] = [];
  const { client, close } = await connectedPair(calls);
  try {
    await callOk(client, "task.settings", { operation: { type: "get" } });
    await callOk(client, "task.settings", {
      operation: {
        type: "update",
        expected_version: 3,
        idempotency_key: "settings:v3",
        timezone: "America/Los_Angeles",
        hard_lead_days: 9,
        hard_second_lead_hours: 36,
        due_day_local_time: "07:30:00",
        soft_window_days: 4,
        triage_after_days: 21,
        waiting_followup_days: 5,
        quiet_override_enabled: true,
        quiet_override_within_hours: 18,
        quiet_hours_start: "22:00:00",
        quiet_hours_end: "07:00:00",
        surface_defaults: { agent: ["online"], ios: ["phone", "online"] },
      },
    });
    await callOk(client, "project.register", {
      slug: "brunn",
      title: "Brunn",
      aliases: ["carry state"],
      description: "Durable context service",
      hub_path: "sources/Projects/Brunn/Brunn.md",
      repo_path: "/Volumes/NyxFastData/dev/projects/brunn",
      source: "agent:aether",
      expected_version: 0,
      idempotency_key: "project:register:brunn",
    });
    await callOk(client, "project.list", {
      include_archived: false,
      limit: 50,
      cursor: "next-project",
      as_of: asOf,
    });
    await callOk(client, "project.state", { slug: "brunn", as_of: asOf });
    await callOk(client, "project.set_interest", {
      slug: "brunn",
      interest: "hot",
      source: "agent:aether",
      expected_version: 2,
      idempotency_key: "project:interest:brunn:v2",
    });

    assert.deepEqual(calls, [
      {
        url: "https://api.invalid/v1/workspace/tasks/settings",
        method: "GET",
        body: undefined,
      },
      {
        url: "https://api.invalid/v1/workspace/tasks/settings",
        method: "PUT",
        body: JSON.stringify({
          expected_version: 3,
          idempotency_key: "settings:v3",
          timezone: "America/Los_Angeles",
          hard_lead_days: 9,
          hard_second_lead_hours: 36,
          due_day_local_time: "07:30:00",
          soft_window_days: 4,
          triage_after_days: 21,
          waiting_followup_days: 5,
          quiet_override_enabled: true,
          quiet_override_within_hours: 18,
          quiet_hours_start: "22:00:00",
          quiet_hours_end: "07:00:00",
          surface_defaults: { agent: ["online"], ios: ["phone", "online"] },
        }),
      },
      {
        url: "https://api.invalid/v1/workspace/projects/brunn",
        method: "PUT",
        body: JSON.stringify({
          title: "Brunn",
          aliases: ["carry state"],
          description: "Durable context service",
          hub_path: "sources/Projects/Brunn/Brunn.md",
          repo_path: "/Volumes/NyxFastData/dev/projects/brunn",
          source: "agent:aether",
          expected_version: 0,
          idempotency_key: "project:register:brunn",
        }),
      },
      {
        url: "https://api.invalid/v1/workspace/projects?include_archived=false&limit=50&cursor=next-project&as_of=2026-08-27T16%3A00%3A00-07%3A00",
        method: "GET",
        body: undefined,
      },
      {
        url: "https://api.invalid/v1/workspace/projects/brunn/state?as_of=2026-08-27T16%3A00%3A00-07%3A00",
        method: "GET",
        body: undefined,
      },
      {
        url: "https://api.invalid/v1/workspace/projects/brunn/interest",
        method: "PUT",
        body: JSON.stringify({
          interest: "hot",
          source: "agent:aether",
          expected_version: 2,
          idempotency_key: "project:interest:brunn:v2",
        }),
      },
    ]);
  } finally {
    await close();
  }
});

test("task tool validation fails closed before HTTP for ambiguous or unsafe requests", async () => {
  const calls: RecordedCall[] = [];
  const { client, close } = await connectedPair(calls);
  try {
    for (const fixture of [
      { name: "task.candidates", arguments: { view: "all" } },
      { name: "task.candidates", arguments: { view: "next", limit: 26 } },
      { name: "task.candidates", arguments: { view: "next", status: "open" } },
      { name: "task.candidates", arguments: { view: "urgent", context: "phone" } },
      { name: "task.candidates", arguments: { view: "triage", date_type: "hard" } },
      { name: "task.candidates", arguments: { view: "next", source: "owner" } },
      {
        name: "task.candidates",
        arguments: { view: "all", deliberate_all: true, status: "ready" },
      },
      {
        name: "task.candidates",
        arguments: { view: "all", deliberate_all: true, context: "Phone" },
      },
      {
        name: "task.candidates",
        arguments: { view: "all", deliberate_all: true, date_type: "due" },
      },
      {
        name: "task.candidates",
        arguments: { view: "all", deliberate_all: true, source: "inferred" },
      },
      {
        name: "task.candidates",
        arguments: { view: "all", deliberate_all: true, cursor: `task:${taskRef}` },
      },
      {
        name: "task.candidates",
        arguments: { view: "next", contexts_available: tooManyContexts },
      },
      {
        name: "task.capture",
        arguments: {
          idempotency_key: "capture:reserved-source",
          items: [{ raw_text: "Call the insurer", project: { value: "admin", source: "derived" } }],
        },
      },
      {
        name: "task.capture",
        arguments: {
          idempotency_key: "capture:too-many-contexts",
          items: [{
            raw_text: "Use too many contexts",
            required_contexts: { value: tooManyContexts, source: "agent:aether" },
          }],
        },
      },
      {
        name: "task.update",
        arguments: {
          task_ref: `task:${taskRef}`,
          expected_version: 1,
          idempotency_key: "update:prefixed-ref",
          operation: {
            type: "complete",
            source: "agent:aether",
            completed_via: "agent:aether",
          },
        },
      },
      {
        name: "task.update",
        arguments: {
          task_ref: taskRef,
          expected_version: 1,
          idempotency_key: "update:too-many-contexts",
          operation: {
            type: "correct",
            field: "required_contexts",
            value: tooManyContexts,
            source: "agent:aether",
          },
        },
      },
      {
        name: "task.update",
        arguments: {
          task_ref: taskRef,
          expected_version: 1,
          idempotency_key: "update:missing-correction-value",
          operation: {
            type: "correct",
            field: "project",
            source: "agent:aether",
            reason: "Project inference was wrong",
          },
        },
      },
      { name: "task.done_summary", arguments: { from: "2026-08-27" } },
      {
        name: "task.done_summary",
        arguments: { from: "2026-08-28", through: "2026-08-27" },
      },
      {
        name: "task.contexts",
        arguments: { operation: { type: "create", slug: "phone", idempotency_key: "context:1" } },
      },
      {
        name: "task.contexts",
        arguments: { operation: { type: "list", limit: 101 } },
      },
      {
        name: "task.contexts",
        arguments: { operation: { type: "list", cursor: "context:next" } },
      },
      {
        name: "task.contexts",
        arguments: {
          operation: {
            type: "merge",
            from: "on-phone",
            into: "phone",
            source: "agent:aether",
            idempotency_key: "context:merge:missing-versions",
          },
        },
      },
      {
        name: "task.contexts",
        arguments: {
          operation: {
            type: "merge",
            from: "on-phone",
            into: "phone",
            expected_from_version: 1,
            expected_into_version: 0,
            source: "agent:aether",
            idempotency_key: "context:merge:zero-into-version",
          },
        },
      },
      {
        name: "task.contexts",
        arguments: {
          operation: {
            type: "merge",
            from: "on-phone",
            into: "phone",
            expected_from_version: 0,
            expected_into_version: 1,
            source: "agent:aether",
            idempotency_key: "context:merge:zero-version",
          },
        },
      },
      {
        name: "task.contexts",
        arguments: {
          operation: {
            type: "archive",
            slug: "phone",
            source: "agent:aether",
            idempotency_key: "context:archive:missing-version",
          },
        },
      },
      {
        name: "task.contexts",
        arguments: {
          operation: {
            type: "set_available",
            surface: "agent",
            contexts_available: tooManyContexts,
            expected_version: 0,
            source: "agent:aether",
            idempotency_key: "context:available:too-many-contexts",
          },
        },
      },
      {
        name: "task.settings",
        arguments: {
          operation: {
            type: "update",
            expected_version: 1,
            idempotency_key: "settings:too-many-contexts",
            surface_defaults: { agent: tooManyContexts },
          },
        },
      },
      {
        name: "task.settings",
        arguments: {
          operation: {
            type: "update",
            expected_version: 1,
            idempotency_key: "settings:too-many-surfaces",
            surface_defaults: tooManySurfaceDefaults,
          },
        },
      },
      {
        name: "project.list",
        arguments: { cursor: "project:next" },
      },
      {
        name: "task.contexts",
        arguments: {
          operation: {
            type: "archive",
            slug: "phone",
            expected_version: 0,
            source: "agent:aether",
            idempotency_key: "context:archive:zero-version",
          },
        },
      },
      {
        name: "task.contexts",
        arguments: {
          operation: {
            type: "set_available",
            surface: "agent",
            contexts_available: ["phone"],
            source: "agent:aether",
            idempotency_key: "context:available:missing-version",
          },
        },
      },
      {
        name: "task.contexts",
        arguments: {
          operation: {
            type: "set_available",
            surface: "agent",
            contexts_available: ["phone"],
            expected_version: -1,
            source: "agent:aether",
            idempotency_key: "context:available:negative-version",
          },
        },
      },
    ]) {
      const result = await client.callTool({ name: fixture.name, arguments: fixture.arguments });
      assert.equal(result.isError, true, fixture.name);
    }
    assert.equal(calls.length, 0);
  } finally {
    await close();
  }
});

test("every mutating task tool preserves a read-only capability denial", async () => {
  const calls: RecordedCall[] = [];
  const failure = {
    error: {
      code: "capability_denied",
      message: "task.write capability required",
    },
  };
  const { client, close } = await connectedPair(calls, { status: 403, response: failure });
  const fixtures = [
    {
      name: "task.capture",
      arguments: { idempotency_key: "deny:capture", items: [{ raw_text: "Call insurer" }] },
    },
    {
      name: "task.update",
      arguments: {
        task_ref: taskRef,
        expected_version: 1,
        idempotency_key: "deny:update",
        operation: {
          type: "complete",
          source: "agent:aether",
          completed_via: "agent:aether",
        },
      },
    },
    {
      name: "task.contexts",
      arguments: {
        operation: {
          type: "create",
          slug: "office",
          display_name: "Office",
          source: "agent:aether",
          idempotency_key: "deny:context",
        },
      },
    },
    {
      name: "task.settings",
      arguments: {
        operation: {
          type: "update",
          expected_version: 1,
          idempotency_key: "deny:settings",
          soft_window_days: 4,
        },
      },
    },
    {
      name: "project.register",
      arguments: {
        slug: "brunn",
        title: "Brunn",
        source: "agent:aether",
        idempotency_key: "deny:project",
      },
    },
    {
      name: "project.set_interest",
      arguments: {
        slug: "brunn",
        interest: "hot",
        source: "agent:aether",
        expected_version: 1,
        idempotency_key: "deny:interest",
      },
    },
  ];

  try {
    for (const fixture of fixtures) {
      const result = await client.callTool({ name: fixture.name, arguments: fixture.arguments });
      assert.equal(result.isError, true, fixture.name);
      assert.match(JSON.stringify(result.content), /capability_denied/);
    }
    assert.equal(calls.length, fixtures.length);
  } finally {
    await close();
  }
});
