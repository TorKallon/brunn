import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  Check,
  FileText,
  FolderOpen,
  RotateCcw,
  Save,
} from "lucide-react";
import { type FormEvent, useState } from "react";
import { WorkspaceEntryView } from "../components/WorkspaceEntryView";
import { Metric, Page, PageHeader, Section } from "../components/Page";
import {
  EmptyState,
  ErrorState,
  ProtocolNotice,
  ReadOnlyNotice,
  StatusBadge,
} from "../components/StateViews";
import { useApi } from "../lib/auth";
import { useReadOnly } from "../lib/current";
import { shortId } from "../lib/format";
import type { WorkspaceEvidence } from "../lib/types";
import {
  newOperationId,
  parseNonEmptyLines,
} from "../lib/workspace";

export function WorkPage() {
  const api = useApi();
  const queryClient = useQueryClient();
  const readOnly = useReadOnly();
  const [task, setTask] = useState("");
  const [rootRefs, setRootRefs] = useState("");
  const [resumeCheckpoint, setResumeCheckpoint] = useState("");
  const [tokenBudget, setTokenBudget] = useState(24_000);
  const [objective, setObjective] = useState("");
  const [currentState, setCurrentState] = useState("");
  const [decisions, setDecisions] = useState("");
  const [questions, setQuestions] = useState("");
  const [nextActions, setNextActions] = useState("");
  const [artifacts, setArtifacts] = useState("");
  const [sourceRefs, setSourceRefs] = useState("");

  const openMutation = useMutation({
    mutationFn: () =>
      api.workspaceOpen({
        task: task.trim(),
        hints: {
          authorization_scope: "scope:root",
          root_refs: parseNonEmptyLines(rootRefs),
          open_object_refs: [],
        },
        ...(resumeCheckpoint.trim()
          ? { resume_checkpoint_ref: resumeCheckpoint.trim() }
          : {}),
        token_budget: tokenBudget,
      }),
    onSuccess: (response) => {
      const evidenceRefs = response.data.evidence.map((item) => item.reference);
      setSourceRefs(evidenceRefs.join("\n"));
      setObjective((value) => value || task.trim());
      readMutation.reset();
    },
  });

  const readMutation = useMutation({
    mutationFn: (evidence: WorkspaceEvidence) =>
      api.workspaceRead({
        ...(openMutation.data?.session_id
          ? { session_id: openMutation.data.session_id }
          : {}),
        requests: [{ ref: evidence.reference, view: "full" }],
      }),
  });

  const checkpointMutation = useMutation({
    mutationFn: () => {
      const sessionId = openMutation.data?.session_id;
      if (!sessionId) throw new Error("Open the workspace before checkpointing.");
      return api.workspaceCheckpoint({
        session_id: sessionId,
        ...(resumeCheckpoint.trim()
          ? { parent_checkpoint_id: resumeCheckpoint.trim() }
          : {}),
        state: {
          objective: objective.trim(),
          current_state: parseNonEmptyLines(currentState),
          decisions: parseNonEmptyLines(decisions),
          open_questions: parseNonEmptyLines(questions),
          next_actions: parseNonEmptyLines(nextActions),
          artifacts: parseNonEmptyLines(artifacts),
        },
        source_refs: parseNonEmptyLines(sourceRefs),
        idempotency_key: newOperationId("ui_checkpoint"),
      });
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["workspace-manifest"] });
      void queryClient.invalidateQueries({ queryKey: ["workspace-changes"] });
    },
  });

  const opened = openMutation.data;
  const openData = opened?.data;
  const selectedEntry = readMutation.data?.data.items[0];

  function submitOpen(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (task.trim()) openMutation.mutate();
  }

  function submitCheckpoint(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!readOnly && objective.trim()) checkpointMutation.mutate();
  }

  return (
    <Page>
      <PageHeader
        title="Workspace"
        description="Current context and resumable work"
      />
      {readOnly ? <ReadOnlyNotice /> : null}

      <Section title="Open context" actions={<FolderOpen size={18} aria-hidden="true" />}>
        <form className="form-grid" onSubmit={submitOpen}>
          <label className="field field-span-2">
            <span>Goal</span>
            <textarea
              rows={4}
              value={task}
              onChange={(event) => setTask(event.target.value)}
              required
            />
          </label>
          <label className="field">
            <span>Resume checkpoint</span>
            <input
              value={resumeCheckpoint}
              onChange={(event) => setResumeCheckpoint(event.target.value)}
              placeholder="checkpoint:..."
              spellCheck={false}
            />
          </label>
          <label className="field">
            <span>Token budget</span>
            <input
              type="number"
              min={1_000}
              max={64_000}
              step={1_000}
              value={tokenBudget}
              onChange={(event) => setTokenBudget(Number(event.target.value))}
            />
          </label>
          <label className="field field-span-2">
            <span>Root paths or entry references</span>
            <textarea
              rows={2}
              value={rootRefs}
              onChange={(event) => setRootRefs(event.target.value)}
              spellCheck={false}
            />
          </label>
          <div className="form-actions field-span-2">
            <button
              className="button primary"
              type="submit"
              disabled={!task.trim() || openMutation.isPending}
            >
              {openMutation.isPending ? (
                <RotateCcw className="spin" size={17} aria-hidden="true" />
              ) : (
                <FolderOpen size={17} aria-hidden="true" />
              )}
              {openMutation.isPending ? "Opening" : "Open"}
            </button>
          </div>
        </form>
        {openMutation.isError ? (
          <ErrorState
            error={openMutation.error}
            retry={() => openMutation.mutate()}
            title="Open failed"
          />
        ) : null}
      </Section>

      {!opened && !openMutation.isPending ? (
        <EmptyState title="No context packet open" />
      ) : null}

      {opened && openData ? (
        <>
          <ProtocolNotice
            status={opened.status}
            gaps={opened.gaps?.length}
            conflicts={opened.conflicts?.length}
          />
          <div className="metric-grid">
            <Metric
              label="Generation"
              value={openData.workspace_generation}
              detail={opened.session_id ? shortId(opened.session_id, 18) : "Stateless"}
            />
            <Metric
              label="Evidence"
              value={openData.evidence.length}
              detail={`${openData.retrieval_sufficiency?.complete_source_count ?? 0} complete`}
            />
            <Metric
              label="Changes since checkpoint"
              value={openData.changes_since_checkpoint.length}
              detail={openData.checkpoint ? "Resume delta" : "No checkpoint"}
            />
            <Metric
              label="Retrieval"
              value={
                <StatusBadge
                  status={openData.retrieval_sufficiency?.status ?? opened.status}
                />
              }
            />
          </div>

          {openData.checkpoint ? (
            <Section
              title="Resumed checkpoint"
              meta={`Generation ${openData.checkpoint.workspace_generation ?? "unknown"}`}
            >
              <pre className="markdown-source compact-source">
                {openData.checkpoint.text}
              </pre>
            </Section>
          ) : null}

          {openData.changes_since_checkpoint.length ? (
            <Section
              title="Changes since checkpoint"
              meta={`${openData.changes_since_checkpoint.length} paths`}
            >
              <div className="change-list">
                {openData.changes_since_checkpoint.map((change) => (
                  <div key={change.generation}>
                    <StatusBadge status={change.operation} />
                    <code>{change.path}</code>
                    <span>v{change.version}</span>
                    <strong>g{change.generation}</strong>
                  </div>
                ))}
              </div>
            </Section>
          ) : null}

          <div className="workspace-split">
            <Section
              title="Evidence"
              meta={`${openData.evidence.length} selected`}
            >
              {openData.evidence.length ? (
                <div className="result-list">
                  {openData.evidence.map((evidence) => (
                    <article className="result-card" key={evidence.reference}>
                      <header>
                        <div>
                          <StatusBadge status={evidence.representation} />
                          <h3>{evidence.title}</h3>
                        </div>
                        {evidence.score !== undefined ? (
                          <strong className="score">
                            {evidence.score.toFixed(3)}
                          </strong>
                        ) : null}
                      </header>
                      <code className="result-path">{evidence.path}</code>
                      <p>{evidence.text}</p>
                      <footer>
                        <span>v{evidence.version}</span>
                        {(evidence.why_selected ?? []).map((lane) => (
                          <StatusBadge status={lane} key={lane} />
                        ))}
                        <button
                          className="button secondary"
                          type="button"
                          onClick={() => readMutation.mutate(evidence)}
                        >
                          <FileText size={16} aria-hidden="true" />
                          Read exact
                        </button>
                      </footer>
                    </article>
                  ))}
                </div>
              ) : (
                <EmptyState title="No evidence returned" />
              )}
            </Section>

            <Section title="Exact entry">
              {readMutation.isPending ? (
                <EmptyState title="Reading exact entry" />
              ) : null}
              {readMutation.isError ? (
                <ErrorState error={readMutation.error} title="Read failed" />
              ) : null}
              {selectedEntry ? (
                <WorkspaceEntryView entry={selectedEntry} />
              ) : !readMutation.isPending ? (
                <EmptyState title="No entry selected" />
              ) : null}
            </Section>
          </div>

          <Section title="Checkpoint">
            <form className="form-grid" onSubmit={submitCheckpoint}>
              <label className="field field-span-2">
                <span>Objective</span>
                <input
                  value={objective}
                  onChange={(event) => setObjective(event.target.value)}
                  disabled={readOnly}
                  required
                />
              </label>
              <label className="field field-span-2">
                <span>Current state</span>
                <textarea
                  rows={4}
                  value={currentState}
                  onChange={(event) => setCurrentState(event.target.value)}
                  disabled={readOnly}
                />
              </label>
              <label className="field">
                <span>Decisions</span>
                <textarea
                  rows={4}
                  value={decisions}
                  onChange={(event) => setDecisions(event.target.value)}
                  disabled={readOnly}
                />
              </label>
              <label className="field">
                <span>Open questions</span>
                <textarea
                  rows={4}
                  value={questions}
                  onChange={(event) => setQuestions(event.target.value)}
                  disabled={readOnly}
                />
              </label>
              <label className="field">
                <span>Next actions</span>
                <textarea
                  rows={4}
                  value={nextActions}
                  onChange={(event) => setNextActions(event.target.value)}
                  disabled={readOnly}
                />
              </label>
              <label className="field">
                <span>Artifacts</span>
                <textarea
                  rows={4}
                  value={artifacts}
                  onChange={(event) => setArtifacts(event.target.value)}
                  disabled={readOnly}
                />
              </label>
              <label className="field field-span-2">
                <span>Source paths or entry references</span>
                <textarea
                  rows={3}
                  value={sourceRefs}
                  onChange={(event) => setSourceRefs(event.target.value)}
                  disabled={readOnly}
                  spellCheck={false}
                />
              </label>
              <div className="form-actions field-span-2">
                <button
                  className="button primary"
                  type="submit"
                  disabled={
                    readOnly ||
                    !objective.trim() ||
                    checkpointMutation.isPending
                  }
                  title={
                    readOnly ? "Requires a read/write credential" : undefined
                  }
                >
                  <Save size={17} aria-hidden="true" />
                  {checkpointMutation.isPending ? "Saving" : "Save checkpoint"}
                </button>
                {checkpointMutation.isSuccess ? (
                  <span className="success-message">
                    <Check size={16} aria-hidden="true" />
                    {checkpointMutation.data.data.checkpoint_ref}
                  </span>
                ) : null}
              </div>
              {checkpointMutation.isError ? (
                <div className="field-span-2">
                  <ErrorState
                    error={checkpointMutation.error}
                    title="Checkpoint failed"
                  />
                </div>
              ) : null}
            </form>
          </Section>
        </>
      ) : null}
    </Page>
  );
}
