import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { DreamerReviewData, DreamerReviewItem } from "../lib/dreamerReview";
import { formatDate } from "../lib/format";
import { defaultMe, installApiMock, renderApp } from "./renderApp";

const candidate: DreamerReviewItem = {
  id: "2026-09-07-proposed-1",
  kind: "proposal",
  title: "Refresh the project summary",
  body_md: "Bring the summary up to date with the approved milestone.",
  why_md: "The project milestone changed in the source note.",
  uncertainty_md: "The delivery date is still unresolved.",
  run_id: "2026-09-07",
  run_entry_ref: "entry:run-one",
  run_version: 3,
  candidate_hash: "sha256:candidate-one",
  candidate: { target_path: "dreams/summaries/project.md", before_md: "Old milestone.", after_md: "Reviewed milestone." },
  sources: [{ entry_ref: "entry:source-one", version: 4, path: "Projects/Status.md", excerpt: "Milestone was approved." }],
  status: "pending",
  reviewable: true,
  stale: false,
};
const question: DreamerReviewItem = {
  ...candidate, id: "2026-09-07-question-1", kind: "question", title: "Which date should be used?", body_md: "Two source notes give different dates.", candidate: null,
  candidate_hash: "sha256:question-one", reviewable: false, blocked_reason: "An answer is needed before a candidate can be compiled.",
};
function reviewData(overrides: Partial<DreamerReviewData> = {}): DreamerReviewData {
  return {
    available: true, mode: "report-only", paused: false,
    last_attempt: { date: "2026-09-07", outcome: "partial", detail: "Report saved; publication remains pending." },
    last_successful_run: { run_id: "2026-09-06", entry_ref: "entry:run-previous", version: 2 },
    counts: { pending: 2, proposals: 1, questions: 1, approved_held: 0, applied: 0 },
    items: [candidate, question], history: [], decision_version: 7,
    ...overrides,
  };
}
const envelope = (data: DreamerReviewData) => ({ status: "complete", data });

describe("Dreamer Review inbox", () => {
  it("opens the complete proposal and keeps notes through previous/next navigation", async () => {
    const ending = "The final instruction and its uncertainty must remain readable.";
    const fullTitle = "Refresh the project summary using the exact approved milestone, including the original source references and the unresolved delivery date, so the owner can understand the entire proposal.";
    const longCandidate = { ...candidate, title: fullTitle, body_md: `${"Supporting context that belongs to the original proposal. ".repeat(30)}\n\n${ending}` };
    installApiMock({ "GET /api/v1/dreamer/review": envelope(reviewData({ items: [longCandidate, question] })) });
    const user = userEvent.setup();
    renderApp("/dreams");
    const row = await screen.findByRole("button", { name: `Review ${fullTitle}` });
    expect(within(row).getByText("Read full proposal")).toBeInTheDocument();
    await user.click(row);
    expect(screen.getByRole("heading", { name: fullTitle })).toHaveTextContent(fullTitle);
    expect(screen.getByText(ending)).toBeInTheDocument();
    expect(screen.getByText("Report-only: your approvals are saved and held. Nothing is written until Dreaming is switched to full mode.")).toBeInTheDocument();
    expect(screen.getByText(/^Saves your approval and holds it\./)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Previous review item" })).toBeDisabled();
    await user.type(screen.getByRole("textbox", { name: "Comment (optional)" }), "Keep the original caveat.");
    await user.click(screen.getByRole("button", { name: "Next review item" }));
    expect(screen.getByRole("heading", { name: question.title })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Next review item" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Previous review item" }));
    expect(screen.getByRole("textbox", { name: "Comment (optional)" })).toHaveValue("Keep the original caveat.");
  });

  it("shows completion times that distinguish same-day retries", async () => {
    const firstAt = "2026-09-07T18:05:00Z";
    const retryAt = "2026-09-07T19:25:00Z";
    let state = reviewData({ last_attempt: { date: "2026-09-07", outcome: "partial", started_at: "2026-09-07T18:00:00Z", finished_at: firstAt } });
    installApiMock({ "GET /api/v1/dreamer/review": () => envelope(state) });
    const user = userEvent.setup();
    renderApp("/dreams");
    const firstTime = await screen.findByText(formatDate(firstAt));
    expect(firstTime).toHaveAttribute("datetime", firstAt);
    expect(screen.queryByText("2026-09-07", { selector: "strong" })).not.toBeInTheDocument();
    state = reviewData({ last_attempt: { date: "2026-09-07", outcome: "completed", started_at: "2026-09-07T19:00:00Z", finished_at: retryAt } });
    await user.click(screen.getByRole("button", { name: "Refresh" }));
    expect(await screen.findByText(formatDate(retryAt))).toHaveAttribute("datetime", retryAt);
    expect(screen.queryByText(formatDate(firstAt))).not.toBeInTheDocument();
  });

  it.each([
    { name: "start time for an unfinished attempt", attempt: { finished_at: null, started_at: "2026-09-07T19:00:00Z" }, expected: "2026-09-07T19:00:00Z" },
    { name: "historical date without timestamps", attempt: {}, expected: undefined },
  ])("uses $name in the last-attempt card", async ({ attempt, expected }) => {
    installApiMock({ "GET /api/v1/dreamer/review": envelope(reviewData({ last_attempt: { date: "2026-09-07", outcome: "partial", ...attempt } })) });
    renderApp("/dreams");
    const text = await screen.findByText(expected ? formatDate(expected) : "2026-09-07", { selector: expected ? "time" : "strong" });
    if (expected) expect(text).toHaveAttribute("datetime", expected);
    else expect(text.querySelector("time")).toBeNull();
  });

  it("separates run status, proposals, and questions, then records an exact held approval", async () => {
    let state = reviewData();
    let payload: Record<string, unknown> | undefined;
    const fetchMock = installApiMock({
      "GET /api/v1/dreamer/review": () => envelope(state),
      "POST /api/v1/dreamer/review/decisions": async (request: Request) => {
        payload = await request.json();
        const decision = { id: "decision-one", item_id: candidate.id, decision: "approve" as const, comment: "The source is correct.", at: "2026-09-07T18:00:00Z", application_status: "approved_held" };
        state = { ...state, decision_version: 8, history: [decision], items: [question], counts: { ...state.counts, pending: 1, proposals: 0, approved_held: 1 } };
        return { status: "committed", data: { decision, application_status: "approved_held", message: "Approved and held by report-only mode." } };
      },
    });
    const user = userEvent.setup();
    renderApp("/dreams");
    expect(await screen.findByRole("heading", { name: "Review", level: 1 })).toBeInTheDocument();
    expect(await screen.findByText("Report saved; publication remains pending.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "2026-09-06" })).toHaveAttribute("href", expect.stringContaining("version=2"));
    await user.click(screen.getByRole("button", { name: "Needs your call 1" }));
    expect(screen.getByRole("button", { name: `Review ${question.title}` })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: `Review ${candidate.title}` })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "All 2" }));
    await user.click(screen.getByRole("button", { name: `Review ${candidate.title}` }));
    expect(screen.getByText("Old milestone.")).toBeInTheDocument();
    expect(screen.getByText("Reviewed milestone.")).toBeInTheDocument();
    expect(screen.getByText(candidate.uncertainty_md!)).toBeInTheDocument();
    await user.click(screen.getByText("Supporting evidence", { exact: true }));
    expect(screen.getByRole("link", { name: "Projects/Status.md" })).toHaveAttribute("href", expect.stringContaining("version=4"));
    await user.type(screen.getByRole("textbox", { name: "Comment (optional)" }), "The source is correct.");
    await user.click(screen.getByRole("button", { name: "Approve" }));
    expect(await screen.findByText("Approved and held by report-only mode.")).toBeInTheDocument();
    expect(screen.getByText("Approved · held. Applies at the first run after full mode is switched on; returns here if its sources change first.")).toBeInTheDocument();
    expect(payload).toEqual({ item_id: candidate.id, run_entry_ref: candidate.run_entry_ref, run_version: 3, candidate_hash: candidate.candidate_hash, expected_decisions_version: 7, decision: "approve", comment: "The source is correct.", idempotency_key: expect.any(String) });
    expect(await screen.findByText("Approved Held")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
    expect(fetchMock.mock.calls.some(([input]) => /\/api\/v1\/dreams(?:\/|$)/.test(String(input)))).toBe(false);
  });

  it("keeps full older reports collapsed separately from real candidates and owner questions, without decisions", async () => {
    const original = `Applies next run unless vetoed: Rewrite the project history.\n\n${"Original historical context. ".repeat(60)}\n\nFinal historical caveat.`;
    const legacy = { ...candidate, id: "older-prose", legacy: true, title: "Applies next run unless vetoed: Rewrite the project history.", body_md: original, candidate: { before_md: null, after_md: null, body_md: null }, reviewable: false };
    const fetchMock = installApiMock({ "GET /api/v1/dreamer/review": envelope(reviewData({ legacy_items: [legacy], counts: { ...reviewData().counts, legacy: 1 } })) });
    const user = userEvent.setup();
    renderApp("/dreams");
    expect(await screen.findByRole("button", { name: "All 2" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: `Review ${question.title}` })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: `Review ${legacy.title}` })).not.toBeInTheDocument();
    expect(screen.getByText("2 items")).toBeInTheDocument();
    const disclosure = screen.getByText("Older reports").closest("details")!;
    expect(disclosure).not.toHaveAttribute("open");
    expect(within(disclosure).getByText("1 note")).toBeInTheDocument();
    await user.click(screen.getByText("Older reports"));
    await user.click(screen.getByRole("button", { name: "Read older report Rewrite the project history." }));
    const detail = within(screen.getByRole("article", { name: "Older report Rewrite the project history." }));
    expect(detail.getByText(/Final historical caveat/, { selector: "pre" }).textContent).toBe(original);
    expect(detail.getByRole("link", { name: "2026-09-07 · v3" })).toHaveAttribute("href", expect.stringContaining("version=3"));
    expect(detail.getByText(/No decision is needed, and it is not scheduled to apply/)).toBeInTheDocument();
    expect(detail.queryByRole("heading", { name: "Candidate" })).not.toBeInTheDocument();
    expect(detail.queryByText("Pending", { exact: true })).not.toBeInTheDocument();
    for (const action of ["Approve", "Reject", "Defer", "Save correction"]) expect(screen.queryByRole("button", { name: action })).not.toBeInTheDocument();
    expect(fetchMock.mock.calls.some(([input, init]) => String(input).includes("/review/decisions") || (input instanceof Request && input.method === "POST") || init?.method === "POST")).toBe(false);
  });

  it.each(["kind", "import reason", "explicit marker"])("recognizes only the known %s legacy marker during an older API rollout", async (marker) => {
    const legacy: DreamerReviewItem = { ...question, id: "old-question", title: "An old report question?", ...(marker === "kind" ? { kind: "legacy" as const } : marker === "explicit marker" ? { legacy: true } : { why_md: "Retained from an earlier run; a concrete candidate is required before application." }) };
    installApiMock({ "GET /api/v1/dreamer/review": envelope(reviewData({ items: [legacy, candidate, question], counts: { pending: 3, proposals: 1, questions: 2, approved_held: 0, applied: 0 } })) });
    renderApp("/dreams");
    expect(await screen.findByRole("button", { name: "All 2" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Needs your call 1" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: `Review ${legacy.title}` })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: `Review ${question.title}` })).toBeInTheDocument();
    expect(screen.getByText("Older reports")).toBeInTheDocument();
  });

  it("shows an empty active inbox when only historical notes exist", async () => {
    installApiMock({ "GET /api/v1/dreamer/review": envelope(reviewData({ items: [], legacy_items: [{ ...candidate, legacy: true }], counts: { pending: 0, proposals: 0, questions: 0, approved_held: 0, applied: 0, legacy: 1 } })) });
    renderApp("/dreams");
    expect(await screen.findByText("Nothing waiting for review")).toBeInTheDocument();
    expect(screen.getByText("0 items")).toBeInTheDocument();
    expect(screen.getByText("Older reports")).toBeInTheDocument();
  });

  it("honors an explicit current classification instead of an older import fallback", async () => {
    installApiMock({ "GET /api/v1/dreamer/review": envelope(reviewData({ items: [{ ...question, legacy: false, why_md: "Retained from an earlier run; a concrete candidate is required before application." }] })) });
    renderApp("/dreams");
    expect(await screen.findByRole("button", { name: `Review ${question.title}` })).toBeInTheDocument();
    expect(screen.queryByText("Older reports")).not.toBeInTheDocument();
  });

  it("does not show an empty Candidate section for an active question", async () => {
    installApiMock({ "GET /api/v1/dreamer/review": envelope(reviewData({ items: [{ ...question, candidate: { target_path: "", body_md: " ", before_md: null, after_md: null } }] })) });
    const user = userEvent.setup();
    renderApp("/dreams");
    await user.click(await screen.findByRole("button", { name: `Review ${question.title}` }));
    expect(screen.queryByRole("heading", { name: "Candidate" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Approve" })).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Answer or correction" })).toBeVisible();
  });

  it("records a question correction and a deferral through the durable decision endpoint", async () => {
    const submitted: Record<string, unknown>[] = [];
    installApiMock({
      "GET /api/v1/dreamer/review": envelope(reviewData()),
      "POST /api/v1/dreamer/review/decisions": async (request: Request) => { const payload = await request.json(); submitted.push(payload); return { status: "committed", data: { application_status: payload.decision === "correct" ? "needs_changes" : "deferred", message: "Decision held." } }; },
    });
    const user = userEvent.setup();
    renderApp("/dreams");
    await user.click(await screen.findByRole("button", { name: `Review ${question.title}` }));
    const save = screen.getByRole("button", { name: "Save correction" });
    expect(save).toBeDisabled();
    await user.type(screen.getByRole("textbox", { name: "Answer or correction" }), "Use the date in the approved plan.");
    await user.click(save);
    expect(await screen.findByText("Decision held.")).toBeInTheDocument();
    expect(submitted[0]).toMatchObject({ item_id: question.id, decision: "correct", correction: "Use the date in the approved plan." });
    await user.click(screen.getByRole("button", { name: `Review ${candidate.title}` }));
    await user.click(screen.getByRole("button", { name: "Defer" }));
    await waitFor(() => expect(submitted[1]).toMatchObject({ item_id: candidate.id, decision: "defer" }));
  });

  it("removes decisions immediately when a selected item moves to history, preserving only an uncertain exact retry", async () => {
    let state = reviewData();
    const submitted: unknown[] = [];
    installApiMock({
      "GET /api/v1/dreamer/review": () => envelope(state),
      "POST /api/v1/dreamer/review/decisions": async (request: Request) => {
        submitted.push(await request.json());
        return submitted.length === 1 ? { status: 503, body: { error: { code: "unavailable", message: "Response was interrupted." } } } : { status: 409, body: { error: { code: "conflict", message: "This note belongs to older reports." } } };
      },
    });
    const user = userEvent.setup();
    renderApp("/dreams");
    await user.click(await screen.findByRole("button", { name: `Review ${candidate.title}` }));
    await user.click(screen.getByRole("button", { name: "Defer" }));
    await screen.findByRole("button", { name: "Retry" });
    state = reviewData({ items: [question], legacy_items: [{ ...candidate, legacy: true, candidate: null, reviewable: false }] });
    await user.click(screen.getByRole("button", { name: "Refresh" }));
    expect(await screen.findByRole("article", { name: `Older report ${candidate.title}` })).toBeInTheDocument();
    for (const action of ["Approve", "Reject", "Defer", "Save correction"]) expect(screen.queryByRole("button", { name: action })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(submitted).toHaveLength(2));
    expect(submitted[1]).toEqual(submitted[0]);
    await waitFor(() => expect(screen.queryByRole("button", { name: "Retry" })).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: `Review ${question.title}` })).toBeEnabled();
  });

  it("shows the refreshed candidate immediately and requires acknowledgment before deciding on the new version", async () => {
    let state = reviewData();
    const submitted: unknown[] = [];
    installApiMock({
      "GET /api/v1/dreamer/review": () => envelope(state),
      "POST /api/v1/dreamer/review/decisions": async (request: Request) => { submitted.push(await request.json()); return { status: "committed", data: { application_status: "approved_held", message: "New version approved and held." } }; },
    });
    const user = userEvent.setup();
    renderApp("/dreams");
    await user.click(await screen.findByRole("button", { name: `Review ${candidate.title}` }));
    await user.type(screen.getByRole("textbox", { name: "Comment (optional)" }), "Keep this note.");
    state = reviewData({ decision_version: 8, items: [{ ...candidate, title: "Updated proposal title", body_md: "Fresh proposal context.", candidate_hash: "sha256:updated", run_version: 4, candidate: { after_md: "A different candidate." } }] });
    await user.click(screen.getByRole("button", { name: "Refresh" }));
    expect(await screen.findByText("This review has changed.")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Updated proposal title" })).toBeInTheDocument();
    expect(screen.getByText("Fresh proposal context.")).toBeInTheDocument();
    expect(screen.getByText("A different candidate.")).toBeInTheDocument();
    expect(screen.queryByText("Reviewed milestone.")).not.toBeInTheDocument();
    expect(screen.queryByText(candidate.body_md)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
    expect(screen.getByRole("textbox", { name: "Comment (optional)" })).toHaveValue("Keep this note.");
    await user.click(screen.getByRole("button", { name: "Approve" }));
    expect(submitted).toHaveLength(0);
    await user.click(screen.getByRole("button", { name: "Review updated item" }));
    expect(screen.getByText("A different candidate.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Approve" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "Approve" }));
    await screen.findByText("New version approved and held.");
    expect(submitted[0]).toMatchObject({ item_id: candidate.id, candidate_hash: "sha256:updated", run_version: 4, expected_decisions_version: 8, decision: "approve", comment: "Keep this note." });
  });

  it("refreshes displayed content during an uncertain decision without changing the exact retry or its acknowledgment gate", async () => {
    let state = reviewData();
    const submitted: unknown[] = [];
    installApiMock({
      "GET /api/v1/dreamer/review": () => envelope(state),
      "POST /api/v1/dreamer/review/decisions": async (request: Request) => {
        submitted.push(await request.json());
        return submitted.length === 1 ? { status: 503, body: { error: { code: "unavailable", message: "Response interrupted." } } } : { status: 409, body: { error: { code: "stale_review", message: "The candidate changed." } } };
      },
    });
    const user = userEvent.setup();
    renderApp("/dreams");
    await user.click(await screen.findByRole("button", { name: `Review ${candidate.title}` }));
    await user.type(screen.getByRole("textbox", { name: "Comment (optional)" }), "My original note.");
    await user.click(screen.getByRole("button", { name: "Defer" }));
    await screen.findByRole("button", { name: "Retry" });
    state = reviewData({ decision_version: 9, items: [{ ...candidate, body_md: "Replacement while the earlier request is unconfirmed.", candidate_hash: "sha256:replacement", run_version: 5, candidate: { after_md: "Latest candidate content." } }] });
    await user.click(screen.getByRole("button", { name: "Refresh" }));
    expect(await screen.findByText("Latest candidate content.")).toBeInTheDocument();
    expect(screen.queryByText("Reviewed milestone.")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Review updated item" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Next review item" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    await screen.findByText("Decision needs another review");
    expect(submitted).toHaveLength(2);
    expect(submitted[1]).toEqual(submitted[0]);
    expect(submitted[1]).toMatchObject({ item_id: candidate.id, candidate_hash: candidate.candidate_hash, run_version: 3, expected_decisions_version: 7, comment: "My original note." });
    expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Review updated item" }));
    expect(screen.getByRole("button", { name: "Approve" })).toBeEnabled();
    expect(screen.getByRole("textbox", { name: "Comment (optional)" })).toHaveValue("My original note.");
    expect(screen.getByText("Latest candidate content.")).toBeInTheDocument();
  });

  it("blocks stale approval and preserves the exact request on an uncertain retry", async () => {
    const submitted: unknown[] = [];
    installApiMock({
      "GET /api/v1/dreamer/review": envelope(reviewData({ items: [{ ...candidate, stale: true }] })),
      "POST /api/v1/dreamer/review/decisions": async (request: Request) => {
        submitted.push(await request.json());
        return submitted.length === 1 ? { status: 503, body: { error: { code: "unavailable", message: "Response was interrupted." } } } : { status: "committed", data: { application_status: "deferred", message: "Deferred safely." } };
      },
    });
    const user = userEvent.setup();
    renderApp("/dreams");
    await user.click(await screen.findByRole("button", { name: `Review ${candidate.title}` }));
    expect(screen.getByText("Needs another review")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Defer" }));
    await user.click(await screen.findByRole("button", { name: "Retry" }));
    expect(await screen.findByText("Deferred safely.")).toBeInTheDocument();
    expect(submitted).toHaveLength(2);
    expect(submitted[0]).toEqual(submitted[1]);
  });

  it("removes selected cached evidence immediately when the refreshed server snapshot withholds it", async () => {
    let state = reviewData();
    installApiMock({ "GET /api/v1/dreamer/review": () => envelope(state) });
    const user = userEvent.setup();
    renderApp("/dreams");
    await user.click(await screen.findByRole("button", { name: `Review ${candidate.title}` }));
    expect(screen.getByText("Reviewed milestone.")).toBeInTheDocument();
    state = reviewData({ items: [{ ...candidate, title: "Evidence unavailable", body_md: "", why_md: "", uncertainty_md: "", candidate: null, sources: [], reviewable: false, stale: true, status: "stale", blocked_reason: "Evidence is no longer available. Cached proposal text has been withheld." }] });
    await user.click(screen.getByRole("button", { name: "Refresh" }));
    expect(await screen.findByRole("heading", { name: "Evidence unavailable" })).toBeInTheDocument();
    for (const text of [candidate.title, candidate.body_md, candidate.why_md!, candidate.uncertainty_md!, "Old milestone.", "Reviewed milestone.", "Milestone was approved."]) {
      expect(screen.queryByText(text)).not.toBeInTheDocument();
    }
    expect(screen.queryByRole("link", { name: "Projects/Status.md" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
  });

  it("keeps a network failure uncertain and retries the identical decision request", async () => {
    const submitted: unknown[] = [];
    installApiMock({
      "GET /api/v1/dreamer/review": envelope(reviewData()),
      "POST /api/v1/dreamer/review/decisions": async (request: Request) => {
        submitted.push(await request.json());
        if (submitted.length === 1) throw new TypeError("Connection lost");
        return { status: "committed", data: { application_status: "deferred", message: "Deferred after retry." } };
      },
    });
    const user = userEvent.setup();
    renderApp("/dreams");
    await user.click(await screen.findByRole("button", { name: `Review ${candidate.title}` }));
    await user.click(screen.getByRole("button", { name: "Defer" }));
    const retry = await screen.findByRole("button", { name: "Retry" });
    expect(screen.getByRole("button", { name: `Review ${question.title}` })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Next review item" })).toBeDisabled();
    await user.click(retry);
    expect(await screen.findByText("Deferred after retry.")).toBeInTheDocument();
    expect(submitted).toHaveLength(2);
    expect(submitted[1]).toEqual(submitted[0]);
  });

  it("shows read-only decisions and distinguishes unavailable review from an empty inbox", async () => {
    installApiMock({
      "GET /api/v1/me": { ...defaultMe, data: { ...defaultMe.data, read_only: true } },
      "GET /api/v1/dreamer/review": envelope(reviewData({ available: false, unavailable_reason: "Candidate state could not be read." })),
    });
    const user = userEvent.setup();
    renderApp("/dreams");
    expect(await screen.findByText("Candidate state could not be read.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: `Review ${candidate.title}` }));
    const detail = within(screen.getByRole("article", { name: `Review ${candidate.title}` }));
    expect(detail.getByText("Owner access is required to record decisions.")).toBeInTheDocument();
    for (const action of ["Approve", "Reject", "Defer"]) expect(detail.getByRole("button", { name: action })).toBeDisabled();
    expect(screen.queryByText("Nothing waiting for review")).not.toBeInTheDocument();
  });

  it("opens supporting evidence at the pinned version", async () => {
    let readPayload: Record<string, unknown> | undefined;
    installApiMock({
      "GET /api/v1/dreamer/review": envelope(reviewData()),
      "POST /api/v1/workspace/read": async (request: Request) => {
        readPayload = await request.json();
        return { status: "complete", data: { items: [{ reference: "entry:source-one", path: "Projects/Status.md", title: "Status", version: 4, text: "Exact evidence at version four.", status: "complete", view: "full" }] } };
      },
    });
    const user = userEvent.setup();
    renderApp("/dreams");
    await user.click(await screen.findByRole("button", { name: `Review ${candidate.title}` }));
    await user.click(screen.getByText("Supporting evidence", { exact: true }));
    await user.click(screen.getByRole("link", { name: "Projects/Status.md" }));
    expect(await screen.findByText("Exact evidence at version four.")).toBeInTheDocument();
    expect(readPayload).toEqual({ requests: [{ ref: "entry:source-one", version: 4, view: "full" }] });
  });

  it("shows raw location evidence as labeled text without a fabricated entry link or version", async () => {
    const label = "Raw location report 2026-09-06T10:00:00Z (ping)";
    installApiMock({ "GET /api/v1/dreamer/review": envelope(reviewData({ items: [{ ...candidate, sources: [{ entry_ref: "location-report:0123456789abcdef", label, version: null, excerpt: "Observed fields and original report key." }] }] })) });
    const user = userEvent.setup();
    renderApp("/dreams");
    await user.click(await screen.findByRole("button", { name: `Review ${candidate.title}` }));
    await user.click(screen.getByText("Supporting evidence", { exact: true }));
    expect(screen.getByText(label)).toBeInTheDocument();
    expect(screen.getByText("Observed fields and original report key.")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: label })).not.toBeInTheDocument();
    expect(screen.queryByText("v0")).not.toBeInTheDocument();
  });

  it("keeps thirty complete source previews collapsed until requested while decisions remain available", async () => {
    const sources = Array.from({ length: 30 }, (_, index) => ({ entry_ref: `entry:source-${index}`, version: index + 1, label: `Source ${index + 1}`, excerpt: `Full source ${index + 1}. ${"Original audit fields. ".repeat(50)}` }));
    installApiMock({ "GET /api/v1/dreamer/review": envelope(reviewData({ items: [{ ...candidate, sources }] })) });
    const user = userEvent.setup();
    renderApp("/dreams");
    await user.click(await screen.findByRole("button", { name: `Review ${candidate.title}` }));
    const evidence = screen.getByText("Supporting evidence", { exact: true }).closest("details")!;
    expect(evidence).not.toHaveAttribute("open");
    expect(within(evidence).getByText("30", { exact: true })).toBeVisible();
    expect(screen.getByRole("link", { name: "Source 30" })).not.toBeVisible();
    expect(screen.getByRole("button", { name: "Approve" })).toBeEnabled();
    await user.click(screen.getByText("Supporting evidence", { exact: true }));
    expect(evidence).toHaveAttribute("open");
    expect(within(evidence).getAllByRole("link")).toHaveLength(30);
    expect(screen.getByRole("link", { name: "Source 30" })).toHaveAttribute("href", expect.stringContaining("version=30"));
    const lastExcerpt = within(evidence).getByText(/^Full source 30\./);
    expect(lastExcerpt).toBeVisible();
    expect(lastExcerpt.textContent).toBe(sources[29].excerpt);
  });

  it.each(["empty", "already displayed"])("omits %s rationale and uncertainty sections without adding placeholders", async (reason) => {
    const item = { ...candidate, why_md: reason === "empty" ? " \n" : candidate.body_md, uncertainty_md: reason === "empty" ? "\n\t" : candidate.candidate!.after_md };
    installApiMock({ "GET /api/v1/dreamer/review": envelope(reviewData({ items: [item] })) });
    const user = userEvent.setup();
    renderApp("/dreams");
    await user.click(await screen.findByRole("button", { name: `Review ${candidate.title}` }));
    expect(screen.queryByRole("heading", { name: "Why it is proposed" })).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Uncertainty" })).not.toBeInTheDocument();
    expect(screen.queryByText("(Empty content)")).not.toBeInTheDocument();
    expect(screen.getAllByText(candidate.body_md)).toHaveLength(1);
    expect(screen.getAllByText("Reviewed milestone.")).toHaveLength(1);
  });

  it.each(["derived/location/day.md", "derived/entities/project.md"])("renders the managed summary at %s and keeps exact changes collapsed", async (targetPath) => {
    const after = "| When | Where |\n| --- | --- |\n| 08:00–10:00 | Home |\n| 10:00–17:00 | Office |";
    const before = "The exact earlier summary.";
    installApiMock({ "GET /api/v1/dreamer/review": envelope(reviewData({ items: [{ ...candidate, candidate: { target_path: targetPath, before_md: before, after_md: after, body_md: after } }] })) });
    const user = userEvent.setup();
    renderApp("/dreams");
    await user.click(await screen.findByRole("button", { name: `Review ${candidate.title}` }));
    const table = screen.getByRole("table");
    expect(within(table).getByRole("columnheader", { name: "When" })).toBeVisible();
    expect(within(table).getByRole("cell", { name: "Office" })).toBeVisible();
    const exact = screen.getByText("View exact changes").closest("details")!;
    expect(exact).not.toHaveAttribute("open");
    expect(screen.getByText(before)).not.toBeVisible();
    await user.click(screen.getByText("View exact changes"));
    expect(screen.getByText(before)).toBeVisible();
    expect(exact.querySelectorAll("pre")[1].textContent).toBe(after);
  });

  it("requires another review after a server conflict and never retries the stale decision", async () => {
    let state = reviewData();
    let posts = 0;
    installApiMock({
      "GET /api/v1/dreamer/review": () => envelope(state),
      "POST /api/v1/dreamer/review/decisions": () => {
        posts += 1;
        state = reviewData({ decision_version: 8 });
        return { status: 409, body: { error: { code: "stale_review", message: "Decisions changed since this review." } } };
      },
    });
    const user = userEvent.setup();
    renderApp("/dreams");
    await user.click(await screen.findByRole("button", { name: `Review ${candidate.title}` }));
    await user.click(screen.getByRole("button", { name: "Approve" }));
    expect(await screen.findByText("Decision needs another review")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Retry" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Review updated item" }));
    expect(screen.getByRole("button", { name: "Approve" })).toBeEnabled();
    expect(posts).toBe(1);
  });

  it("reports a failed inbox read without claiming there is no pending work", async () => {
    installApiMock({ "GET /api/v1/dreamer/review": { status: 503, body: { error: { code: "unavailable", message: "The review service is temporarily unavailable." } } } });
    renderApp("/dreams");
    expect(await screen.findByText("Review inbox unavailable")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
    expect(screen.queryByText("Nothing waiting for review")).not.toBeInTheDocument();
  });
});
