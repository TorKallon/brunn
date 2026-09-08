import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { DreamerReviewData, DreamerReviewItem } from "../lib/dreamerReview";
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
    expect(screen.getByRole("link", { name: "Projects/Status.md" })).toHaveAttribute("href", expect.stringContaining("version=4"));
    await user.type(screen.getByRole("textbox", { name: "Comment (optional)" }), "The source is correct.");
    await user.click(screen.getByRole("button", { name: "Approve" }));
    expect(await screen.findByText("Approved and held by report-only mode.")).toBeInTheDocument();
    expect(payload).toEqual({ item_id: candidate.id, run_entry_ref: candidate.run_entry_ref, run_version: 3, candidate_hash: candidate.candidate_hash, expected_decisions_version: 7, decision: "approve", comment: "The source is correct.", idempotency_key: expect.any(String) });
    expect(await screen.findByText("Approved Held")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
    expect(fetchMock.mock.calls.some(([input]) => /\/api\/v1\/dreams(?:\/|$)/.test(String(input)))).toBe(false);
  });

  it("shows legacy prose honestly and records a rejection without inventing a candidate", async () => {
    const legacy = { ...candidate, candidate: null, reviewable: false, blocked_reason: "This legacy report contains a description, not an exact candidate." };
    let submitted: Record<string, unknown> | undefined;
    installApiMock({
      "GET /api/v1/dreamer/review": envelope(reviewData({ items: [legacy] })),
      "POST /api/v1/dreamer/review/decisions": async (request: Request) => { submitted = await request.json(); return { status: "committed", data: { application_status: "rejected", message: "Rejected. The proposal will not be applied." } }; },
    });
    const user = userEvent.setup();
    renderApp("/dreams");
    await user.click(await screen.findByRole("button", { name: `Review ${candidate.title}` }));
    expect(screen.getByText(legacy.blocked_reason)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Reject" }));
    expect(await screen.findByText("Rejected. The proposal will not be applied.")).toBeInTheDocument();
    expect(submitted).toMatchObject({ item_id: candidate.id, decision: "reject" });
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
    await user.click(screen.getByText("Answer or correct this item"));
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

  it("pins the selected candidate across refresh and requires reviewing the changed version", async () => {
    let state = reviewData();
    installApiMock({ "GET /api/v1/dreamer/review": () => envelope(state) });
    const user = userEvent.setup();
    renderApp("/dreams");
    await user.click(await screen.findByRole("button", { name: `Review ${candidate.title}` }));
    state = reviewData({ decision_version: 8, items: [{ ...candidate, candidate_hash: "sha256:updated", run_version: 4, candidate: { after_md: "A different candidate." } }] });
    await user.click(screen.getByRole("button", { name: "Refresh" }));
    expect(await screen.findByText("This review has changed.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
    expect(screen.getByText("Reviewed milestone.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Review updated item" }));
    expect(screen.getByText("A different candidate.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Approve" })).toBeEnabled();
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
    expect(screen.getByText(label)).toBeInTheDocument();
    expect(screen.getByText("Observed fields and original report key.")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: label })).not.toBeInTheDocument();
    expect(screen.queryByText("v0")).not.toBeInTheDocument();
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
