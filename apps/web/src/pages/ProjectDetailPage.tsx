import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import { ArrowLeft, Clock3, FolderKanban } from "lucide-react";
import { Page, PageHeader, Section } from "../components/Page";
import { EmptyState, ErrorState, LoadingState, StatusBadge } from "../components/StateViews";
import { TaskRow } from "../components/TaskRow";
import { useApi } from "../lib/auth";
import { useCapability } from "../lib/current";
import { formatDate } from "../lib/format";
import type { TaskProjectStateData } from "../lib/types";
import { newOperationId } from "../lib/workspace";

export function ProjectDetailPage() {
  const { slug } = useParams({ from: "/authenticated/projects/$slug" });
  const api = useApi();
  const queryClient = useQueryClient();
  const canWrite = useCapability("task.write");
  const query = useQuery({
    queryKey: ["task-project-state", slug],
    queryFn: () => api.taskProjectState(slug),
  });
  const state = query.data?.data;
  const interestMutation = useMutation({
    mutationFn: (interest: "hot" | "normal" | "parked") => {
      if (!state) throw new Error("The project is not loaded.");
      return api.taskProjectSetInterest(slug, {
        interest,
        expected_version: state.project.version,
        source: "owner",
        idempotency_key: newOperationId("web_project_interest"),
      });
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["task-project-state", slug] });
      void queryClient.invalidateQueries({ queryKey: ["task-projects"] });
    },
  });

  return (
    <Page>
      <PageHeader
        title={state?.project.title ?? "Project"}
        description="Checkpoint context and bounded next actions, derived without manual duplication"
        actions={
          <Link className="button secondary" to="/dashboard">
            <ArrowLeft size={16} aria-hidden="true" />
            Overview
          </Link>
        }
      />
      {query.isPending ? <LoadingState label="Loading project state" /> : null}
      {query.isError ? (
        <ErrorState error={query.error} retry={() => void query.refetch()} title="Unable to load project" />
      ) : null}
      {interestMutation.isError ? <ErrorState error={interestMutation.error} title="Unable to change interest" /> : null}
      {state ? <ProjectState state={state} canWrite={canWrite} setInterest={(value) => interestMutation.mutate(value)} pending={interestMutation.isPending} /> : null}
    </Page>
  );
}

function ProjectState({
  state,
  canWrite,
  setInterest,
  pending,
}: {
  state: TaskProjectStateData;
  canWrite: boolean;
  setInterest: (value: "hot" | "normal" | "parked") => void;
  pending: boolean;
}) {
  const checkpoint = state.checkpoint;
  const checkpointState = checkpoint?.state;
  const next = state.next.slice(0, 3);
  const waitingSlots = Math.max(0, 5 - next.length);
  const waiting = state.waiting.slice(0, waitingSlots);
  return (
    <>
      <section className="project-state-summary" aria-label="Project summary">
        <div>
          <FolderKanban size={18} aria-hidden="true" />
          <span>Interest</span>
          <StatusBadge status={state.project.interest} />
        </div>
        <div><span>Urgent</span><strong>{state.urgent_count}</strong></div>
        <div><span>Open</span><strong>{state.rollups.open}</strong></div>
        <div><span>Waiting</span><strong>{state.rollups.waiting}</strong></div>
        <div><span>Parked</span><strong>{state.parked_count}</strong></div>
      </section>
      {canWrite ? (
        <fieldset className="project-interest-control">
          <legend>Set interest for 14 days</legend>
          {(["hot", "normal", "parked"] as const).map((interest) => (
            <button
              className={state.project.interest === interest ? "button primary" : "button secondary"}
              type="button"
              key={interest}
              disabled={pending}
              onClick={() => setInterest(interest)}
            >
              {interest}
            </button>
          ))}
        </fieldset>
      ) : null}
      <Section
        title="Latest checkpoint"
        meta={checkpoint ? formatDate(checkpoint.checkpoint_at) : "No linked checkpoint"}
      >
        {checkpoint && checkpointState ? (
          <div className="checkpoint-state-card">
            <span className="project-attribution">{checkpoint.attribution} linkage</span>
            <h3>{checkpointState.objective ?? "Checkpoint"}</h3>
            <CheckpointList title="Current state" items={asList(checkpointState.current_state)} />
            <CheckpointList title="Next actions" items={asList(checkpointState.next_actions)} />
            <CheckpointList title="Open questions" items={asList(checkpointState.open_questions)} />
          </div>
        ) : (
          <EmptyState title="No linked checkpoint yet" detail="Task state still remains available below." />
        )}
      </Section>
      <Section title="Next and waiting" meta="Five task rows maximum">
        <div className="project-task-sections">
          <div className="task-list">
            {next.map((item) => (
              <TaskRow key={item.task_ref} item={item} canWrite={false} showActions={false} />
            ))}
          </div>
          {waiting.map((item) => (
            <article className="project-waiting-row" key={item.task_ref}>
              <Clock3 size={16} aria-hidden="true" />
              <span>
                <Link to="/tasks/$taskRef" params={{ taskRef: item.task_ref }}>{item.title}</Link>
                <small>
                  Waiting {item.age_days} day{item.age_days === 1 ? "" : "s"}
                  {item.waiting_on?.who_or_what ? ` on ${item.waiting_on.who_or_what}` : ""}
                </small>
              </span>
            </article>
          ))}
          {state.waiting_total > waiting.length ? (
            <p className="task-card-note">{state.waiting_total - waiting.length} more waiting tasks held out of view.</p>
          ) : null}
        </div>
      </Section>
    </>
  );
}

function CheckpointList({ title, items }: { title: string; items: string[] }) {
  if (!items.length) return null;
  return (
    <section>
      <h4>{title}</h4>
      <ul>{items.map((item) => <li key={item}>{item}</li>)}</ul>
    </section>
  );
}

function asList(value: string | string[] | undefined): string[] {
  if (!value) return [];
  return Array.isArray(value) ? value : [value];
}
