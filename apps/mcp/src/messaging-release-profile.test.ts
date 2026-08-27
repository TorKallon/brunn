import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

interface MessagingReleaseProfile {
  readonly MESSAGING_TOOL_NAMES: readonly string[];
  messagingChildEnvironment(
    environment: Readonly<Record<string, string | undefined>>,
  ): Record<string, string>;
  expectedRemoteToolNames(
    existingTools: readonly string[],
    environment: Readonly<Record<string, string | undefined>>,
  ): readonly string[];
}

const profileUrl = new URL("../scripts/messaging-release-profile.mjs", import.meta.url);
const profile = await import(profileUrl.href) as MessagingReleaseProfile;

test("task scenario and remote canary consume the shared release profile", async () => {
  const [taskScenario, remoteCanary] = await Promise.all([
    readFile(new URL("../scripts/task-gate12-scenario.mjs", import.meta.url), "utf8"),
    readFile(new URL("../scripts/remote-canary.mjs", import.meta.url), "utf8"),
  ]);

  assert.match(taskScenario, /\.\.\.messagingChildEnvironment\(process\.env\)/);
  assert.match(remoteCanary, /expectedRemoteToolNames\(\[/);
  assert.match(remoteCanary, /\], process\.env\)\)/);

  const expectedToolsSource = remoteCanary.match(
    /expectedRemoteToolNames\(\[([\s\S]*?)\], process\.env\)/,
  )?.[1];
  if (expectedToolsSource === undefined) assert.fail("remote canary tool expectation is missing");
  const gateOffTools = [...expectedToolsSource.matchAll(/"([^"]+)"/g)].map((match) => {
    const name = match[1];
    if (name === undefined) assert.fail("remote canary tool name is missing");
    return name;
  });
  assert.equal(gateOffTools.length, 32);
  assert.deepEqual(gateOffTools, [...gateOffTools].sort());
  assert.deepEqual(gateOffTools.filter((name) => profile.MESSAGING_TOOL_NAMES.includes(name)), []);
});

test("messaging child environment is present only when explicitly configured", () => {
  assert.deepEqual(profile.messagingChildEnvironment({}), {});
  assert.deepEqual(profile.messagingChildEnvironment({ STRAYLIGHT_MESSAGING_ENABLED: "true" }), {
    STRAYLIGHT_MESSAGING_ENABLED: "true",
  });
  assert.deepEqual(profile.messagingChildEnvironment({ STRAYLIGHT_MESSAGING_ENABLED: "false" }), {
    STRAYLIGHT_MESSAGING_ENABLED: "false",
  });
  assert.deepEqual(profile.messagingChildEnvironment({ STRAYLIGHT_MESSAGING_ENABLED: "" }), {
    STRAYLIGHT_MESSAGING_ENABLED: "",
  });
});

test("remote tools retain the exact gate-off inventory", () => {
  const existingTools = Object.freeze(["asset.list", "memory.open", "task.update"]);
  const expected = profile.expectedRemoteToolNames(existingTools, {});

  assert.strictEqual(expected, existingTools);
  assert.deepEqual(expected, ["asset.list", "memory.open", "task.update"]);
});

test("remote tools add exactly the five messaging tools only for literal true", () => {
  const existingTools = ["asset.list", "memory.open", "task.update"];
  const enabled = profile.expectedRemoteToolNames(existingTools, {
    STRAYLIGHT_MESSAGING_ENABLED: "true",
  });

  assert.deepEqual(enabled, [
    "agent.list",
    "asset.list",
    "memory.open",
    "message.list",
    "message.read",
    "message.send",
    "message.wait",
    "task.update",
  ]);
  assert.deepEqual(profile.MESSAGING_TOOL_NAMES, [
    "agent.list",
    "message.list",
    "message.read",
    "message.send",
    "message.wait",
  ]);
  assert.strictEqual(
    profile.expectedRemoteToolNames(existingTools, {
      STRAYLIGHT_MESSAGING_ENABLED: "TRUE",
    }),
    existingTools,
  );
  assert.strictEqual(
    profile.expectedRemoteToolNames(existingTools, {
      STRAYLIGHT_MESSAGING_ENABLED: "false",
    }),
    existingTools,
  );
});
