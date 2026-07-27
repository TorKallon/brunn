import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  Binary,
  Check,
  FilePenLine,
  RefreshCw,
  Save,
  UploadCloud,
} from "lucide-react";
import { type FormEvent, useState } from "react";
import { DefinitionList, Page, PageHeader, Section } from "../components/Page";
import {
  ErrorState,
  ReadOnlyNotice,
  StatusBadge,
} from "../components/StateViews";
import { TabPanel, Tabs } from "../components/Tabs";
import { useApi } from "../lib/auth";
import { useReadOnly } from "../lib/current";
import { formatBytes, shortId } from "../lib/format";
import type {
  JsonObject,
  WorkspaceBinaryReceipt,
  WorkspaceWriteReceipt,
} from "../lib/types";
import { newOperationId } from "../lib/workspace";

function WriteReceiptView({
  receipt,
  title,
}: {
  receipt: WorkspaceWriteReceipt | WorkspaceBinaryReceipt;
  title: string;
}) {
  const binary =
    "size_bytes" in receipt ? (receipt as WorkspaceBinaryReceipt) : null;
  return (
    <Section title={title} actions={<Check size={18} aria-hidden="true" />}>
      <div className="receipt-heading">
        <StatusBadge status={receipt.no_op ? "no_op" : "committed"} />
        <code>{receipt.entry_ref}</code>
      </div>
      <DefinitionList
        items={[
          { label: "Path", value: <code>{receipt.path}</code> },
          { label: "Version", value: `v${receipt.version}` },
          {
            label: "Generation",
            value: receipt.workspace_generation,
          },
          {
            label: "Hash",
            value: (
              <code title={receipt.content_hash}>
                {shortId(receipt.content_hash, 20)}
              </code>
            ),
          },
          {
            label: "Search",
            value: receipt.search_status ? (
              <StatusBadge status={receipt.search_status} />
            ) : binary ? (
              <StatusBadge status={binary.description_status} />
            ) : (
              "Ready"
            ),
          },
          ...(binary
            ? [
                {
                  label: "Size",
                  value: formatBytes(binary.size_bytes),
                },
                {
                  label: "Companion",
                  value: <code>{binary.companion.path}</code>,
                },
              ]
            : []),
        ]}
      />
    </Section>
  );
}

export function CapturePage() {
  const api = useApi();
  const queryClient = useQueryClient();
  const readOnly = useReadOnly();
  const [activeTab, setActiveTab] = useState("capture");

  const [captureContent, setCaptureContent] = useState("");
  const [captureIntent, setCaptureIntent] = useState("");
  const [captureSource, setCaptureSource] = useState("");
  const [captureAuthority, setCaptureAuthority] = useState("owner");
  const [captureKey, setCaptureKey] = useState(() =>
    newOperationId("capture"),
  );

  const [path, setPath] = useState("");
  const [content, setContent] = useState("");
  const [mediaType, setMediaType] = useState("text/markdown");
  const [expectedVersion, setExpectedVersion] = useState("");
  const [metadataText, setMetadataText] = useState("{}");
  const [metadataError, setMetadataError] = useState<string | null>(null);
  const [writeKey, setWriteKey] = useState(() => newOperationId("write"));

  const [file, setFile] = useState<File | null>(null);
  const [binaryPath, setBinaryPath] = useState("");
  const [description, setDescription] = useState("");
  const [provenance, setProvenance] = useState("");
  const [limitations, setLimitations] = useState("");

  function invalidateWorkspace() {
    void queryClient.invalidateQueries({ queryKey: ["workspace-manifest"] });
    void queryClient.invalidateQueries({ queryKey: ["workspace-changes"] });
    void queryClient.invalidateQueries({ queryKey: ["workspace-binaries"] });
    void queryClient.invalidateQueries({ queryKey: ["workspace-usage"] });
  }

  const captureMutation = useMutation({
    mutationFn: () =>
      api.workspaceCapture({
        content: captureContent.trim(),
        source: {
          ...(captureSource.trim() ? { locator: captureSource.trim() } : {}),
          authority: captureAuthority,
        },
        ...(captureIntent.trim() ? { intent: captureIntent.trim() } : {}),
        idempotency_key: captureKey,
      }),
    onSuccess: () => {
      setCaptureContent("");
      setCaptureKey(newOperationId("capture"));
      invalidateWorkspace();
    },
  });

  const writeMutation = useMutation({
    mutationFn: (metadata: JsonObject) =>
      api.workspaceWrite({
        path: path.trim(),
        content,
        media_type: mediaType,
        ...(expectedVersion
          ? { expected_version: Number(expectedVersion) }
          : {}),
        idempotency_key: writeKey,
        metadata,
      }),
    onSuccess: (response) => {
      setExpectedVersion(String(response.data.version));
      setWriteKey(newOperationId("write"));
      invalidateWorkspace();
    },
  });

  const binaryMutation = useMutation({
    mutationFn: async () => {
      if (!file) throw new Error("Select a binary file.");
      const body = new FormData();
      const digest = await crypto.subtle.digest("SHA-256", await file.arrayBuffer());
      const contentHash = Array.from(new Uint8Array(digest), (byte) =>
        byte.toString(16).padStart(2, "0"),
      ).join("");
      body.set("file", file);
      body.set("path", binaryPath.trim());
      body.set("expected_content_hash", `sha256:${contentHash}`);
      if (file.type) body.set("media_type", file.type);
      if (description.trim()) body.set("description", description.trim());
      if (provenance.trim()) body.set("provenance", provenance.trim());
      if (limitations.trim()) body.set("limitations", limitations.trim());
      return api.uploadWorkspaceBinary(body);
    },
    onSuccess: () => {
      setFile(null);
      setBinaryPath("");
      setDescription("");
      setProvenance("");
      setLimitations("");
      invalidateWorkspace();
    },
  });

  function submitCapture(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!readOnly && captureContent.trim()) captureMutation.mutate();
  }

  function submitWrite(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (readOnly || !path.trim()) return;
    setMetadataError(null);
    try {
      const parsed = JSON.parse(metadataText) as unknown;
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
        throw new Error("Metadata must be a JSON object.");
      }
      writeMutation.mutate(parsed as JsonObject);
    } catch (error) {
      setMetadataError(
        error instanceof Error ? error.message : "Metadata is not valid JSON.",
      );
    }
  }

  function submitBinary(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!readOnly && file && binaryPath.trim()) binaryMutation.mutate();
  }

  function chooseFile(nextFile: File | null) {
    setFile(nextFile);
    if (nextFile && !binaryPath.trim()) setBinaryPath(nextFile.name);
  }

  return (
    <Page>
      <PageHeader title="Write" description="Durable workspace changes" />
      {readOnly ? <ReadOnlyNotice /> : null}
      <Tabs
        tabs={[
          { id: "capture", label: "Capture" },
          { id: "markdown", label: "Markdown" },
          { id: "binary", label: "Binary" },
        ]}
        active={activeTab}
        onChange={setActiveTab}
      />

      <TabPanel id="capture" active={activeTab}>
        <Section title="Capture context">
          <form className="form-grid" onSubmit={submitCapture}>
            <label className="field">
              <span>Source</span>
              <input
                value={captureSource}
                onChange={(event) => setCaptureSource(event.target.value)}
                disabled={readOnly}
              />
            </label>
            <label className="field">
              <span>Authority</span>
              <select
                value={captureAuthority}
                onChange={(event) => setCaptureAuthority(event.target.value)}
                disabled={readOnly}
              >
                <option value="owner">Owner</option>
                <option value="source">Source</option>
                <option value="inferred">Inferred</option>
                <option value="unknown">Unknown</option>
              </select>
            </label>
            <label className="field field-span-2">
              <span>Intent</span>
              <input
                value={captureIntent}
                onChange={(event) => setCaptureIntent(event.target.value)}
                disabled={readOnly}
              />
            </label>
            <label className="field field-span-2">
              <span>Content</span>
              <textarea
                rows={12}
                value={captureContent}
                onChange={(event) => setCaptureContent(event.target.value)}
                disabled={readOnly}
                required
              />
            </label>
            <label className="field field-span-2">
              <span>Idempotency key</span>
              <div className="input-with-button">
                <input value={captureKey} readOnly />
                <button
                  className="icon-button"
                  type="button"
                  onClick={() => setCaptureKey(newOperationId("capture"))}
                  disabled={readOnly}
                  aria-label="Regenerate capture key"
                  title="Regenerate"
                >
                  <RefreshCw size={16} aria-hidden="true" />
                </button>
              </div>
            </label>
            <div className="form-actions field-span-2">
              <button
                className="button primary"
                type="submit"
                disabled={
                  readOnly ||
                  !captureContent.trim() ||
                  captureMutation.isPending
                }
              >
                <Save size={17} aria-hidden="true" />
                {captureMutation.isPending ? "Capturing" : "Capture"}
              </button>
            </div>
          </form>
          {captureMutation.isError ? (
            <ErrorState error={captureMutation.error} title="Capture failed" />
          ) : null}
        </Section>
        {captureMutation.data ? (
          <WriteReceiptView
            receipt={captureMutation.data.data}
            title="Capture receipt"
          />
        ) : null}
      </TabPanel>

      <TabPanel id="markdown" active={activeTab}>
        <Section title="Write Markdown">
          <form className="form-grid" onSubmit={submitWrite}>
            <label className="field field-span-2">
              <span>Path</span>
              <input
                value={path}
                onChange={(event) => setPath(event.target.value)}
                disabled={readOnly}
                spellCheck={false}
                required
              />
            </label>
            <label className="field">
              <span>Media type</span>
              <select
                value={mediaType}
                onChange={(event) => setMediaType(event.target.value)}
                disabled={readOnly}
              >
                <option value="text/markdown">Markdown</option>
                <option value="text/plain">Plain text</option>
              </select>
            </label>
            <label className="field">
              <span>Expected version</span>
              <input
                type="number"
                min={0}
                value={expectedVersion}
                onChange={(event) => setExpectedVersion(event.target.value)}
                disabled={readOnly}
              />
            </label>
            <label className="field field-span-2">
              <span>Content</span>
              <textarea
                className="code-input"
                rows={18}
                value={content}
                onChange={(event) => setContent(event.target.value)}
                disabled={readOnly}
                spellCheck={false}
              />
            </label>
            <label className="field field-span-2">
              <span>Metadata (JSON)</span>
              <textarea
                className="code-input"
                rows={5}
                value={metadataText}
                onChange={(event) => setMetadataText(event.target.value)}
                disabled={readOnly}
                spellCheck={false}
              />
            </label>
            {metadataError ? (
              <span className="field-error field-span-2" role="alert">
                {metadataError}
              </span>
            ) : null}
            <div className="form-actions field-span-2">
              <button
                className="button primary"
                type="submit"
                disabled={readOnly || !path.trim() || writeMutation.isPending}
              >
                <FilePenLine size={17} aria-hidden="true" />
                {writeMutation.isPending ? "Writing" : "Write"}
              </button>
            </div>
          </form>
          {writeMutation.isError ? (
            <ErrorState error={writeMutation.error} title="Write failed" />
          ) : null}
        </Section>
        {writeMutation.data ? (
          <WriteReceiptView
            receipt={writeMutation.data.data}
            title="Write receipt"
          />
        ) : null}
      </TabPanel>

      <TabPanel id="binary" active={activeTab}>
        <Section title="Upload binary">
          <form className="form-grid" onSubmit={submitBinary}>
            <label className="field field-span-2 file-field">
              <span>File</span>
              <input
                type="file"
                onChange={(event) =>
                  chooseFile(event.target.files?.[0] ?? null)
                }
                disabled={readOnly}
                required
              />
              <Binary size={20} aria-hidden="true" />
              <strong>
                {file
                  ? `${file.name} · ${formatBytes(file.size)}`
                  : "Select file"}
              </strong>
            </label>
            <label className="field field-span-2">
              <span>Workspace path</span>
              <input
                value={binaryPath}
                onChange={(event) => setBinaryPath(event.target.value)}
                disabled={readOnly}
                spellCheck={false}
                required
              />
            </label>
            <label className="field field-span-2">
              <span>Description</span>
              <textarea
                rows={5}
                value={description}
                onChange={(event) => setDescription(event.target.value)}
                disabled={readOnly}
              />
            </label>
            <label className="field">
              <span>Provenance</span>
              <textarea
                rows={4}
                value={provenance}
                onChange={(event) => setProvenance(event.target.value)}
                disabled={readOnly}
              />
            </label>
            <label className="field">
              <span>Limitations</span>
              <textarea
                rows={4}
                value={limitations}
                onChange={(event) => setLimitations(event.target.value)}
                disabled={readOnly}
              />
            </label>
            <div className="form-actions field-span-2">
              <button
                className="button primary"
                type="submit"
                disabled={
                  readOnly ||
                  !file ||
                  !binaryPath.trim() ||
                  binaryMutation.isPending
                }
              >
                <UploadCloud size={17} aria-hidden="true" />
                {binaryMutation.isPending ? "Uploading" : "Upload"}
              </button>
            </div>
          </form>
          {binaryMutation.isError ? (
            <ErrorState error={binaryMutation.error} title="Upload failed" />
          ) : null}
        </Section>
        {binaryMutation.data ? (
          <WriteReceiptView
            receipt={binaryMutation.data.data}
            title="Binary receipt"
          />
        ) : null}
      </TabPanel>
    </Page>
  );
}
