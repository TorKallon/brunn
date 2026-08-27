#!/usr/bin/env node

import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { mkdir, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

const SCHEMA = "straylight-agent-first-tasks-gate12@v1";
const EXPECTED_TASK_TOOLS = [
  "project.register",
  "project.state",
  "task.candidates",
  "task.capture",
  "task.contexts",
  "task.corrections",
  "task.done_summary",
  "task.update",
];
const config = parseArguments(process.argv.slice(2));
const started = performance.now();
const checks = [];
const evidence = {
  schema: SCHEMA,
  status: "fail",
  api_url: redactUrl(config.apiUrl),
  checks,
};
let client;

try {
  const ready = await record("http.ready", async () => {
    const response = await fetch(new URL("/ready", config.apiUrl), {
      headers: { accept: "application/json" },
      signal: AbortSignal.timeout(30_000),
    });
    const body = await response.json();
    assert.equal(response.ok, true, `readiness returned HTTP ${response.status}`);
    assert.ok(isObject(body), "readiness returned non-object JSON");
    return body;
  });
  evidence.ready = compactReadyEvidence(ready);

  const environment = Object.fromEntries(
    ["LANG", "LC_ALL", "PATH", "TMPDIR"]
      .map((name) => [name, process.env[name]])
      .filter((entry) => entry[1] !== undefined),
  );
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [config.mcpEntry],
    env: {
      ...environment,
      STRAYLIGHT_API_URL: config.apiUrl.href,
      STRAYLIGHT_API_TOKEN: config.apiToken,
      STRAYLIGHT_MCP_INCLUDE_STRUCTURED_CONTENT: "1",
      STRAYLIGHT_MCP_RETRY_BACKOFF_MS: "10,20,40,80,160,320",
    },
  });
  client = new Client({ name: "task-gate12-scenario", version: "1.0.0" });
  await record("mcp.connect", () => client.connect(transport));

  const listed = await record("mcp.required_tool_preflight", async () => {
    const response = await client.listTools();
    const names = response.tools.map((tool) => tool.name).sort();
    const missing = EXPECTED_TASK_TOOLS.filter((name) => !names.includes(name));
    assert.deepEqual(missing, [], `compiled adapter omits task tools: ${missing.join(", ")}`);
    return names;
  });
  evidence.task_tools = EXPECTED_TASK_TOOLS;
  evidence.total_tool_count = listed.length;

  if (config.preflightOnly) {
    evidence.status = "pass";
  } else {
    Object.assign(evidence, await runScenario(client));
    evidence.status = "pass";
  }
} catch (error) {
  evidence.error = sanitizeError(error, config.apiToken);
} finally {
  if (client !== undefined) {
    await client.close().catch(() => undefined);
  }
  evidence.elapsed_ms = roundMilliseconds(performance.now() - started);
  await writeEvidence(config, evidence);
  process.stdout.write(`${JSON.stringify(evidence, null, 2)}\n`);
}

process.exitCode = evidence.status === "pass" ? 0 : 1;

async function runScenario(mcp) {
  const suffix = randomBytes(6).toString("hex");
  const source = "agent:gate12";
  const projectSlug = `gate12-${suffix}`;
  const hubDirectory = `sources/Projects/Gate12-${suffix}`;
  const hubPath = `${hubDirectory}/Project.md`;
  const repoPath = `/tmp/straylight-gate12-${suffix}/`;
  const scenarioAsOf = new Date();
  const tomorrow = new Date(scenarioAsOf.getTime() + 24 * 60 * 60 * 1_000)
    .toISOString()
    .slice(0, 10);

  const opened = await record("memory.open", () => invoke(mcp, "memory.open", {
    task: "Run the disposable agent-first task gate-12 scenario",
    token_budget: 2_000,
  }));
  const sessionId = requireString(findValue(opened, "session_id"), "memory.open session_id");

  await record("project.register", () => invoke(mcp, "project.register", {
    slug: projectSlug,
    title: "Gate 12 disposable project",
    description: "Synthetic project for the real-interface acceptance scenario.",
    aliases: [`gate-${suffix}`],
    hub_path: hubPath,
    repo_path: repoPath,
    source,
    idempotency_key: `gate12:${suffix}:project-register`,
  }));

  await record("memory.write_hub_source", () => invoke(mcp, "memory.write", {
    path: hubPath,
    content: "# Gate 12 disposable project\n\nSynthetic checkpoint-link source.\n",
    media_type: "text/markdown",
    idempotency_key: `gate12:${suffix}:hub-source`,
  }));

  const explicitObjective = `Gate 12 explicit checkpoint ${suffix}`;
  const explicitCheckpoint = await record("project.checkpoint_explicit", () => invoke(
    mcp,
    "memory.checkpoint",
    {
      session_id: sessionId,
      idempotency_key: `gate12:${suffix}:checkpoint-explicit`,
      state: {
        objective: explicitObjective,
        project: projectSlug,
        current_state: ["Explicit project linkage is under test."],
      },
    },
  ));
  const explicitState = await record("project.state_explicit", () => invoke(
    mcp,
    "project.state",
    { slug: projectSlug, as_of: futureAsOf() },
  ));
  assertContainsKeyValue(explicitState, "objective", explicitObjective);

  const hubObjective = `Gate 12 hub fallback checkpoint ${suffix}`;
  const hubCheckpoint = await record("project.checkpoint_hub_fallback", () => invoke(
    mcp,
    "memory.checkpoint",
    {
      session_id: sessionId,
      idempotency_key: `gate12:${suffix}:checkpoint-hub`,
      state: {
        objective: hubObjective,
        current_state: ["Hub-path fallback is under test."],
      },
      source_refs: [hubPath],
    },
  ));
  const hubState = await record("project.state_hub_fallback", () => invoke(
    mcp,
    "project.state",
    { slug: projectSlug, as_of: futureAsOf() },
  ));
  assertContainsKeyValue(hubState, "objective", hubObjective);

  const repoObjective = `Gate 12 repository fallback checkpoint ${suffix}`;
  const repoCheckpoint = await record("project.checkpoint_repo_fallback", () => invoke(
    mcp,
    "memory.checkpoint",
    {
      session_id: sessionId,
      idempotency_key: `gate12:${suffix}:checkpoint-repo`,
      state: {
        objective: repoObjective,
        current_state: ["Repository-path fallback is under test."],
        state_refs: [`${repoPath}state/current.md`],
      },
    },
  ));
  const repoState = await record("project.state_repo_fallback", () => invoke(
    mcp,
    "project.state",
    { slug: projectSlug, as_of: futureAsOf() },
  ));
  assertContainsKeyValue(repoState, "objective", repoObjective);

  const contextsBefore = await record("task.contexts_list_before", () => invoke(
    mcp,
    "task.contexts",
    { operation: { type: "list", include_archived: false } },
  ));
  assert.equal(hasExactString(contextsBefore, "phone"), true, "seeded phone context is absent");

  const nearMatch = await record("task.contexts_near_match_refusal", () => invoke(
    mcp,
    "task.contexts",
    {
      operation: {
        type: "create",
        slug: "phne",
        display_name: "Phne",
        description: "Must be refused as a near-match to phone.",
        aliases: [],
        source,
        confirm_new: false,
        idempotency_key: `gate12:${suffix}:context-near-match`,
      },
    },
  ));
  assertContainsKeyValue(nearMatch, "status", "needs_review");
  assert.equal(hasContextSlug(nearMatch, "phone"), true, "near-match refusal omitted phone");
  const contextsAfter = await record("task.contexts_no_write_after_refusal", () => invoke(
    mcp,
    "task.contexts",
    { operation: { type: "list", include_archived: true } },
  ));
  assert.equal(hasContextSlug(contextsAfter, "phne"), false, "refused context was persisted");

  const capture = await record("task.capture", () => invoke(mcp, "task.capture", {
    idempotency_key: `gate12:${suffix}:capture`,
    items: [{
      client_ref: `gate12-${suffix}`,
      raw_text: "Prepare the disposable gate proof at home by tomorrow.",
      title: "Prepare disposable gate proof",
      project: sourced(projectSlug, source, "Registered project selected by the agent."),
      required_contexts: sourced(["home"], source, "The task requires the home context."),
      soft_due: sourced(tomorrow, source, "The request says by tomorrow."),
      estimate_minutes: sourced(20, source, "A small deterministic fixture estimate."),
    }],
  }));
  const taskRef = requireTaskReference(capture);
  const taskId = requireTaskId(capture, taskRef);
  const taskPath = `.straylight/tasks/${taskId}.md`;
  const capturedVersion = requirePositiveInteger(findValue(capture, "version"), "capture version");
  assert.equal(
    hasSourceForValue(capture, projectSlug, source),
    true,
    "capture response omitted project enrichment provenance",
  );
  assert.equal(
    hasSourceForValue(capture, ["home"], source),
    true,
    "capture response omitted context enrichment provenance",
  );

  const blockedCandidates = await record("task.candidates_context_block", () => invoke(
    mcp,
    "task.candidates",
    {
      view: "next",
      limit: 5,
      contexts_available: ["online"],
      project: projectSlug,
      as_of: new Date().toISOString(),
    },
  ));
  assert.equal(responseContainsTask(blockedCandidates, taskRef, taskId), false,
    "context-unavailable task appeared in candidates");

  const visibleCandidates = await record("task.candidates_reason_and_provenance", () => invoke(
    mcp,
    "task.candidates",
    {
      view: "next",
      limit: 5,
      contexts_available: ["home", "online"],
      project: projectSlug,
      as_of: new Date().toISOString(),
    },
  ));
  const candidate = requireTaskItem(visibleCandidates, taskRef, taskId);
  assert.equal(typeof candidate.reason, "string");
  assert.notEqual(candidate.reason.trim(), "", "candidate reason is empty");
  assert.ok(
    Array.isArray(candidate.provenance_markers) && candidate.provenance_markers.length > 0,
    "candidate omitted visible provenance markers",
  );

  const corrected = await record("task.update_correction", () => invoke(mcp, "task.update", {
    task_ref: taskRef,
    expected_version: capturedVersion,
    idempotency_key: `gate12:${suffix}:correct-context`,
    operation: {
      type: "correct",
      field: "required_contexts",
      value: ["online"],
      source,
      note: "The synthetic task is available online.",
      reason: "Gate 12 correction-history proof.",
    },
  }));
  const correctedVersion = requirePositiveInteger(findValue(corrected, "version"), "correction version");
  assert.ok(correctedVersion > capturedVersion, "correction did not advance task version");

  const corrections = await record("task.corrections_history", () => invoke(
    mcp,
    "task.corrections",
    { task_ref: taskRef, limit: 10 },
  ));
  const correction = requireObjectWithKeyValue(corrections, "field", "required_contexts")
    ?? requireObjectWithKeyValue(corrections, "field_name", "required_contexts");
  assert.ok(correction, "corrections history omitted required_contexts");
  assert.equal(
    findValue(correction, "corrected_source") ?? findValue(correction, "source"),
    source,
    "correction history omitted the correcting source",
  );

  const completed = await record("task.update_complete", () => invoke(mcp, "task.update", {
    task_ref: taskRef,
    expected_version: correctedVersion,
    idempotency_key: `gate12:${suffix}:complete`,
    operation: {
      type: "complete",
      source,
      completed_via: source,
    },
  }));
  const completedVersion = requirePositiveInteger(findValue(completed, "version"), "completion version");
  assert.ok(completedVersion > correctedVersion, "completion did not advance task version");
  assert.ok(
    requirePositiveInteger(findValue(completed, "done_today_count"), "done_today_count") >= 1,
    "completion did not increment Done today",
  );

  const doneAsOf = new Date(Date.now() + 60_000).toISOString();
  const doneSummary = await record("task.done_summary", () => invoke(mcp, "task.done_summary", {
    as_of: doneAsOf,
    limit: 25,
  }));
  assert.equal(responseContainsTask(doneSummary, taskRef, taskId), true,
    "completed task is absent from Done today");
  const doneCount = findValue(doneSummary, "done_today_count");
  assert.ok(Number.isInteger(doneCount) && doneCount >= 1, "Done today count was not positive");

  const changes = await record("memory.changes_task_history", () => invoke(
    mcp,
    "memory.changes",
    { since_generation: 0, limit: 2_000 },
  ));
  const taskChanges = findObjects(changes, (value) => value.path === taskPath);
  const changeVersions = taskChanges
    .map((change) => change.version ?? change.entry_version)
    .filter(Number.isInteger)
    .sort((left, right) => left - right);
  assert.deepEqual(
    changeVersions,
    [capturedVersion, correctedVersion, completedVersion],
    "memory.changes did not expose every task version",
  );

  return {
    task_version_count: changeVersions.length,
    checkpoint_linkage_count: [explicitCheckpoint, hubCheckpoint, repoCheckpoint].length,
    linkage: {
      explicit: true,
      hub_path_fallback: true,
      repo_path_fallback: true,
    },
    context_near_match_refused_without_write: true,
    context_visibility_blocked: true,
    candidate_reason_and_provenance_visible: true,
    correction_history_visible: true,
    done_today_visible: true,
    task_changes_visible: true,
  };
}

async function invoke(mcp, name, args) {
  const result = await mcp.callTool({ name, arguments: args });
  const body = parseToolBody(result);
  if (result.isError === true) {
    const code = findValue(body, "code");
    const safeCode = typeof code === "string" && /^[a-z0-9._-]{1,120}$/u.test(code)
      ? code
      : "mcp_tool_error";
    throw new Error(`${name} failed with ${safeCode}`);
  }
  return body;
}

function parseToolBody(result) {
  if (isObject(result.structuredContent)) return result.structuredContent;
  const text = Array.isArray(result.content)
    ? result.content.find((item) => item?.type === "text")?.text
    : undefined;
  assert.equal(typeof text, "string", "MCP tool response omitted JSON text content");
  let value;
  try {
    value = JSON.parse(text);
  } catch {
    throw new Error("MCP tool response was not valid JSON");
  }
  assert.ok(isObject(value), "MCP tool response was not an object");
  return value;
}

async function record(name, operation) {
  const checkStarted = performance.now();
  try {
    const result = await operation();
    checks.push({ name, status: "pass", elapsed_ms: roundMilliseconds(performance.now() - checkStarted) });
    return result;
  } catch (error) {
    checks.push({
      name,
      status: "fail",
      elapsed_ms: roundMilliseconds(performance.now() - checkStarted),
      error: sanitizeError(error, config.apiToken),
    });
    throw error;
  }
}

function sourced(value, source, note) {
  return { value, source, note };
}

function requireTaskReference(body) {
  const value = requireString(findValue(body, "task_ref"), "captured task_ref");
  assert.match(
    value,
    /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u,
    "captured task_ref was not a lowercase UUIDv7",
  );
  return value;
}

function requireTaskId(body, taskRef) {
  const taskId = findValue(body, "task_id") ?? taskRef;
  assert.equal(taskId, taskRef, "capture returned inconsistent task_ref and task_id values");
  return taskId;
}

function requireTaskItem(body, taskRef, taskId) {
  const matches = findObjects(body, (value) => objectContainsTask(value, taskRef, taskId));
  const item = matches.find((value) => typeof value.reason === "string");
  assert.ok(item, "candidate response omitted the captured task item");
  return item;
}

function responseContainsTask(body, taskRef, taskId) {
  return findObjects(body, (value) => objectContainsTask(value, taskRef, taskId)).length > 0;
}

function objectContainsTask(value, taskRef, taskId) {
  return [value.task_ref, value.task_id, value.id, value.reference].includes(taskRef)
    || [value.task_ref, value.task_id, value.id, value.reference].includes(taskId);
}

function hasSourceForValue(body, expectedValue, expectedSource) {
  return findObjects(body, (value) => {
    if (value.source !== expectedSource || !("value" in value)) return false;
    return JSON.stringify(value.value) === JSON.stringify(expectedValue);
  }).length > 0;
}

function hasContextSlug(body, slug) {
  return findObjects(body, (value) => value.slug === slug).length > 0;
}

function requireObjectWithKeyValue(body, key, expected) {
  return findObjects(body, (value) => value[key] === expected)[0];
}

function assertContainsKeyValue(body, key, expected) {
  assert.ok(requireObjectWithKeyValue(body, key, expected), `response omitted ${key}`);
}

function findObjects(value, predicate, found = []) {
  if (Array.isArray(value)) {
    for (const item of value) findObjects(item, predicate, found);
  } else if (isObject(value)) {
    if (predicate(value)) found.push(value);
    for (const item of Object.values(value)) findObjects(item, predicate, found);
  }
  return found;
}

function findValue(value, key) {
  if (Array.isArray(value)) {
    for (const item of value) {
      const found = findValue(item, key);
      if (found !== undefined) return found;
    }
  } else if (isObject(value)) {
    if (key in value) return value[key];
    for (const item of Object.values(value)) {
      const found = findValue(item, key);
      if (found !== undefined) return found;
    }
  }
  return undefined;
}

function hasExactString(value, expected) {
  if (value === expected) return true;
  if (Array.isArray(value)) return value.some((item) => hasExactString(item, expected));
  if (isObject(value)) return Object.values(value).some((item) => hasExactString(item, expected));
  return false;
}

function requireString(value, label) {
  assert.equal(typeof value, "string", `${label} was not a string`);
  assert.notEqual(value.trim(), "", `${label} was empty`);
  return value;
}

function requirePositiveInteger(value, label) {
  assert.ok(Number.isInteger(value) && value > 0, `${label} was not a positive integer`);
  return value;
}

function isObject(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function compactReadyEvidence(body) {
  return {
    status: findValue(body, "status") ?? "ready",
    revision: findValue(body, "revision") ?? findValue(body, "version"),
  };
}

function roundMilliseconds(value) {
  return Math.round(value * 1_000) / 1_000;
}

function futureAsOf() {
  return new Date(Date.now() + 60_000).toISOString();
}

function sanitizeError(error, secret) {
  const raw = error instanceof Error ? error.message : String(error);
  return raw.replaceAll(secret, "[REDACTED]").slice(0, 2_000);
}

function redactUrl(url) {
  const value = new URL(url);
  value.username = "";
  value.password = "";
  return value.href;
}

async function writeEvidence(options, body) {
  const json = `${JSON.stringify(body, null, 2)}\n`;
  const junit = junitXml(body);
  await Promise.all([
    writeArtifact(options.jsonOutput, json),
    writeArtifact(options.junitOutput, junit),
  ]);
}

async function writeArtifact(path, content) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, content, { encoding: "utf8", mode: 0o600 });
}

function junitXml(body) {
  const failed = body.status === "pass" ? 0 : 1;
  const cases = body.checks.map((check) => {
    const failure = check.status === "fail"
      ? `<failure message="${xml(check.error ?? "contract failure")}"/>`
      : "";
    return `<testcase classname="agent_first_tasks.gate12" name="${xml(check.name)}" time="${(
      check.elapsed_ms / 1_000
    ).toFixed(6)}">${failure}</testcase>`;
  }).join("");
  return [
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
    `<testsuite name="agent-first-tasks-gate12" tests="${body.checks.length}" failures="${failed}" time="${(
      body.elapsed_ms / 1_000
    ).toFixed(6)}">${cases}</testsuite>`,
    "",
  ].join("\n");
}

function xml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("\"", "&quot;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

function parseArguments(args) {
  const values = {};
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--preflight-only") {
      values.preflightOnly = true;
      continue;
    }
    const next = args[index + 1];
    if (next === undefined || next.startsWith("--")) {
      throw new Error(`${argument} requires a value`);
    }
    values[argument] = next;
    index += 1;
  }
  const apiUrl = new URL(values["--api-url"] ?? requiredEnvironment("STRAYLIGHT_API_URL"));
  const apiToken = requiredEnvironment("STRAYLIGHT_API_TOKEN");
  const scriptDirectory = dirname(fileURLToPath(import.meta.url));
  return {
    apiUrl,
    apiToken,
    mcpEntry: resolve(values["--mcp-entry"] ?? `${scriptDirectory}/../dist/index.js`),
    jsonOutput: resolve(values["--json-output"] ?? requiredEnvironment("STRAYLIGHT_TASK_GATE12_JSON")),
    junitOutput: resolve(values["--junit-output"] ?? requiredEnvironment("STRAYLIGHT_TASK_GATE12_JUNIT")),
    preflightOnly: values.preflightOnly === true,
  };
}

function requiredEnvironment(name) {
  const value = process.env[name];
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`${name} is required`);
  }
  return value;
}
