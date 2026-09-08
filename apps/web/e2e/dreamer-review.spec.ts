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
const originalReport = `Applies next run unless vetoed: Retain this older planning note.\n\n${"Original report context, preserved as evidence rather than pending work. ".repeat(30)}\n\nFinal historical caveat, kept in full.`;
const legacyItems = Array.from({ length: 63 }, (_, index) => ({
  ...proposal, id: `older-report-${index + 1}`, legacy: true, kind: index % 2 ? "question" : "proposal",
  title: `Applies next run unless vetoed: Older planning note ${index + 1}`,
  body_md: originalReport, candidate: null, reviewable: false,
}));
const review = {
  available: true, mode: "report-only", paused: false,
  last_attempt: { date: "2026-09-07", outcome: "partial", detail: "Review is ready; pending work remains.", finished_at: "2026-09-07T18:00:00Z" },
  counts: { pending: 2, proposals: 1, questions: 1, approved_held: 0, applied: 0, legacy: legacyItems.length },
  items: [proposal, question], legacy_items: legacyItems, history: [], decision_version: 7,
};

// Every API request is intercepted; this suite never reads personal content or
// records a decision against the server backing the supplied Web URL.
async function fixture(page: Page, theme: "dark" | "light" = "dark", failFirstDecision = false, reviewState: unknown = review) {
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
      case "/api/v1/dreamer/review": data = reviewState; break;
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
  await expect(page.locator(".review-inbox").getByRole("button", { name: /^Review / }).first()).toBeVisible();
  return posts;
}

async function noPageOverflow(page: Page) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
}

for (const theme of ["dark", "light"] as const) {
  test(`390px ${theme}: compact timeline leads directly to decisions with thirty evidence rows collapsed`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width: 390, height: 844 });
    const shortTitle = "Review the daily location timeline";
    const caveat = "The travel interval remains uncertain.";
    const stops = [
      ["08:00–09:00", "Home"], ["09:00–09:20", "Station"], ["09:20–12:00", "Office"],
      ["12:00–13:00", "Café"], ["13:00–17:00", "Office"], ["17:00–18:00", "Park"], ["18:00–20:00", "Home"],
    ];
    const timeline = `| When | Where |\n| --- | --- |\n${stops.map(([when, where]) => `| ${when} | ${where} |`).join("\n")}\n\n${caveat}`;
    const sources = Array.from({ length: 30 }, (_, index) => ({
      entry_ref: index < 9 ? `entry:exact-source-${index}` : `location-report:raw-${index}`,
      version: index < 9 ? index + 1 : null, label: `Exact source ${index + 1}`,
      excerpt: `${"Original audit fields and observation details. ".repeat(40)}Final source detail ${index + 1}.`,
    }));
    const posts = await fixture(page, theme, false, { ...review, items: [{ ...proposal, title: shortTitle,
      body_md: "Review the observed timeline.", why_md: "  ", uncertainty_md: caveat,
      candidate: { target_path: "derived/location/2026-09-07.md", before_md: "Old location summary.", after_md: timeline, body_md: timeline }, sources,
    }], counts: { ...review.counts, pending: 1, proposals: 1, questions: 0 } });
    await page.getByRole("button", { name: `Review ${shortTitle}`, exact: true }).click();
    const evidence = page.locator(".review-evidence");
    await expect(evidence).not.toHaveAttribute("open");
    await expect(evidence.locator("summary")).toHaveText("Supporting evidence30");
    await expect(evidence.locator("blockquote").last()).toBeHidden();
    await expect(page.getByRole("heading", { name: "Why it is proposed" })).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "Uncertainty", exact: true })).toHaveCount(0);
    await expect(page.getByText(caveat, { exact: true })).toHaveCount(1);
    const table = page.getByRole("table");
    await expect(table).toBeVisible();
    await expect(table.getByRole("columnheader", { name: "When", exact: true })).toBeVisible();
    await expect(table.getByRole("columnheader", { name: "Where", exact: true })).toBeVisible();
    await expect(table.getByRole("row")).toHaveCount(8);
    for (const [when, where] of stops) await expect(table.getByRole("row").filter({ has: page.getByRole("cell", { name: when, exact: true }) }).getByRole("cell")).toHaveText([when, where]);
    await expect(page.locator(".review-exact-changes")).not.toHaveAttribute("open");
    await expect(page.getByText("Old location summary.", { exact: true })).toBeHidden();
    const decision = page.getByRole("heading", { name: "Your decision", exact: true });
    const tableBounds = (await table.boundingBox())!;
    expect((await decision.boundingBox())!.y - tableBounds.y - tableBounds.height).toBeLessThan(350);
    await table.scrollIntoViewIfNeeded();
    await page.screenshot({ path: testInfo.outputPath(`rendered-timeline-${theme}.png`) });
    await decision.scrollIntoViewIfNeeded();
    await page.getByRole("button", { name: "Approve", exact: true }).scrollIntoViewIfNeeded();
    await expect(page.getByRole("button", { name: "Approve", exact: true })).toBeInViewport();
    await noPageOverflow(page);
    await page.screenshot({ path: testInfo.outputPath(`compact-evidence-${theme}.png`) });
    await evidence.locator("summary").click();
    await expect(evidence).toHaveAttribute("open");
    await expect(evidence.locator("blockquote")).toHaveCount(30);
    await expect(evidence.getByRole("link", { name: "Exact source 9", exact: true })).toHaveAttribute("href", /version=9/);
    await expect(evidence.getByText("Exact source 30", { exact: true })).toBeVisible();
    await expect(evidence.getByRole("link", { name: "Exact source 30", exact: true })).toHaveCount(0);
    await expect(evidence.locator("blockquote").last()).toHaveText(sources[29].excerpt);
    await noPageOverflow(page);
    await evidence.locator("summary").click();
    await page.getByText("View exact changes", { exact: true }).click();
    await expect(page.getByText("Old location summary.", { exact: true })).toBeVisible();
    await expect(page.locator(".review-exact-changes pre").last()).toHaveText(timeline);
    expect(posts).toHaveLength(0);
  });

  test(`390px ${theme}: 63 older notes stay outside pending review and open as full read-only reports`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width: 390, height: 844 });
    const posts = await fixture(page, theme);
    await expect(page.locator(".review-inbox").getByRole("button", { name: /^Review / })).toHaveCount(2);
    await expect(page.getByRole("button", { name: "Needs your call 1" })).toBeVisible();
    await expect(page.getByText("2 items", { exact: true })).toBeVisible();
    const history = page.locator(".review-older-reports");
    await expect(history).not.toHaveAttribute("open");
    await expect(history.getByRole("button").first()).toBeHidden();
    await expect(history.getByText("63 notes", { exact: true })).toBeVisible();
    await history.scrollIntoViewIfNeeded();
    await noPageOverflow(page);
    await page.screenshot({ path: testInfo.outputPath(`older-reports-collapsed-${theme}.png`) });
    await history.locator("summary").click();
    const first = page.getByRole("button", { name: "Read older report Older planning note 1", exact: true });
    await first.click();
    await expect(page.getByRole("heading", { name: "Older planning note 1", exact: true })).toBeInViewport();
    await expect(page.locator(".review-original-text")).toHaveText(originalReport);
    await expect(page.getByRole("button", { name: "Back to older reports" })).toBeInViewport();
    await expect(page.getByRole("article").getByText("Pending", { exact: true })).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "Candidate", exact: true })).toHaveCount(0);
    for (const action of ["Approve", "Reject", "Defer", "Save correction"]) await expect(page.getByRole("button", { name: action, exact: true })).toHaveCount(0);
    await noPageOverflow(page);
    await page.screenshot({ path: testInfo.outputPath(`older-report-${theme}.png`) });
    await page.getByRole("button", { name: "Next older report" }).click();
    await expect(page.getByRole("heading", { name: "Older planning note 2", exact: true })).toBeInViewport();
    await page.getByRole("button", { name: "Previous older report" }).click();
    await page.getByRole("button", { name: "Back to older reports" }).click();
    await expect(first).toBeFocused();
    await expect(first).toBeInViewport();
    await history.locator("summary").click();
    await expect(history).not.toHaveAttribute("open");
    expect(posts).toHaveLength(0);
  });

  test(`390px ${theme}: full proposal, clear navigation, preserved draft and no nested inbox scroll`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await fixture(page, theme);
    const row = page.getByRole("button", { name: `Review ${title}`, exact: true });
    await expect(row.getByText("Read full proposal")).toBeVisible();
    expect(await page.locator(".review-inbox .review-item-list").evaluate((element) => getComputedStyle(element).maxHeight)).toBe("none");
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
