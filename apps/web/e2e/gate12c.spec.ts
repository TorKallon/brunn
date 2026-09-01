import { expect, test, type Page, type TestInfo } from "@playwright/test";

interface CaptureReceipt {
  status: string;
  data: {
    items: Array<{ task_ref: string; title: string }>;
  };
}

interface ContextReceipt {
  status: string;
  data: {
    context: { slug: string; display_name: string };
  };
}

const email = requiredEnvironment("BRUNN_GATE12_EMAIL");
const password = requiredEnvironment("BRUNN_GATE12_PASSWORD");

test("gate 12c: authenticated agent-first Web flow", async ({ page }, testInfo) => {
  await signIn(page);
  await expect(page.getByText("Nothing urgent", { exact: true })).toBeVisible();
  await expect(page.getByRole("region", { name: "Urgent tasks" })).toHaveCount(0);
  await expect(
    page.getByRole("navigation", { name: "Primary navigation" }).getByRole("link", {
      name: "Tasks",
      exact: true,
    }),
  ).toHaveCount(0);

  const run = `${Date.now()}-${testInfo.workerIndex}`;
  const completeTitle = `Gate 12 complete ${run}`;
  const snoozeTitle = `Gate 12 snooze ${run}`;
  const confirmTitle = `Gate 12 inferred ${run}`;
  const taskItems = taskSeedItems(run, completeTitle, snoozeTitle, confirmTitle);
  const firstCapture = await apiJson<CaptureReceipt>(
    page,
    "POST",
    "/api/v1/workspace/tasks/capture",
    {
      idempotency_key: operationId("gate12c_capture_a"),
      items: taskItems.slice(0, 25),
    },
  );
  const secondCapture = await apiJson<CaptureReceipt>(
    page,
    "POST",
    "/api/v1/workspace/tasks/capture",
    {
      idempotency_key: operationId("gate12c_capture_b"),
      items: taskItems.slice(25),
    },
  );
  const captured = [...firstCapture.data.items, ...secondCapture.data.items];
  expect(captured).toHaveLength(30);
  const completeRef = taskRefFor(captured, completeTitle);
  const snoozeRef = taskRefFor(captured, snoozeTitle);
  const confirmRef = taskRefFor(captured, confirmTitle);

  const contextSource = await createContext(page, `Gate 12 source ${run}`);
  const contextTarget = await createContext(page, `Gate 12 target ${run}`);

  await page.goto("/dashboard");
  await expect(page.getByRole("heading", { name: "What needs your attention" })).toBeVisible();
  const urgent = page.getByRole("region", { name: "Urgent tasks" });
  const next = page.getByRole("region", { name: "Next tasks" });
  await expect(urgent).toBeVisible();
  await expect(urgent.getByTestId("task-row")).toHaveCount(3);
  await expect(next.getByTestId("task-row")).toHaveCount(2);
  await expect.poll(() => dashboardTaskCount(urgent, next)).toBe(5);
  await expect(urgent.getByText("hard deadline", { exact: false }).first()).toBeVisible();
  await expect(urgent.getByLabel(/^Inferred by agent:/)).toBeVisible();

  await quickAction(page, urgent, completeRef, "Complete", "Task complete");
  const done = page.getByRole("region", { name: "Done today" });
  await expect(done.locator("header strong")).toHaveText("1");
  await expect(done.getByRole("listitem")).toHaveCount(0);
  await done.getByRole("button", { name: "Show completed tasks" }).click();
  await expect(done.getByRole("listitem").filter({ hasText: completeTitle })).toBeVisible();
  await quickAction(page, urgent, snoozeRef, "Snooze one day", "Task snoozed");
  await quickAction(
    page,
    urgent,
    confirmRef,
    "Confirm hard deadline",
    "Hard deadline confirmed",
  );

  await next.getByRole("button", { name: "5 more" }).click();
  await expect.poll(() => dashboardTaskCount(urgent, next)).toBe(10);
  expect(await dashboardTaskCount(urgent, next)).toBeLessThanOrEqual(10);
  await next.getByRole("link", { name: "Show all" }).click();
  await expect(page.getByRole("heading", { name: "All tasks" })).toBeVisible();
  const allTasks = page.getByRole("region", { name: "Filtered tasks" });
  await expect(allTasks.getByTestId("task-row")).toHaveCount(25);
  await page.getByRole("button", { name: "Next page" }).click();
  await expect(page.getByText(/^Page 2 ·/)).toBeVisible();
  await expect(allTasks.getByTestId("task-row").first()).toBeVisible();
  await page.getByRole("button", { name: "Previous page" }).click();
  await expect(page.getByText(/^Page 1 ·/)).toBeVisible();

  await page.getByRole("combobox", { name: "Status", exact: true }).selectOption("open");
  await page.getByRole("combobox", { name: "Project", exact: true }).selectOption("brunn");
  await page.getByRole("combobox", { name: "Context", exact: true }).selectOption("online");
  await page.getByRole("combobox", { name: "Date type", exact: true }).selectOption("hard");
  await page.getByRole("combobox", { name: "Source", exact: true }).selectOption("owner");
  await expect(allTasks.getByRole("link", { name: snoozeTitle })).toBeVisible();
  await page.getByRole("checkbox", { name: "Include parked" }).check();
  await page.getByRole("checkbox", { name: "Include waiting" }).check();
  await expect(allTasks.getByRole("link", { name: confirmTitle })).toBeVisible();

  await page.goto("/settings");
  await expect(page.getByRole("heading", { name: "Contexts" })).toBeVisible();
  await page.getByLabel("Merge from").selectOption(contextSource.slug);
  await page.getByLabel("Merge into").selectOption(contextTarget.slug);
  const mergeResponse = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      response.url().endsWith("/api/v1/workspace/contexts/merge"),
  );
  await page.getByRole("button", { name: "Merge contexts" }).click();
  expect((await mergeResponse).ok()).toBe(true);
  await expect(
    page.getByLabel("Merge from").locator(`option[value="${contextSource.slug}"]`),
  ).toHaveCount(0);

  await expect(page.getByRole("heading", { name: "Todoist inlet" })).toBeVisible();
  await expect(page.getByText("Environment kill switch is off")).toBeVisible();
  await expect(page.getByRole("button", { name: "Pull now" })).toBeDisabled();

  await attachRunEvidence(testInfo, {
    captured_task_refs: captured.map((item) => item.task_ref),
    action_task_refs: { complete: completeRef, snooze: snoozeRef, confirm: confirmRef },
    merged_contexts: { from: contextSource.slug, into: contextTarget.slug },
  });
});

async function dashboardTaskCount(
  urgent: ReturnType<Page["getByRole"]>,
  next: ReturnType<Page["getByRole"]>,
): Promise<number> {
  return (await urgent.getByTestId("task-row").count()) +
    (await next.getByTestId("task-row").count());
}

async function signIn(page: Page): Promise<void> {
  await page.goto("/login");
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page).toHaveURL(/\/dashboard$/u);
  await expect(page.getByRole("navigation", { name: "Primary navigation" })).toBeVisible();
}

async function quickAction(
  page: Page,
  region: ReturnType<Page["getByRole"]>,
  taskRef: string,
  action: string,
  feedback: string,
): Promise<void> {
  const response = page.waitForResponse(
    (candidate) =>
      candidate.request().method() === "PATCH" &&
      candidate.url().endsWith(`/api/v1/workspace/tasks/${taskRef}`),
  );
  await region.getByTestId(`task-row-${taskRef}`).getByRole("button", { name: action }).click();
  expect((await response).ok()).toBe(true);
  await expect(page.getByRole("status").filter({ hasText: feedback })).toBeVisible();
}

async function createContext(
  page: Page,
  displayName: string,
): Promise<ContextReceipt["data"]["context"]> {
  const response = await apiJson<ContextReceipt>(
    page,
    "POST",
    "/api/v1/workspace/contexts",
    {
      display_name: displayName,
      aliases: [],
      source: "owner",
      confirm_new: true,
      idempotency_key: operationId("gate12c_context"),
    },
  );
  return response.data.context;
}

async function apiJson<T>(
  page: Page,
  method: "POST" | "PATCH" | "PUT",
  path: string,
  body: unknown,
): Promise<T> {
  return page.evaluate(
    async ({ requestMethod, requestPath, requestBody }) => {
      const csrfCookie = document.cookie
        .split(";")
        .map((value) => value.trim())
        .find(
          (value) =>
            value.startsWith("__Host-brunn_csrf=") ||
            value.startsWith("brunn_csrf="),
        );
      if (!csrfCookie) throw new Error("The signed-in page did not receive a CSRF cookie");
      const csrf = decodeURIComponent(csrfCookie.slice(csrfCookie.indexOf("=") + 1));
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
      const payload = await response.json();
      if (!response.ok) {
        throw new Error(`${requestMethod} ${requestPath} failed (${response.status}): ${JSON.stringify(payload)}`);
      }
      return payload;
    },
    { requestMethod: method, requestPath: path, requestBody: body },
  ) as Promise<T>;
}

function taskSeedItems(
  run: string,
  completeTitle: string,
  snoozeTitle: string,
  confirmTitle: string,
) {
  const due = (hours: number) => new Date(Date.now() + hours * 60 * 60 * 1000).toISOString();
  const common = {
    project: { value: "brunn", source: "owner" },
    required_contexts: { value: ["online"], source: "owner" },
  };
  const hard = (raw_text: string, hours: number, source: string) => ({
    raw_text,
    captured_from: `gate12c:${run}`,
    ...common,
    hard_due: { value: due(hours), source },
  });
  return [
    hard(completeTitle, 2, "owner"),
    hard(snoozeTitle, 3, "owner"),
    hard(confirmTitle, 4, "agent:gate12c"),
    ...Array.from({ length: 27 }, (_, index) => ({
      raw_text: `Gate 12 backlog ${run} ${String(index + 1).padStart(2, "0")}`,
      captured_from: `gate12c:${run}`,
      ...common,
    })),
  ];
}

function taskRefFor(
  items: CaptureReceipt["data"]["items"],
  title: string,
): string {
  const item = items.find((candidate) => candidate.title === title);
  if (!item) throw new Error(`Capture response omitted ${title}`);
  return item.task_ref;
}

function operationId(prefix: string): string {
  return `${prefix}_${crypto.randomUUID()}`;
}

async function attachRunEvidence(testInfo: TestInfo, value: unknown): Promise<void> {
  await testInfo.attach("gate12c-run-evidence", {
    body: Buffer.from(JSON.stringify(value, null, 2)),
    contentType: "application/json",
  });
}

function requiredEnvironment(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required for the disposable-stack Gate 12c run`);
  return value;
}
