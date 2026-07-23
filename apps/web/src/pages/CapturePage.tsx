import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  ArchiveRestore,
  Check,
  FileArchive,
  FileUp,
  RefreshCw,
  Save,
  Sparkles,
  UploadCloud,
} from "lucide-react";
import { type FormEvent, useMemo, useState } from "react";
import { DefinitionList, Page, PageHeader, Section } from "../components/Page";
import { JsonView } from "../components/JsonView";
import { ErrorState, ReadOnlyNotice, StatusBadge } from "../components/StateViews";
import { TabPanel, Tabs } from "../components/Tabs";
import { useApi } from "../lib/auth";
import { useCurrent, useReadOnly } from "../lib/current";
import { formatBytes, shortId } from "../lib/format";
import type { CaptureReceipt, CommitReceipt, JsonObject, JsonValue, StageReceipt } from "../lib/types";

function operationId(prefix: string): string {
  return `${prefix}_${globalThis.crypto?.randomUUID?.() ?? `${Date.now()}_${Math.random().toString(16).slice(2)}`}`;
}

function collectItems(value: JsonValue | undefined, kind: string): JsonObject[] {
  if (!Array.isArray(value)) return [];
  return value
    .filter((item): item is JsonObject => Boolean(item && typeof item === "object" && !Array.isArray(item)))
    .map((item) => ({ ...item, kind: item.kind ?? kind }));
}

function kindFromRef(reference: string): string {
  const prefix = reference.split(":", 1)[0];
  if (prefix === "source_episode") return "source";
  if (["object", "claim", "relation", "source", "evidence"].includes(prefix)) return prefix;
  return "object";
}

function ReceiptView({ receipt, title = "Commit receipt" }: { receipt: CommitReceipt; title?: string }) {
  return (
    <Section title={title} actions={<Check size={18} aria-hidden="true" />}>
      <div className="receipt-heading">
        <StatusBadge status={receipt.status} />
        <code>{receipt.id}</code>
      </div>
      <DefinitionList items={[
        { label: "Operation", value: receipt.operation_id ? <code>{receipt.operation_id}</code> : "Not returned" },
        { label: "Base revision", value: <code>{receipt.base_corpus_revision ?? "Not returned"}</code> },
        { label: "Result revision", value: <code>{receipt.resulting_corpus_revision ?? "Not returned"}</code> },
        { label: "Created", value: receipt.created?.length ?? 0 },
        { label: "Revised", value: receipt.revised?.length ?? 0 },
        { label: "Deduplicated", value: receipt.deduplicated?.length ?? 0 },
        { label: "Review required", value: receipt.review_required?.length ?? 0 },
      ]} />
      {receipt.indexing ? <div className="index-status-list">{Object.entries(receipt.indexing).map(([index, status]) => <div key={index}><span>{index}</span><StatusBadge status={status} /></div>)}</div> : null}
    </Section>
  );
}

function CaptureReceiptView({ receipt }: { receipt: CaptureReceipt }) {
  const summary = receipt.capture_summary ?? receipt.summary ?? "Capture processed";
  return (
    <Section title="Capture receipt" actions={<Sparkles size={18} aria-hidden="true" />}>
      <div className="receipt-heading">
        <StatusBadge status={receipt.status} />
        {receipt.id ? <code>{receipt.id}</code> : null}
      </div>
      <DefinitionList items={[
        { label: "Summary", value: summary },
        { label: "Source", value: receipt.source_ref ? <code>{receipt.source_ref}</code> : "Not committed" },
        { label: "Created", value: receipt.created?.length ?? 0 },
        { label: "Revised", value: receipt.revised?.length ?? 0 },
        { label: "Review issues", value: receipt.issues?.length ?? 0 },
        { label: "Unresolved", value: receipt.unresolved?.length ?? 0 },
      ]} />
      {receipt.issues?.length ? <JsonView value={receipt.issues} label="Capture issues" /> : null}
      {receipt.draft?.save_request ? <JsonView value={receipt.draft.save_request} label="Canonical save draft" /> : null}
    </Section>
  );
}

function StageReceiptView({
  receipt,
  selected,
  onToggle,
}: {
  receipt: StageReceipt;
  selected: Set<string>;
  onToggle: (path: string) => void;
}) {
  return (
    <Section title="Stage receipt" actions={<FileArchive size={18} aria-hidden="true" />}>
      <div className="receipt-heading"><StatusBadge status={receipt.status} /><code>{receipt.id}</code></div>
      <DefinitionList items={[
        { label: "Input hash", value: <code>{shortId(receipt.input_hash, 20)}</code> },
        { label: "Inventory hash", value: <code>{shortId(receipt.inventory_hash, 20)}</code> },
        { label: "Entries", value: receipt.entries?.length ?? 0 },
      ]} />
      {receipt.entries?.length ? (
        <div className="stage-entry-list">
          {receipt.entries.map((entry) => (
            <label key={entry.path}>
              <input
                type="checkbox"
                checked={selected.has(entry.path)}
                onChange={() => onToggle(entry.path)}
                disabled={["rejected", "quarantined", "unreadable"].includes(entry.status ?? "")}
              />
              <div><strong>{entry.path}</strong><span>{entry.content_type ?? "Unknown type"} · {formatBytes(entry.size_bytes)}</span></div>
              <StatusBadge status={entry.status ?? entry.disposition} />
            </label>
          ))}
        </div>
      ) : null}
    </Section>
  );
}

export function CapturePage() {
  const api = useApi();
  const current = useCurrent();
  const readOnly = useReadOnly();
  const queryClient = useQueryClient();
  const [activeTab, setActiveTab] = useState("capture");
  const [captureContent, setCaptureContent] = useState("");
  const [captureOrigin, setCaptureOrigin] = useState("user");
  const [captureIntent, setCaptureIntent] = useState("");
  const [captureRootRef, setCaptureRootRef] = useState("");
  const [captureSourceRef, setCaptureSourceRef] = useState("");
  const [captureDraft, setCaptureDraft] = useState(false);
  const [captureKey, setCaptureKey] = useState(() => operationId("capture"));
  const [action, setAction] = useState("create");
  const [targetRef, setTargetRef] = useState("");
  const [expectedVersion, setExpectedVersion] = useState("");
  const [sourceTitle, setSourceTitle] = useState("");
  const [sourceKind, setSourceKind] = useState("user_input");
  const [payloadText, setPayloadText] = useState("{\n  \"objects\": [],\n  \"claims\": [],\n  \"relations\": []\n}");
  const [idempotencyKey, setIdempotencyKey] = useState(() => operationId("save"));
  const [parseError, setParseError] = useState<string | null>(null);
  const [file, setFile] = useState<File | null>(null);
  const [importIdentity, setImportIdentity] = useState(() => operationId("import"));
  const [selectedEntries, setSelectedEntries] = useState<Set<string>>(new Set());
  const [promoteKey, setPromoteKey] = useState(() => operationId("promote"));

  const baseRevision = current.data.corpus_revision ?? current.corpus_revision ?? "";
  const captureMutation = useMutation({
    mutationFn: (payload: JsonObject) => api.capture(payload),
    onSuccess: (result) => {
      setCaptureKey(operationId("capture"));
      if (["committed", "no_op"].includes(result.status)) setCaptureContent("");
      if (result.status === "committed") {
        void queryClient.invalidateQueries({ queryKey: ["me"] });
        void queryClient.invalidateQueries({ queryKey: ["sessions"] });
        void queryClient.invalidateQueries({ queryKey: ["service-status"] });
        void queryClient.invalidateQueries({ queryKey: ["dreams"] });
      }
    },
  });
  const saveMutation = useMutation({
    mutationFn: (payload: JsonObject) => api.save(payload),
    onSuccess: (result) => {
      setIdempotencyKey(operationId("save"));
      void queryClient.invalidateQueries({ queryKey: ["me"] });
      void queryClient.invalidateQueries({ queryKey: ["sessions"] });
      if (result.data.resulting_corpus_revision) {
        void queryClient.invalidateQueries({ queryKey: ["service-status"] });
      }
    },
  });
  const stageMutation = useMutation({
    mutationFn: (body: FormData) => api.stage(body),
    onSuccess: (result) => {
      setSelectedEntries(
        new Set(
          (result.data.entries ?? [])
            .filter((entry) => !["rejected", "quarantined", "unreadable"].includes(entry.status ?? ""))
            .map((entry) => entry.path),
        ),
      );
    },
  });
  const promoteMutation = useMutation({
    mutationFn: () =>
      api.save({
        intent: "stage_promotion",
        operation_id: operationId("op"),
        idempotency_key: promoteKey,
        base_corpus_revision: baseRevision,
        scope: current.data.active_scope?.id ?? "authorized",
        root_refs: [],
        source_refs: [],
        items: [{
          action: "create",
          kind: "import_receipt",
          payload: {
            stage_ref: stageMutation.data?.data.id ?? "",
            selected_entries: Array.from(selectedEntries),
          },
        }],
      }),
    onSuccess: () => {
      setPromoteKey(operationId("promote"));
      void queryClient.invalidateQueries({ queryKey: ["me"] });
      void queryClient.invalidateQueries({ queryKey: ["sessions"] });
    },
  });

  const destructive = action === "tombstone";
  const mutationDisabled = readOnly;
  const stagedReceipt = stageMutation.data?.data;
  const promotionReady = Boolean(stagedReceipt && selectedEntries.size);

  const saveActionLabel = useMemo(() => {
    if (action === "tombstone") return "Initiate deletion";
    if (action === "retract") return "Retract assertion";
    if (action === "supersede") return "Supersede";
    if (action === "revise") return "Save revision";
    return "Save change";
  }, [action]);

  function submitSave(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setParseError(null);
    let content: JsonValue;
    try {
      content = JSON.parse(payloadText) as JsonValue;
    } catch (error) {
      setParseError(error instanceof Error ? error.message : "Invalid JSON payload");
      return;
    }
    const contentObject = content && typeof content === "object" && !Array.isArray(content)
      ? content as JsonObject
      : {};
    const suppliedItems = Array.isArray(contentObject.items)
      ? contentObject.items.filter((item): item is JsonObject => Boolean(item && typeof item === "object" && !Array.isArray(item)))
      : [];
    const sectionItems = suppliedItems.length ? suppliedItems : [
      ...collectItems(contentObject.objects, "object"),
      ...collectItems(contentObject.claims, "claim"),
      ...collectItems(contentObject.relations, "relation"),
    ];
    const derivedItems = action === "tombstone" && targetRef
      ? [{
          action: "tombstone",
          kind: kindFromRef(targetRef),
          ref: targetRef,
          payload: { reason: sourceTitle, source_text: payloadText },
        }]
      : sectionItems.map((item) => ({
          action: item.action ?? action,
          kind: item.kind ?? "object",
          ...(item.ref || targetRef ? { ref: item.ref ?? targetRef } : {}),
          ...(item.expected_version || expectedVersion
            ? { expected_version: Number(item.expected_version ?? expectedVersion) }
            : {}),
          payload: {
            ...((item.payload && typeof item.payload === "object" && !Array.isArray(item.payload)
              ? item.payload
              : item) as JsonObject),
            source_text: payloadText,
          },
        }));
    const sourceRef = `capture:${idempotencyKey}`;
    const items = [{
      action: "create",
      kind: "source",
      payload: {
        source_ref: sourceRef,
        source_kind: sourceKind,
        title: sourceTitle,
        path: `${sourceRef}.json`,
        media_type: "application/json",
        content: payloadText,
      },
    }, ...derivedItems];
    saveMutation.mutate({
      intent: `capture:${sourceKind}`,
      operation_id: operationId("op"),
      idempotency_key: idempotencyKey,
      base_corpus_revision: baseRevision,
      scope: current.data.active_scope?.id ?? "authorized",
      root_refs: targetRef ? [targetRef] : [],
      source_refs: [{ ref: sourceRef }],
      items,
    });
  }

  function submitCapture(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (readOnly || !captureContent.trim()) return;
    captureMutation.mutate({
      content: captureContent,
      source: {
        ...(captureSourceRef.trim() ? { ref: captureSourceRef.trim() } : {}),
        ...(sourceTitle.trim() ? { title: sourceTitle.trim() } : {}),
        kind: sourceKind,
        origin: captureOrigin,
        media_type: "text/plain",
      },
      scope: current.data.active_scope?.id ?? "authorized",
      root_refs: captureRootRef.trim() ? [captureRootRef.trim()] : [],
      ...(captureIntent.trim() ? { intent: captureIntent.trim() } : {}),
      idempotency_key: captureKey,
      base_corpus_revision: baseRevision,
      mode: captureDraft ? "draft" : "auto",
    });
  }

  function submitStage(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!file || readOnly) return;
    const body = new FormData();
    body.set("file", file);
    body.set("stable_import_id", importIdentity);
    body.set("scope", current.data.active_scope?.id ?? "authorized");
    stageMutation.mutate(body);
  }

  function toggleEntry(path: string) {
    setSelectedEntries((currentSelection) => {
      const next = new Set(currentSelection);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  return (
    <Page>
      <PageHeader title="Capture" description="Sources and durable records" />
      {readOnly ? <ReadOnlyNotice /> : null}
      <Tabs tabs={[{ id: "capture", label: "Capture" }, { id: "save", label: "Canonical save" }, { id: "stage", label: "Stage and import" }]} active={activeTab} onChange={setActiveTab} />

      <TabPanel id="capture" active={activeTab}>
        <Section title="Source capture">
          <form className="form-grid" onSubmit={submitCapture}>
            <label className="field"><span>Source title</span><input value={sourceTitle} onChange={(event) => setSourceTitle(event.target.value)} disabled={mutationDisabled} required={!captureSourceRef.trim()} /></label>
            <label className="field"><span>Existing source reference</span><input value={captureSourceRef} onChange={(event) => setCaptureSourceRef(event.target.value)} disabled={mutationDisabled} placeholder="source:..." /></label>
            <label className="field"><span>Source kind</span><input value={sourceKind} onChange={(event) => setSourceKind(event.target.value)} disabled={mutationDisabled} /></label>
            <label className="field"><span>Origin</span><select value={captureOrigin} onChange={(event) => setCaptureOrigin(event.target.value)} disabled={mutationDisabled}><option value="user">User</option><option value="external">External source</option><option value="agent">Agent</option><option value="tool">Tool result</option><option value="system">System observation</option></select></label>
            <label className="field"><span>Intent</span><input value={captureIntent} onChange={(event) => setCaptureIntent(event.target.value)} disabled={mutationDisabled} placeholder="remember_this" /></label>
            <label className="field"><span>Root reference</span><input value={captureRootRef} onChange={(event) => setCaptureRootRef(event.target.value)} disabled={mutationDisabled} placeholder="object:..." /></label>
            <label className="field field-span-2"><span>Content</span><textarea rows={10} value={captureContent} onChange={(event) => setCaptureContent(event.target.value)} disabled={mutationDisabled} required /></label>
            <label className="field field-span-2"><span>Idempotency key</span><div className="input-with-button"><input value={captureKey} onChange={(event) => setCaptureKey(event.target.value)} disabled={mutationDisabled} required /><button className="icon-button" type="button" onClick={() => setCaptureKey(operationId("capture"))} disabled={mutationDisabled} aria-label="Regenerate idempotency key" title="Regenerate"><RefreshCw size={16} /></button></div></label>
            <label className="checkbox-field field-span-2"><input type="checkbox" checked={captureDraft} onChange={(event) => setCaptureDraft(event.target.checked)} disabled={mutationDisabled} /><span>Draft only</span></label>
            {captureMutation.isError ? <div className="field-span-2"><ErrorState error={captureMutation.error} title="Capture failed" /></div> : null}
            <div className="form-actions field-span-2">
              <button className="button primary" type="submit" disabled={mutationDisabled || !captureContent.trim() || (!sourceTitle.trim() && !captureSourceRef.trim()) || !captureKey.trim() || captureMutation.isPending} title={mutationDisabled ? "Requires a read/write credential" : undefined}>
                <Sparkles size={17} />
                {captureMutation.isPending ? "Capturing" : captureDraft ? "Build draft" : "Capture"}
              </button>
            </div>
          </form>
        </Section>
        {captureMutation.data ? <CaptureReceiptView receipt={captureMutation.data.data} /> : null}
      </TabPanel>

      <TabPanel id="save" active={activeTab}>
        <Section title="Canonical mutation">
          <form className="form-grid" onSubmit={submitSave}>
            <label className="field"><span>Action</span><select value={action} onChange={(event) => setAction(event.target.value)} disabled={mutationDisabled}><option value="create">Create</option><option value="revise">Revise</option><option value="supersede">Supersede</option><option value="retract">Retract</option><option value="tombstone">Tombstone</option></select></label>
            <label className="field"><span>Target reference</span><input value={targetRef} onChange={(event) => setTargetRef(event.target.value)} disabled={mutationDisabled} placeholder="Required for existing records" /></label>
            <label className="field"><span>Expected target version</span><input type="number" min="1" value={expectedVersion} onChange={(event) => setExpectedVersion(event.target.value)} disabled={mutationDisabled} /></label>
            <label className="field"><span>Base corpus revision</span><input value={baseRevision} readOnly /></label>
            <label className="field"><span>Source title</span><input value={sourceTitle} onChange={(event) => setSourceTitle(event.target.value)} disabled={mutationDisabled} required /></label>
            <label className="field"><span>Source kind</span><input value={sourceKind} onChange={(event) => setSourceKind(event.target.value)} disabled={mutationDisabled} /></label>
            <label className="field field-span-2"><span>Content (JSON)</span><textarea className="code-input" rows={12} value={payloadText} onChange={(event) => setPayloadText(event.target.value)} disabled={mutationDisabled} spellCheck={false} /></label>
            <label className="field field-span-2"><span>Idempotency key</span><div className="input-with-button"><input value={idempotencyKey} onChange={(event) => setIdempotencyKey(event.target.value)} disabled={mutationDisabled} required /><button className="icon-button" type="button" onClick={() => setIdempotencyKey(operationId("save"))} disabled={mutationDisabled} aria-label="Regenerate idempotency key" title="Regenerate"><RefreshCw size={16} /></button></div></label>
            {parseError ? <span className="field-error field-span-2" role="alert">{parseError}</span> : null}
            {saveMutation.isError ? <div className="field-span-2"><ErrorState error={saveMutation.error} title="Save failed" /></div> : null}
            <div className="form-actions field-span-2">
              <button className={`button ${destructive ? "danger" : "primary"}`} type="submit" disabled={mutationDisabled || !sourceTitle.trim() || !idempotencyKey.trim() || saveMutation.isPending} title={mutationDisabled ? "Requires a read/write credential" : undefined}>
                {destructive ? <ArchiveRestore size={17} /> : <Save size={17} />}
                {saveMutation.isPending ? "Saving" : saveActionLabel}
              </button>
            </div>
          </form>
        </Section>
        {saveMutation.data ? <ReceiptView receipt={saveMutation.data.data} /> : null}
      </TabPanel>

      <TabPanel id="stage" active={activeTab}>
        <Section title="Stage content pack">
          <form className="form-grid" onSubmit={submitStage}>
            <label className="field field-span-2 file-field"><span>File or archive</span><input type="file" onChange={(event) => setFile(event.target.files?.[0] ?? null)} disabled={mutationDisabled} required /><FileUp size={20} aria-hidden="true" />{file ? <strong>{file.name} · {formatBytes(file.size)}</strong> : <strong>Select content</strong>}</label>
            <label className="field"><span>Import identity</span><input value={importIdentity} onChange={(event) => setImportIdentity(event.target.value)} disabled={mutationDisabled} /></label>
            <label className="field"><span>Base corpus revision</span><input value={baseRevision} readOnly /></label>
            <div className="form-actions field-span-2"><button className="button primary" type="submit" disabled={mutationDisabled || !file || stageMutation.isPending} title={mutationDisabled ? "Requires a read/write credential" : undefined}><UploadCloud size={17} />{stageMutation.isPending ? "Staging" : "Stage"}</button></div>
          </form>
          {stageMutation.isError ? <ErrorState error={stageMutation.error} title="Staging failed" /> : null}
        </Section>
        {stagedReceipt ? <StageReceiptView receipt={stagedReceipt} selected={selectedEntries} onToggle={toggleEntry} /> : null}
        {stagedReceipt ? (
          <Section title="Import selection" meta={`${selectedEntries.size} selected`}>
            <div className="form-actions">
              <button className="button primary" type="button" onClick={() => promoteMutation.mutate()} disabled={mutationDisabled || !promotionReady || promoteMutation.isPending} title={mutationDisabled ? "Requires a read/write credential" : undefined}><Save size={17} />{promoteMutation.isPending ? "Importing" : "Import selected"}</button>
              <code>{promoteKey}</code>
            </div>
            {promoteMutation.isError ? <ErrorState error={promoteMutation.error} title="Import failed" /> : null}
          </Section>
        ) : null}
        {promoteMutation.data ? <ReceiptView receipt={promoteMutation.data.data} title="Import commit receipt" /> : null}
      </TabPanel>
    </Page>
  );
}
