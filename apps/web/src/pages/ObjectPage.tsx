import { useQuery } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import { ExternalLink, Fingerprint, History, Network } from "lucide-react";
import { useState } from "react";
import { JsonView } from "../components/JsonView";
import { DefinitionList, Page, PageHeader, Section } from "../components/Page";
import { EmptyState, ErrorState, LoadingState, ProtocolNotice, StatusBadge } from "../components/StateViews";
import { TabPanel, Tabs } from "../components/Tabs";
import { useApi } from "../lib/auth";
import { formatDate, humanize, shortId } from "../lib/format";

export function ObjectPage() {
  const { objectId } = useParams({ from: "/authenticated/objects/$objectId" });
  const api = useApi();
  const [activeTab, setActiveTab] = useState("overview");
  const objectQuery = useQuery({
    queryKey: ["object", objectId],
    queryFn: () => api.object(objectId),
  });

  if (objectQuery.isPending) return <Page><LoadingState label="Loading object" /></Page>;
  if (objectQuery.isError) return <Page><ErrorState error={objectQuery.error} retry={() => void objectQuery.refetch()} /></Page>;
  const object = objectQuery.data.data;
  const tabs = [
    { id: "overview", label: "Overview" },
    { id: "profiles", label: "Profiles", count: object.profiles.length },
    { id: "claims", label: "Claims", count: object.claims?.length ?? 0 },
    { id: "relations", label: "Relations", count: object.relations?.length ?? 0 },
    { id: "states", label: "States", count: object.states?.length ?? 0 },
    { id: "timeline", label: "Timeline", count: object.timeline?.length ?? 0 },
    { id: "evidence", label: "Evidence", count: object.evidence?.length ?? 0 },
    { id: "versions", label: "Versions", count: object.versions?.length ?? 0 },
    { id: "policy", label: "Policy" },
    { id: "audit", label: "Audit", count: object.audit?.length ?? 0 },
  ];

  return (
    <Page>
      <PageHeader
        title={object.display_name}
        description={`Object ${object.id}`}
        actions={<StatusBadge status={object.states?.find((state) => state.machine === "work.execution")?.value ?? object.profiles[0]} />}
      />
      <ProtocolNotice status={objectQuery.data.status} gaps={objectQuery.data.gaps?.length} conflicts={objectQuery.data.conflicts?.length} />
      <Tabs tabs={tabs} active={activeTab} onChange={setActiveTab} />

      <TabPanel id="overview" active={activeTab}>
        <div className="two-column-layout">
          <Section title="Identity" actions={<Fingerprint size={18} aria-hidden="true" />}>
            <DefinitionList items={[
              { label: "Object ID", value: <code>{object.id}</code> },
              { label: "Version", value: object.version },
              { label: "Updated", value: formatDate(object.updated_at) },
              { label: "Profiles", value: object.profiles.map(humanize).join(", ") },
            ]} />
          </Section>
          <Section title="Summary">
            <p className="prose-value">{object.summary ?? "No summary is recorded."}</p>
          </Section>
        </div>
        <Section title="Properties">
          <JsonView value={object.properties ?? {}} label="Object properties" />
        </Section>
      </TabPanel>

      <TabPanel id="profiles" active={activeTab}>
        <Section title="Type profiles">
          <div className="profile-list">
            {object.profiles.map((profile) => <div key={profile}><strong>{profile}</strong><span>{humanize(profile)}</span></div>)}
          </div>
        </Section>
      </TabPanel>

      <TabPanel id="claims" active={activeTab}>
        <Section title="Claims">
          {object.claims?.length ? <div className="record-list">{object.claims.map((claim) => (
            <article key={claim.id}>
              <header><strong>{claim.predicate}</strong><StatusBadge status={claim.status} /></header>
              {claim.value !== undefined ? <JsonView value={claim.value} label={`Value for ${claim.predicate}`} /> : null}
              <footer><code>{shortId(claim.id)}</code>{claim.authority ? <span>{claim.authority}</span> : null}{claim.confidence !== undefined ? <span>{Math.round(claim.confidence * 100)}%</span> : null}</footer>
            </article>
          ))}</div> : <EmptyState title="No claims" />}
        </Section>
      </TabPanel>

      <TabPanel id="relations" active={activeTab}>
        <Section title="Qualified relations" actions={<Network size={18} aria-hidden="true" />}>
          {object.relations?.length ? <div className="record-list">{object.relations.map((relation) => (
            <article key={relation.id}>
              <header><strong>{relation.predicate}</strong><StatusBadge status={relation.status} /></header>
              <div className="endpoint-list">{relation.endpoints.map((endpoint, index) => <div key={`${endpoint.role}-${endpoint.object_id}-${index}`}><span>{humanize(endpoint.role)}</span><Link to="/objects/$objectId" params={{ objectId: endpoint.object_id }}>{endpoint.label ?? endpoint.object_id}</Link></div>)}</div>
              {relation.qualifiers ? <JsonView value={relation.qualifiers} label={`Qualifiers for ${relation.predicate}`} /> : null}
            </article>
          ))}</div> : <EmptyState title="No relations" />}
        </Section>
      </TabPanel>

      <TabPanel id="states" active={activeTab}>
        <Section title="Independent state machines">
          {object.states?.length ? <div className="state-list">{object.states.map((state) => <div key={state.machine}><div><strong>{state.machine}</strong><span>{formatDate(state.effective_at)}</span></div><StatusBadge status={state.value} /></div>)}</div> : <EmptyState title="No state assignments" />}
        </Section>
      </TabPanel>

      <TabPanel id="timeline" active={activeTab}>
        <Section title="Timeline" actions={<History size={18} aria-hidden="true" />}>
          {object.timeline?.length ? <ol className="timeline">{object.timeline.map((item, index) => <li key={item.id ?? `${item.at}-${index}`}><time>{formatDate(item.at)}</time><strong>{item.label}</strong>{item.detail ? <span>{item.detail}</span> : null}</li>)}</ol> : <EmptyState title="No timeline entries" />}
        </Section>
      </TabPanel>

      <TabPanel id="evidence" active={activeTab}>
        <Section title="Evidence">
          {object.evidence?.length ? <div className="evidence-list">{object.evidence.map((evidence, index) => <div key={`${evidence.source_id}-${evidence.locator}-${index}`}><div><strong>{evidence.source_title ?? evidence.source_id ?? "Evidence"}</strong><span>{evidence.locator ?? "No locator"}</span></div>{evidence.source_id ? <Link className="icon-button" to="/sources/$sourceId" params={{ sourceId: evidence.source_id }} aria-label="Open source" title="Open source"><ExternalLink size={16} /></Link> : null}</div>)}</div> : <EmptyState title="No evidence" />}
        </Section>
      </TabPanel>

      <TabPanel id="versions" active={activeTab}>
        <Section title="Immutable versions">
          {object.versions?.length ? <div className="version-list">{object.versions.map((version) => <div key={version.version}><strong>Version {version.version}</strong><span>{formatDate(version.created_at)}</span><p>{version.change_summary ?? "No change summary"}</p><code>{version.source_refs?.join(", ") || "No source references"}</code></div>)}</div> : <EmptyState title="No version history" />}
        </Section>
      </TabPanel>

      <TabPanel id="policy" active={activeTab}>
        <Section title="Effective policy">
          {object.policy ? <><DefinitionList items={[{ label: "Policy", value: object.policy.name }, { label: "ID", value: <code>{object.policy.id}</code> }, { label: "Version", value: object.policy.version ?? "Unknown" }, { label: "Status", value: <StatusBadge status={object.policy.status} /> }]} />{object.policy.rules ? <JsonView value={object.policy.rules} label="Policy rules" /> : null}</> : <EmptyState title="No policy projection" />}
        </Section>
      </TabPanel>

      <TabPanel id="audit" active={activeTab}>
        <Section title="Audit events">
          {object.audit?.length ? <div className="audit-list">{object.audit.map((event) => <div key={event.id}><time>{formatDate(event.created_at)}</time><strong>{event.action}</strong><span>{event.actor ?? "System"}</span><StatusBadge status={event.status} /></div>)}</div> : <EmptyState title="No audit events" />}
        </Section>
      </TabPanel>
    </Page>
  );
}
