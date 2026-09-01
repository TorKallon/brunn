export const MESSAGING_TOOL_NAMES = Object.freeze([
  "agent.list",
  "message.list",
  "message.read",
  "message.send",
  "message.wait",
]);

export function messagingChildEnvironment(environment) {
  const value = environment.BRUNN_MESSAGING_ENABLED;
  return value === undefined ? {} : { BRUNN_MESSAGING_ENABLED: value };
}

export function expectedRemoteToolNames(existingTools, environment) {
  if (environment.BRUNN_MESSAGING_ENABLED !== "true") {
    return existingTools;
  }
  return [...existingTools, ...MESSAGING_TOOL_NAMES].sort();
}
