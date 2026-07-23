import { useMutation, useQuery } from "@tanstack/react-query";
import { useParams } from "@tanstack/react-router";
import { Download, FileText, Maximize2 } from "lucide-react";
import { useState } from "react";
import { JsonView } from "../components/JsonView";
import { DefinitionList, Page, PageHeader, Section } from "../components/Page";
import { EmptyState, ErrorState, LoadingState, ProtocolNotice, StatusBadge } from "../components/StateViews";
import { TabPanel, Tabs } from "../components/Tabs";
import { useApi } from "../lib/auth";
import { useCurrent } from "../lib/current";
import { formatBytes, formatDate, shortId } from "../lib/format";

export function SourcePage() {
  const { sourceId } = useParams({ from: "/sources/$sourceId" });
  const api = useApi();
  const current = useCurrent();
  const [activeTab, setActiveTab] = useState("preview");
  const sourceQuery = useQuery({ queryKey: ["source", sourceId], queryFn: () => api.source(sourceId) });
  const fullReadMutation = useMutation({
    mutationFn: async () => {
      const opened = await api.open({
        task: `Inspect source ${sourceId}`,
        mode: "exploration",
        hints: {
          authorization_scope: current.data.active_scope?.id ?? "authorized",
          root_refs: [sourceId],
        },
        token_budget: 4_000,
      });
      const sessionId = opened.data.session_id ?? opened.session_id ?? opened.data.id;
      return api.read({ session_id: sessionId, requests: [{ ref: sourceId, view: "full" }] });
    },
  });
  const downloadMutation = useMutation({
    mutationFn: () => api.downloadSource(sourceId),
    onSuccess: (blob) => {
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = sourceQuery.data?.data.title ?? sourceId;
      anchor.click();
      URL.revokeObjectURL(url);
    },
  });

  if (sourceQuery.isPending) return <Page><LoadingState label="Loading source" /></Page>;
  if (sourceQuery.isError) return <Page><ErrorState error={sourceQuery.error} retry={() => void sourceQuery.refetch()} /></Page>;
  const source = sourceQuery.data.data;
  const tabs = [
    { id: "preview", label: "Preview" },
    { id: "outline", label: "Outline", count: source.outline?.length ?? 0 },
    { id: "claims", label: "Derived claims", count: source.derived_claims?.length ?? 0 },
    { id: "versions", label: "Versions", count: source.versions?.length ?? 0 },
    { id: "policy", label: "Policy" },
    { id: "audit", label: "Audit", count: source.audit?.length ?? 0 },
  ];

  return (
    <Page>
      <PageHeader
        title={source.title}
        description={`Source ${source.id}`}
        actions={<><button className="button secondary" type="button" onClick={() => fullReadMutation.mutate()} disabled={fullReadMutation.isPending}><Maximize2 size={17} />Full read</button><button className="icon-button" type="button" onClick={() => downloadMutation.mutate()} disabled={downloadMutation.isPending} aria-label="Download source" title="Download"><Download size={18} /></button></>}
      />
      <ProtocolNotice status={sourceQuery.data.status} gaps={sourceQuery.data.gaps?.length} conflicts={sourceQuery.data.conflicts?.length} />
      <Section title="Source details" actions={<FileText size={18} aria-hidden="true" />}>
        <DefinitionList items={[
          { label: "Kind", value: source.kind ?? "Unknown" },
          { label: "Content type", value: source.content_type ?? "Unknown" },
          { label: "Size", value: formatBytes(source.size_bytes) },
          { label: "Version", value: source.version ?? "Unknown" },
          { label: "Hash", value: <code title={source.hash}>{shortId(source.hash, 18)}</code> },
          { label: "Updated", value: formatDate(source.updated_at) },
        ]} />
      </Section>
      <Tabs tabs={tabs} active={activeTab} onChange={setActiveTab} />
      <TabPanel id="preview" active={activeTab}>
        <Section title="Source-native preview">
          {source.content ? <pre className="source-preview">{source.content}</pre> : <EmptyState title="No inline preview" />}
          {fullReadMutation.isError ? <ErrorState error={fullReadMutation.error} /> : null}
          {downloadMutation.isError ? <ErrorState error={downloadMutation.error} title="Download failed" /> : null}
          {fullReadMutation.data ? <><ProtocolNotice status={fullReadMutation.data.status} gaps={fullReadMutation.data.gaps?.length} conflicts={fullReadMutation.data.conflicts?.length} /><JsonView value={fullReadMutation.data.data} label="Full source read" /></> : null}
        </Section>
      </TabPanel>
      <TabPanel id="outline" active={activeTab}>
        <Section title="Outline">{source.outline?.length ? <ol className="outline-list">{source.outline.map((item) => <li key={item.locator} style={{ paddingLeft: `${Math.max(0, (item.level ?? 1) - 1) * 16}px` }}><code>{item.locator}</code><span>{item.title}</span></li>)}</ol> : <EmptyState title="No outline" />}</Section>
      </TabPanel>
      <TabPanel id="claims" active={activeTab}>
        <Section title="Derived claims">{source.derived_claims?.length ? <div className="record-list">{source.derived_claims.map((claim) => <article key={claim.id}><header><strong>{claim.predicate}</strong><StatusBadge status={claim.status} /></header>{claim.value !== undefined ? <JsonView value={claim.value} /> : null}<footer><code>{claim.id}</code></footer></article>)}</div> : <EmptyState title="No derived claims" />}</Section>
      </TabPanel>
      <TabPanel id="versions" active={activeTab}>
        <Section title="Versions">{source.versions?.length ? <div className="version-list">{source.versions.map((version) => <div key={version.version}><strong>Version {version.version}</strong><span>{formatDate(version.created_at)}</span><p>{version.change_summary ?? "No change summary"}</p></div>)}</div> : <EmptyState title="No versions" />}</Section>
      </TabPanel>
      <TabPanel id="policy" active={activeTab}>
        <Section title="Effective policy">{source.policy ? <><DefinitionList items={[{ label: "Policy", value: source.policy.name }, { label: "ID", value: <code>{source.policy.id}</code> }, { label: "Version", value: source.policy.version ?? "Unknown" }, { label: "Status", value: <StatusBadge status={source.policy.status} /> }]} />{source.policy.rules ? <JsonView value={source.policy.rules} /> : null}</> : <EmptyState title="No policy projection" />}</Section>
      </TabPanel>
      <TabPanel id="audit" active={activeTab}>
        <Section title="Audit events">{source.audit?.length ? <div className="audit-list">{source.audit.map((event) => <div key={event.id}><time>{formatDate(event.created_at)}</time><strong>{event.action}</strong><span>{event.actor ?? "System"}</span><StatusBadge status={event.status} /></div>)}</div> : <EmptyState title="No audit events" />}</Section>
      </TabPanel>
    </Page>
  );
}
