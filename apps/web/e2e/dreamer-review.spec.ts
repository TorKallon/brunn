import { expect, test, type Page } from "@playwright/test";

const title = "Refresh the project summary with the approved milestone and its original supporting references, retaining the unresolved delivery date and the final condition that the owner must be able to read.";
const finalParagraph = "The final condition: retain the caveat until the owner confirms the delivery date. This is the complete end of the original proposal.";
const longPath = `sources/Projects/${"LongUnbrokenProjectName".repeat(10)}/Status.md`;
const proposal = {
  id: "2026-09-07/1", kind: "proposal", title,
  body_md: `${"The original proposal contains supporting context that must remain available for review. ".repeat(24)}\n\n${finalParagraph}\n\n\`${longPath}\``,
  why_md: "The original milestone changed.", uncertainty_md: "The date remains unconfirmed.",
  run_id: "2026-09-07", run_entry_ref: "entry:review-fixture", run_version: 3,
  candidate_hash: "fixture-original-hash", candidate: { after_md: "Reviewed milestone." },
  sources: [{ entry_ref: "entry:source-fixture", version: 4, path: longPath, excerpt: longPath }],
  status: "pending", reviewable: true, stale: false,
};
const question = {
  ...proposal, id: "2026-09-07/question-2", kind: "question", title: "Which delivery date should be used?", body_md: "Two sources give different dates.", candidate: null, reviewable: false,
};
const review = {
  available: true, mode: "report-only", paused: false,
  last_attempt: { date: "2026-09-07", outcome: "partial", detail: "Review is ready; pending work remains.", finished_at: "2026-09-07T18:00:00Z" },
  counts: { pending: 2, proposals: 1, questions: 1, approved_held: 0, applied: 0 },
  items: [proposal, question], history: [], decision_version: 7,
};

// Every API request is intercepted; this suite never reads personal content or
// records a decision against the server backing the supplied Web URL.
async function fixture(page: Page, theme: "dark" | "light" = "dark", failFirstDecision = false) {
  const posts: unknown[] = [];
  await page.addInitScript((appearance) => localStorage.setItem("brunn.appearance.v1", appearance), theme);
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const user = { id: "review-test", display_name: "Review test", email: "review@example.test" };
    let data: unknown;
    switch (path) {
      case "/api/v1/auth/session": data = { user, expires_at: "2099-01-01T00:00:00Z" }; break;
      case "/api/v1/me": data = { user, capabilities: ["open", "query", "read", "save", "credential:manage"], read_only: false, scopes: [] }; break;
      case "/api/v1/status": data = { status: "healthy", version: "review-test", dependencies: {} }; break;
      case "/api/v1/dreamer/review": data = review; break;
      case "/api/v1/dreamer/review/decisions":
        posts.push(route.request().postDataJSON());
        if (failFirstDecision && posts.length === 1) {
          await route.fulfill({ status: 503, json: { error: { code: "unavailable", message: "The response was interrupted." } } });
          return;
        }
        data = { application_status: "deferred", message: "Deferred safely." };
        break;
      default: await route.abort(); return;
    }
    await route.fulfill({ json: { status: "complete", data } });
  });
  await page.goto("/dreams");
  await expect(page.getByRole("button", { name: `Review ${title}`, exact: true })).toBeVisible();
  return posts;
}

async function noPageOverflow(page: Page) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
}

for (const theme of ["dark", "light"] as const) {
  test(`390px ${theme}: full proposal, clear navigation, preserved draft and no nested inbox scroll`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await fixture(page, theme);
    const row = page.getByRole("button", { name: `Review ${title}`, exact: true });
    await expect(row.getByText("Read full proposal")).toBeVisible();
    expect(await page.locator(".review-item-list").evaluate((element) => getComputedStyle(element).maxHeight)).toBe("none");
    await row.scrollIntoViewIfNeeded();
    await noPageOverflow(page);
    await page.screenshot({ path: testInfo.outputPath(`inbox-${theme}.png`) });
    await row.click();
    await expect(page.getByRole("heading", { name: "Pending review", exact: true })).toBeHidden();
    const back = page.getByRole("button", { name: "Back to inbox" });
    await expect(back).toBeInViewport();
    await expect(page.getByRole("heading", { name: title, exact: true })).toBeVisible();
    await expect(page.getByText(finalParagraph)).toBeVisible();
    await expect(page.getByText("Report-only · Approvals are held. No candidate is applied in this mode.")).toBeVisible();
    await noPageOverflow(page);
    await page.screenshot({ path: testInfo.outputPath(`detail-${theme}.png`) });
    await page.getByText(finalParagraph).scrollIntoViewIfNeeded();
    await expect(page.getByText(finalParagraph)).toBeInViewport();
    await expect(back).toBeInViewport();
    const comment = page.getByRole("textbox", { name: "Comment (optional)" });
    await comment.fill("Keep the original caveat.");
    await page.getByRole("button", { name: "Next review item" }).click();
    await expect(page.getByRole("heading", { name: question.title })).toBeInViewport();
    await page.getByRole("button", { name: "Previous review item" }).click();
    await expect(comment).toHaveValue("Keep the original caveat.");
    await back.click();
    await expect(row).toBeFocused();
    await expect(row).toBeInViewport();
    await row.click();
    await expect(comment).toHaveValue("Keep the original caveat.");
  });
}

test("desktop keeps the inbox beside the complete selected proposal", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await fixture(page);
  await page.getByRole("button", { name: `Review ${title}`, exact: true }).click();
  await expect(page.getByRole("heading", { name: "Pending review", exact: true })).toBeVisible();
  await expect(page.getByText(finalParagraph)).toBeVisible();
  await expect(page.getByRole("button", { name: "Back to inbox" })).toBeHidden();
  const inbox = await page.locator(".review-inbox").boundingBox();
  const detail = await page.locator(".review-detail-wrap").boundingBox();
  expect(inbox!.x + inbox!.width).toBeLessThan(detail!.x);
  await noPageOverflow(page);
});

test("mobile keeps an uncertain decision in place and retries its exact identity", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const posts = await fixture(page, "dark", true);
  await page.getByRole("button", { name: `Review ${title}`, exact: true }).click();
  await page.getByRole("button", { name: "Defer", exact: true }).click();
  await expect(page.getByRole("button", { name: "Retry", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Back to inbox" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Next review item" })).toBeDisabled();
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(page.getByText("Deferred safely.")).toBeVisible();
  expect(posts).toHaveLength(2);
  expect(posts[1]).toEqual(posts[0]);
});
