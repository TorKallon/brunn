import { expect, test, type Page, type TestInfo } from "@playwright/test";

interface ConversationReceipt {
  data: { conversation_id: string };
}

interface SendReceipt {
  data: {
    conversation_id: string;
    seq: number;
    message: { from_agent_id: string };
  };
}

const email = requiredEnvironment("STRAYLIGHT_GATE12_EMAIL");
const password = requiredEnvironment("STRAYLIGHT_GATE12_PASSWORD");
const baseUrl = requiredEnvironment("STRAYLIGHT_GATE12_BASE_URL");
const reboundCredentialName = requiredEnvironment(
  "STRAYLIGHT_GATE12_REBIND_CREDENTIAL_NAME",
);
const reboundCredential = requiredEnvironment(
  "STRAYLIGHT_GATE12_REBIND_CREDENTIAL",
);
const echoAgentId = process.env.STRAYLIGHT_GATE12_ECHO_AGENT_ID?.trim() || "echo";
const echoDisplayName =
  process.env.STRAYLIGHT_GATE12_ECHO_DISPLAY_NAME?.trim() || "Echo";

test("gate 12d: authenticated Web messaging and credential-derived sender", async ({
  page,
}, testInfo) => {
  await signIn(page);
  const primaryNavigation = page.getByRole("navigation", {
    name: "Primary navigation",
  });
  const agentsLink = primaryNavigation.getByRole("link", {
    name: "Agents",
    exact: true,
  });
  await expect(agentsLink).toBeVisible();
  await agentsLink.click();
  await expect(page).toHaveURL(/\/agents$/u);
  await expect(page.getByRole("heading", { name: "Agents" })).toBeVisible();

  const run = `${Date.now()}-${testInfo.workerIndex}`;
  const subject = `Gate 12d echo ${run}`;
  const question = `Gate 12d question ${run}`;
  const credentialMessage = `Gate 12d rebound sender ${run}`;

  await page.getByRole("button", { name: "New conversation" }).click();
  const picker = page.getByRole("dialog", { name: "New conversation" });
  await expect(picker).toBeVisible();
  await picker.getByRole("checkbox", { name: new RegExp(echoDisplayName, "iu") }).check();
  await picker.getByLabel("Subject").fill(subject);
  const createResponse = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      response.url().endsWith("/api/v1/workspace/messaging/conversations"),
  );
  await picker.getByRole("button", { name: "Create conversation" }).click();
  const created = await createResponse;
  expect(created.ok()).toBe(true);
  const conversationId = ((await created.json()) as ConversationReceipt).data
    .conversation_id;
  expect(conversationId).toMatch(
    /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u,
  );

  await expect(
    page.getByRole("heading", { name: subject, exact: true }),
  ).toBeVisible();
  await page.getByLabel("Kind").selectOption("question");
  await page.getByLabel("Message").fill(question);
  const ownerSendResponse = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      response.url().endsWith(
        `/api/v1/workspace/messaging/conversations/${conversationId}/messages`,
      ),
  );
  await page.getByRole("button", { name: "Send", exact: true }).click();
  const ownerSend = await ownerSendResponse;
  expect(ownerSend.ok()).toBe(true);
  expect(ownerSend.request().postDataJSON()).not.toHaveProperty("from");
  await expect(page.getByText(question, { exact: true })).toBeVisible();
  const echoReply = page
    .getByRole("listitem")
    .filter({ hasText: "Acknowledged." });
  await expect(echoReply).toBeVisible({ timeout: 45_000 });
  await expect(echoReply.locator("header strong")).toHaveText(echoDisplayName);

  await page.getByText("Registry settings", { exact: true }).click();
  const echoSettings = page.getByRole("group", {
    name: `${echoDisplayName} settings`,
  });
  await expect(echoSettings).toBeVisible();
  await echoSettings.getByLabel("Credential").selectOption({
    label: reboundCredentialName,
  });
  const bindingResponse = page.waitForResponse(
    (response) =>
      response.request().method() === "PUT" &&
      response.url().endsWith(
        `/api/v1/workspace/messaging/agents/${encodeURIComponent(echoAgentId)}/credential`,
      ),
  );
  await echoSettings.getByRole("button", { name: "Apply binding" }).click();
  const appliedBinding = await bindingResponse;
  expect(appliedBinding.ok()).toBe(true);

  // Keep the bearer request outside Playwright's page/request tracing so a
  // retained failure trace cannot capture the credential header.
  const reboundSend = await fetch(
    new URL(
      `/api/v1/workspace/messaging/conversations/${conversationId}/messages`,
      baseUrl,
    ),
    {
      method: "POST",
      headers: {
        Accept: "application/json",
        Authorization: `Bearer ${reboundCredential}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        client_key: newClientKey(),
        kind: "text",
        body_md: credentialMessage,
        refs: [],
        expects_reply: false,
      }),
    },
  );
  expect(reboundSend.ok).toBe(true);
  const reboundReceipt = (await reboundSend.json()) as SendReceipt;
  expect(reboundReceipt.data.message.from_agent_id).toBe(echoAgentId);

  const reboundRow = page
    .getByRole("listitem")
    .filter({ hasText: credentialMessage });
  await expect(reboundRow).toBeVisible({ timeout: 30_000 });
  await expect(reboundRow.locator("header strong")).toHaveText(echoDisplayName);

  const closeResponse = await browserSessionJson(
    page,
    "POST",
    `/api/v1/workspace/messaging/conversations/${conversationId}/close`,
    {},
  );
  expect(closeResponse.ok).toBe(true);
  await testInfo.attach("gate12d-run-evidence", {
    body: Buffer.from(
      JSON.stringify(
        {
          conversation_created: true,
          owner_send_status: ownerSend.status(),
          echo_reply_observed: true,
          credential_binding_status: appliedBinding.status(),
          rebound_send_status: reboundSend.status,
          server_derived_sender_verified: true,
          close_status: closeResponse.status,
        },
        null,
        2,
      ),
    ),
    contentType: "application/json",
  });
});

async function signIn(page: Page): Promise<void> {
  await page.goto("/login");
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page).toHaveURL(/\/dashboard$/u);
  await expect(
    page.getByRole("navigation", { name: "Primary navigation" }),
  ).toBeVisible();
}

async function browserSessionJson(
  page: Page,
  method: "POST" | "PUT",
  path: string,
  body: unknown,
): Promise<{ ok: boolean; status: number }> {
  return page.evaluate(
    async ({ requestMethod, requestPath, requestBody }) => {
      const csrfCookie = document.cookie
        .split(";")
        .map((value) => value.trim())
        .find(
          (value) =>
            value.startsWith("__Host-straylight_csrf=") ||
            value.startsWith("straylight_csrf="),
        );
      if (!csrfCookie) {
        throw new Error("The signed-in page did not receive a CSRF cookie");
      }
      const csrf = decodeURIComponent(
        csrfCookie.slice(csrfCookie.indexOf("=") + 1),
      );
      const response = await fetch(requestPath, {
        method: requestMethod,
        credentials: "same-origin",
        headers: {
          Accept: "application/json",
          "Content-Type": "application/json",
          "X-CSRF-Token": csrf,
        },
        body: JSON.stringify(requestBody),
      });
      return { ok: response.ok, status: response.status };
    },
    { requestMethod: method, requestPath: path, requestBody: body },
  );
}

function newClientKey(): string {
  const alphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  const random = new Uint8Array(10);
  crypto.getRandomValues(random);
  let value = BigInt(Date.now());
  for (const byte of random) value = (value << 8n) | BigInt(byte);
  let encoded = "";
  for (let index = 0; index < 26; index += 1) {
    encoded = alphabet[Number(value & 31n)] + encoded;
    value >>= 5n;
  }
  return encoded;
}

function requiredEnvironment(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) {
    throw new Error(`${name} is required for the disposable-stack Gate 12d run`);
  }
  return value;
}
