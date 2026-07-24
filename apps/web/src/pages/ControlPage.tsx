import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Activity,
  Ban,
  BarChart3,
  Check,
  ChevronsDown,
  Copy,
  KeyRound,
  Plus,
  RefreshCw,
  Shield,
} from "lucide-react";
import { type FormEvent, useState } from "react";
import { DataTable } from "../components/DataTable";
import { JsonView } from "../components/JsonView";
import { DefinitionList, Metric, Page, PageHeader, Section } from "../components/Page";
import { EmptyState, ErrorState, LoadingState, ReadOnlyNotice, StatusBadge } from "../components/StateViews";
import { TabPanel, Tabs } from "../components/Tabs";
import { useApi } from "../lib/auth";
import { useCurrent, useReadOnly } from "../lib/current";
import { formatDate, humanize, shortId } from "../lib/format";
import {
  asItems,
  type CredentialSummary,
  type DataUsageItem,
} from "../lib/types";

const numberFormat = new Intl.NumberFormat();

export function ControlPage() {
  const api = useApi();
  const current = useCurrent();
  const readOnly = useReadOnly();
  const canManageCredentials = current.data.capabilities?.includes("credential:manage") ?? false;
  const queryClient = useQueryClient();
  const [activeTab, setActiveTab] = useState("credentials");
  const [usageRanking, setUsageRanking] = useState<
    "most_used" | "least_used" | "least_recently_used"
  >("most_used");
  const [createOpen, setCreateOpen] = useState(false);
  const [credentialName, setCredentialName] = useState("");
  const [credentialAccess, setCredentialAccess] = useState<"read_only" | "read_write">("read_only");
  const [credentialScope, setCredentialScope] = useState(current.data.active_scope?.id ?? "");
  const [copied, setCopied] = useState(false);

  const credentialsQuery = useQuery({ queryKey: ["credentials"], queryFn: () => api.credentials() });
  const scopesQuery = useQuery({ queryKey: ["scopes"], queryFn: () => api.scopes() });
  const policiesQuery = useQuery({ queryKey: ["policies"], queryFn: () => api.policies() });
  const auditQuery = useInfiniteQuery({
    queryKey: ["audit"],
    queryFn: ({ pageParam }) => api.audit(pageParam || undefined),
    initialPageParam: "",
    getNextPageParam: (lastPage) =>
      Array.isArray(lastPage.data)
        ? undefined
        : lastPage.data.continuation_token ?? undefined,
  });
  const usageQuery = useQuery({ queryKey: ["usage"], queryFn: () => api.usage() });
  const statusQuery = useQuery({ queryKey: ["service-status"], queryFn: () => api.status(), refetchInterval: 30_000 });
  const createCredentialMutation = useMutation({
    mutationFn: () => api.createCredential({ name: credentialName, access: credentialAccess, scope_ids: credentialScope ? [credentialScope] : [] }),
    onSuccess: () => {
      setCredentialName("");
      setCreateOpen(false);
      void queryClient.invalidateQueries({ queryKey: ["credentials"] });
    },
  });
  const revokeMutation = useMutation({
    mutationFn: (id: string) => api.revokeCredential(id),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ["credentials"] }),
  });

  const credentials = asItems(credentialsQuery.data?.data);
  const scopes = asItems(scopesQuery.data?.data);
  const policies = asItems(policiesQuery.data?.data);
  const audit = auditQuery.data?.pages.flatMap((page) => asItems(page.data)) ?? [];
  const auditTotal = (() => {
    const first = auditQuery.data?.pages[0]?.data;
    return first && !Array.isArray(first) ? first.total ?? audit.length : audit.length;
  })();
  const usage = usageQuery.data?.data;
  const usageItems = usage?.[usageRanking] ?? [];
  const createdCredential = createCredentialMutation.data?.data;

  function submitCredential(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (canManageCredentials && credentialName.trim()) createCredentialMutation.mutate();
  }

  async function copyToken(credential: CredentialSummary) {
    if (!credential.token) return;
    await navigator.clipboard.writeText(credential.token);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 2_000);
  }

  return (
    <Page>
      <PageHeader title="Control" description="Access, usage, policy, audit, and service state" />
      {readOnly ? <ReadOnlyNotice /> : null}
      <Tabs
        tabs={[
          { id: "credentials", label: "Credentials", count: credentials.length || undefined },
          { id: "usage", label: "Usage", count: usage?.summary.used_source_count || undefined },
          { id: "scopes", label: "Scopes", count: scopes.length || undefined },
          { id: "policies", label: "Policies", count: policies.length || undefined },
          { id: "audit", label: "Audit", count: audit.length || undefined },
          { id: "health", label: "Health" },
        ]}
        active={activeTab}
        onChange={setActiveTab}
      />

      <TabPanel id="credentials" active={activeTab}>
        <Section title="API credentials" actions={<button className="button primary" type="button" onClick={() => setCreateOpen((value) => !value)} disabled={!canManageCredentials} title={!canManageCredentials ? "Requires an owner credential" : undefined}><Plus size={17} />New credential</button>}>
          {createOpen ? <form className="form-grid inset-form" onSubmit={submitCredential}><label className="field"><span>Name</span><input value={credentialName} onChange={(event) => setCredentialName(event.target.value)} required /></label><label className="field"><span>Access</span><select value={credentialAccess} onChange={(event) => setCredentialAccess(event.target.value as "read_only" | "read_write")}><option value="read_only">Read only</option><option value="read_write">Read/write</option></select></label><label className="field"><span>Scope</span><select value={credentialScope} onChange={(event) => setCredentialScope(event.target.value)}><option value="">All authorized scopes</option>{scopes.map((scope) => <option key={scope.id} value={scope.id}>{scope.name}</option>)}</select></label><div className="form-actions"><button className="button primary" type="submit" disabled={!credentialName.trim() || createCredentialMutation.isPending}><KeyRound size={17} />Create</button></div>{createCredentialMutation.isError ? <div className="field-span-2"><ErrorState error={createCredentialMutation.error} title="Credential creation failed" /></div> : null}</form> : null}
          {createdCredential?.token ? <div className="secret-once" role="status"><div><strong>Credential created</strong><span>This token is displayed once.</span></div><code>{createdCredential.token}</code><button className="icon-button" type="button" onClick={() => void copyToken(createdCredential)} aria-label="Copy token" title="Copy token">{copied ? <Check size={17} /> : <Copy size={17} />}</button></div> : null}
          {credentialsQuery.isPending ? <LoadingState label="Loading credentials" /> : null}
          {credentialsQuery.isError ? <ErrorState error={credentialsQuery.error} retry={() => void credentialsQuery.refetch()} /> : null}
          {credentialsQuery.isSuccess && !credentials.length ? <EmptyState title="No credentials" /> : null}
          {credentials.length ? <div className="credential-list">{credentials.map((credential) => <article key={credential.id}><div className="credential-icon"><KeyRound size={18} /></div><div><strong>{credential.name}</strong><code>{shortId(credential.id, 14)}</code><span>{credential.scope_ids.length ? `${credential.scope_ids.length} scope${credential.scope_ids.length === 1 ? "" : "s"}` : "All authorized scopes"}</span></div><StatusBadge status={credential.revoked_at ? "revoked" : credential.access} /><div><span>Last used</span><strong>{formatDate(credential.last_used_at)}</strong></div><button className="icon-button danger-icon" type="button" onClick={() => revokeMutation.mutate(credential.id)} disabled={!canManageCredentials || Boolean(credential.revoked_at) || revokeMutation.isPending} aria-label={`Revoke ${credential.name}`} title="Revoke"><Ban size={17} /></button></article>)}</div> : null}
        </Section>
      </TabPanel>

      <TabPanel id="usage" active={activeTab}>
        {usageQuery.isPending ? <LoadingState label="Loading data usage" /> : null}
        {usageQuery.isError ? <ErrorState error={usageQuery.error} retry={() => void usageQuery.refetch()} /> : null}
        {usage ? (
          <>
            <div className="metric-grid">
              <Metric label="Active sources" value={numberFormat.format(usage.summary.active_source_count)} />
              <Metric label="Used sources" value={numberFormat.format(usage.summary.used_source_count)} />
              <Metric label="Never used" value={numberFormat.format(usage.summary.never_used_source_count)} />
              <Metric label="Source uses" value={numberFormat.format(usage.summary.source_uses)} detail={`${numberFormat.format(usage.summary.tracked_response_count)} responses`} />
            </div>
            <Section
              title="Agent data usage"
              meta={`${usageItems.length} sources`}
              actions={<BarChart3 size={18} aria-hidden="true" />}
            >
              <div className="mode-control" aria-label="Usage ranking">
                <button type="button" className={usageRanking === "most_used" ? "active" : undefined} onClick={() => setUsageRanking("most_used")}>Most used</button>
                <button type="button" className={usageRanking === "least_used" ? "active" : undefined} onClick={() => setUsageRanking("least_used")}>Least used</button>
                <button type="button" className={usageRanking === "least_recently_used" ? "active" : undefined} onClick={() => setUsageRanking("least_recently_used")}>Least recent</button>
              </div>
              <DataTable<DataUsageItem>
                data={usageItems}
                emptyTitle="No source usage"
                columns={[
                  {
                    id: "source",
                    header: "Source",
                    cell: ({ row }) => (
                      <div className="usage-source">
                        <strong>{row.original.title}</strong>
                        <code>{row.original.source_ref}</code>
                      </div>
                    ),
                  },
                  { accessorKey: "kind", header: "Kind", cell: ({ getValue }) => humanize(String(getValue())) },
                  { accessorKey: "access_count", header: "Uses", cell: ({ getValue }) => numberFormat.format(Number(getValue())) },
                  { accessorKey: "session_count", header: "Sessions", cell: ({ getValue }) => numberFormat.format(Number(getValue())) },
                  { accessorKey: "last_used_at", header: "Last used", cell: ({ row }) => row.original.never_used ? "Never" : formatDate(row.original.last_used_at) },
                ]}
              />
            </Section>
            <Section title="Use by operation">
              {usage.by_operation.length ? (
                <div className="metric-grid compact-metrics">
                  {usage.by_operation.map((operation) => (
                    <Metric
                      key={operation.operation}
                      label={humanize(operation.operation)}
                      value={numberFormat.format(operation.source_uses)}
                      detail={`${numberFormat.format(operation.response_count)} responses`}
                    />
                  ))}
                </div>
              ) : <EmptyState title="No tracked operations" />}
            </Section>
          </>
        ) : null}
      </TabPanel>

      <TabPanel id="scopes" active={activeTab}>
        <Section title="Authorization scopes" actions={<Shield size={18} aria-hidden="true" />}>
          {scopesQuery.isPending ? <LoadingState label="Loading scopes" /> : null}
          {scopesQuery.isError ? <ErrorState error={scopesQuery.error} retry={() => void scopesQuery.refetch()} /> : null}
          {scopes.length ? <div className="scope-list">{scopes.map((scope) => <article key={scope.id}><header><strong>{scope.name}</strong><StatusBadge status={scope.access} /></header><p>{scope.description ?? "No description"}</p><footer><code>{scope.id}</code><span>{scope.object_count ?? 0} objects</span>{scope.id === current.data.active_scope?.id ? <strong>Active</strong> : null}</footer></article>)}</div> : scopesQuery.isSuccess ? <EmptyState title="No scopes" /> : null}
        </Section>
      </TabPanel>

      <TabPanel id="policies" active={activeTab}>
        <Section title="Policy revisions">
          {policiesQuery.isPending ? <LoadingState label="Loading policies" /> : null}
          {policiesQuery.isError ? <ErrorState error={policiesQuery.error} retry={() => void policiesQuery.refetch()} /> : null}
          {policies.length ? <div className="policy-list">{policies.map((policy) => <article key={policy.id}><header><div><strong>{policy.name}</strong><code>{policy.id} · v{policy.version ?? "?"}</code></div><StatusBadge status={policy.status} /></header><DefinitionList items={[{ label: "Audience", value: policy.audience?.join(", ") || "Any authorized" }, { label: "Purpose", value: policy.purpose?.join(", ") || "Any authorized" }, { label: "Updated", value: formatDate(policy.updated_at) }]} />{policy.rules !== undefined ? <JsonView value={policy.rules} label={`${policy.name} rules`} /> : null}</article>)}</div> : policiesQuery.isSuccess ? <EmptyState title="No policies" /> : null}
        </Section>
      </TabPanel>

      <TabPanel id="audit" active={activeTab}>
        <Section title="Audit log" meta={`${audit.length} of ${auditTotal} events`}>
          {auditQuery.isPending ? <LoadingState label="Loading audit log" /> : null}
          {auditQuery.isError ? <ErrorState error={auditQuery.error} retry={() => void auditQuery.refetch()} /> : null}
          {audit.length ? <><div className="audit-list control-audit">{audit.map((event) => <div key={event.id}><time>{formatDate(event.created_at)}</time><strong>{event.action}</strong><span>{event.actor ?? "System"}</span><code>{event.target ?? event.request_id ?? event.id}</code><StatusBadge status={event.status} />{event.details !== undefined ? <JsonView value={event.details} label={`${event.action} details`} /> : null}</div>)}</div>{auditQuery.hasNextPage ? <div className="section-footer"><button className="button secondary" type="button" onClick={() => void auditQuery.fetchNextPage()} disabled={auditQuery.isFetchingNextPage}><ChevronsDown size={16} aria-hidden="true" />{auditQuery.isFetchingNextPage ? "Loading" : "Load more"}</button></div> : null}</> : auditQuery.isSuccess ? <EmptyState title="No audit events" /> : null}
        </Section>
      </TabPanel>

      <TabPanel id="health" active={activeTab}>
        {statusQuery.isPending ? <LoadingState label="Loading service health" /> : null}
        {statusQuery.isError ? <ErrorState error={statusQuery.error} retry={() => void statusQuery.refetch()} /> : null}
        {statusQuery.data ? <><div className="metric-grid"><Metric label="Service" value={<StatusBadge status={statusQuery.data.data.status} />} detail={statusQuery.data.data.version} /><Metric label="Corpus revision" value={<code>{shortId(statusQuery.data.data.corpus_revision)}</code>} /><Metric label="Dependencies" value={Object.keys(statusQuery.data.data.dependencies ?? {}).length} /><Metric label="Indexes" value={Object.keys(statusQuery.data.data.indexes ?? {}).length} /></div><div className="two-column-layout"><Section title="Dependencies" actions={<Activity size={18} aria-hidden="true" />}>{Object.entries(statusQuery.data.data.dependencies ?? {}).length ? <div className="health-list">{Object.entries(statusQuery.data.data.dependencies ?? {}).map(([name, dependency]) => <div key={name}><div><strong>{humanize(name)}</strong><span>{dependency.detail ?? `${dependency.latency_ms ?? 0} ms`}</span></div><StatusBadge status={dependency.status} /></div>)}</div> : <EmptyState title="No dependency details" />}</Section><Section title="Indexes" actions={<RefreshCw size={18} aria-hidden="true" />}>{Object.entries(statusQuery.data.data.indexes ?? {}).length ? <div className="health-list">{Object.entries(statusQuery.data.data.indexes ?? {}).map(([name, index]) => <div key={name}><div><strong>{humanize(name)}</strong><span>{index.revision ? `Revision ${shortId(index.revision)}` : formatDate(index.updated_at)}</span></div><StatusBadge status={index.status} /></div>)}</div> : <EmptyState title="No index details" />}</Section></div></> : null}
      </TabPanel>
    </Page>
  );
}
