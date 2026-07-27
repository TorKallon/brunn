import { createHash, randomBytes } from "node:crypto";
import {
  chmod,
  link,
  lstat,
  mkdir,
  open,
  realpath,
  unlink,
} from "node:fs/promises";
import { constants } from "node:fs";
import { extname, isAbsolute, join, resolve } from "node:path";

export interface AssetMetadata {
  assetRef: string;
  version: number;
  contentHash: string;
  sizeBytes: number;
  mediaType: string;
  path?: string | undefined;
}

export interface AssetFetchResult extends Record<string, unknown> {
  local_path: string;
  content_hash: string;
  size_bytes: number;
  media_type: string;
}

interface RootIdentity {
  path: string;
  device: bigint;
  inode: bigint;
}

const ASSET_REFERENCE = /^(asset|entry):([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$/i;
const SHA256 = /^[0-9a-f]{64}$/i;

export function parseAssetMetadata(
  body: Record<string, unknown>,
  requestedAssetRef: string,
): AssetMetadata {
  const candidate = record(body.data) ?? body;
  const assetRef = requiredString(
    candidate.entry_ref ?? candidate.asset_ref,
    "entry_ref",
  );
  if (!ASSET_REFERENCE.test(assetRef) || assetRef !== requestedAssetRef) {
    throw new Error("asset metadata returned an unexpected entry_ref");
  }
  const version = requiredSafeInteger(candidate.version, "version");
  if (version <= 0) {
    throw new Error("asset metadata version must be positive");
  }
  const sizeBytes = requiredSafeInteger(candidate.size_bytes, "size_bytes");
  if (sizeBytes < 0) {
    throw new Error("asset metadata size_bytes must not be negative");
  }
  const mediaType = requiredString(candidate.media_type, "media_type");
  const path = typeof candidate.path === "string" && candidate.path.length > 0
    ? candidate.path
    : undefined;
  return {
    assetRef,
    version,
    contentHash: `sha256:${normalizeSha256(candidate.content_hash, "content_hash")}`,
    sizeBytes,
    mediaType,
    ...(path === undefined ? {} : { path }),
  };
}

export function configuredAssetRoot(
  environment: NodeJS.ProcessEnv = process.env,
): string {
  const configured =
    environment.CARRYSTATE_MCP_ASSET_ROOT
    ?? environment.STRAYLIGHT_MCP_ASSET_ROOT;
  if (!configured) {
    throw new Error(
      "CARRYSTATE_MCP_ASSET_ROOT is required for asset.fetch "
      + "(STRAYLIGHT_MCP_ASSET_ROOT is accepted as a legacy alias)",
    );
  }
  if (!isAbsolute(configured)) {
    throw new Error("CARRYSTATE_MCP_ASSET_ROOT must be an absolute path");
  }
  return resolve(configured);
}

export async function storeVerifiedAsset(
  response: Response,
  metadata: AssetMetadata,
  configuredRoot: string,
): Promise<AssetFetchResult> {
  await validateContentResponse(response, metadata);
  let root: RootIdentity;
  let finalPath: string;
  try {
    root = await ensurePrivateRoot(configuredRoot);
    finalPath = join(root.path, assetFilename(metadata));
    assertDirectChild(root.path, finalPath);
    const existing = await verifyExistingAsset(finalPath, metadata, root);
    if (existing) {
      await response.body?.cancel().catch(() => undefined);
      return existing;
    }
  } catch (error) {
    await response.body?.cancel().catch(() => undefined);
    throw error;
  }

  const temporaryPath = join(
    root.path,
    `.asset-${process.pid}-${randomBytes(16).toString("hex")}.tmp`,
  );
  assertDirectChild(root.path, temporaryPath);
  let temporaryExists = false;
  let bodyClaimed = false;
  let handle;
  try {
    handle = await open(
      temporaryPath,
      constants.O_CREAT
        | constants.O_EXCL
        | constants.O_WRONLY
        | constants.O_NOFOLLOW,
      0o600,
    );
    temporaryExists = true;
    const opened = await handle.stat({ bigint: true });
    if (!opened.isFile() || opened.nlink !== 1n) {
      throw new Error("asset temporary destination is not a private regular file");
    }

    const hash = createHash("sha256");
    let sizeBytes = 0;
    if (response.body === null && metadata.sizeBytes !== 0) {
      bodyClaimed = true;
      throw new Error("asset content response ended before any bytes were received");
    }
    if (response.body !== null) {
      bodyClaimed = true;
      const reader = response.body.getReader();
      try {
        while (true) {
          const item = await reader.read();
          if (item.done) {
            break;
          }
          const chunk = item.value;
          sizeBytes += chunk.byteLength;
          if (sizeBytes > metadata.sizeBytes) {
            throw new Error("asset content exceeded the API-provided size");
          }
          hash.update(chunk);
          await writeEntireBuffer(handle, chunk);
        }
      } catch (error) {
        await reader.cancel().catch(() => undefined);
        throw error;
      } finally {
        reader.releaseLock();
      }
    }

    const actualHash = hash.digest("hex");
    const expectedHash = metadata.contentHash.slice("sha256:".length);
    if (sizeBytes !== metadata.sizeBytes) {
      throw new Error(
        `asset size verification failed: expected ${metadata.sizeBytes}, received ${sizeBytes}`,
      );
    }
    if (actualHash !== expectedHash) {
      throw new Error(
        `asset SHA-256 verification failed: expected ${expectedHash}, received ${actualHash}`,
      );
    }

    await handle.chmod(0o600);
    await handle.sync();
    await handle.close();
    handle = undefined;
    await assertRootIdentity(root);

    try {
      await link(temporaryPath, finalPath);
    } catch (error) {
      if (isNodeError(error, "EEXIST")) {
        throw new Error(`asset destination already exists: ${finalPath}`);
      }
      throw error;
    }
    await unlink(temporaryPath);
    temporaryExists = false;
    await syncDirectory(root.path);

    return {
      local_path: finalPath,
      content_hash: metadata.contentHash,
      size_bytes: metadata.sizeBytes,
      media_type: metadata.mediaType,
    };
  } catch (error) {
    if (!bodyClaimed) {
      await response.body?.cancel().catch(() => undefined);
    }
    throw error;
  } finally {
    if (handle !== undefined) {
      await handle.close().catch(() => undefined);
    }
    if (temporaryExists) {
      await unlink(temporaryPath).catch(() => undefined);
    }
  }
}

async function validateContentResponse(
  response: Response,
  metadata: AssetMetadata,
): Promise<void> {
  if (response.status !== 200) {
    await response.body?.cancel().catch(() => undefined);
    throw new Error(`asset content request returned unexpected HTTP ${response.status}`);
  }
  const encoding = response.headers.get("content-encoding");
  if (encoding !== null && encoding.toLowerCase() !== "identity") {
    await response.body?.cancel().catch(() => undefined);
    throw new Error("asset content must not use a transfer content encoding");
  }
  const responseHash = normalizeSha256(
    response.headers.get("x-carrystate-sha256"),
    "x-carrystate-sha256",
  );
  if (`sha256:${responseHash}` !== metadata.contentHash) {
    await response.body?.cancel().catch(() => undefined);
    throw new Error("asset content hash header does not match asset metadata");
  }
  const contentLength = parseHeaderInteger(
    response.headers.get("content-length"),
    "content-length",
  );
  if (contentLength !== metadata.sizeBytes) {
    await response.body?.cancel().catch(() => undefined);
    throw new Error("asset content-length header does not match asset metadata");
  }
  const responseRef = response.headers.get("x-carrystate-asset-ref");
  if (responseRef !== metadata.assetRef) {
    await response.body?.cancel().catch(() => undefined);
    throw new Error("asset content response returned an unexpected asset reference");
  }
  const responseVersion = parseHeaderInteger(
    response.headers.get("x-carrystate-asset-version"),
    "x-carrystate-asset-version",
  );
  if (responseVersion !== metadata.version) {
    await response.body?.cancel().catch(() => undefined);
    throw new Error("asset content response returned an unexpected asset version");
  }
  const contentType = response.headers.get("content-type")?.split(";", 1)[0]?.trim();
  const metadataType = metadata.mediaType.split(";", 1)[0]?.trim();
  if (!contentType || !metadataType || contentType.toLowerCase() !== metadataType.toLowerCase()) {
    await response.body?.cancel().catch(() => undefined);
    throw new Error("asset content-type header does not match asset metadata");
  }
}

async function ensurePrivateRoot(configuredRoot: string): Promise<RootIdentity> {
  if (!isAbsolute(configuredRoot)) {
    throw new Error("CARRYSTATE_MCP_ASSET_ROOT must be an absolute path");
  }
  const requestedRoot = resolve(configuredRoot);
  await mkdir(requestedRoot, { recursive: true, mode: 0o700 });
  const requested = await lstat(requestedRoot, { bigint: true });
  if (requested.isSymbolicLink() || !requested.isDirectory()) {
    throw new Error("CARRYSTATE_MCP_ASSET_ROOT must be a real directory, not a symlink");
  }
  const root = await realpath(requestedRoot);
  const metadata = await lstat(root, { bigint: true });
  if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
    throw new Error("CARRYSTATE_MCP_ASSET_ROOT must resolve to a real directory");
  }
  if (typeof process.getuid === "function" && metadata.uid !== BigInt(process.getuid())) {
    throw new Error("CARRYSTATE_MCP_ASSET_ROOT must be owned by the MCP process user");
  }
  await chmod(root, 0o700);
  const privateMetadata = await lstat(root, { bigint: true });
  if ((privateMetadata.mode & 0o777n) !== 0o700n) {
    throw new Error("CARRYSTATE_MCP_ASSET_ROOT could not be made private");
  }
  return {
    path: root,
    device: privateMetadata.dev,
    inode: privateMetadata.ino,
  };
}

async function assertRootIdentity(expected: RootIdentity): Promise<void> {
  const current = await lstat(expected.path, { bigint: true });
  if (
    current.isSymbolicLink()
    || !current.isDirectory()
    || current.dev !== expected.device
    || current.ino !== expected.inode
  ) {
    throw new Error("CARRYSTATE_MCP_ASSET_ROOT changed during asset download");
  }
}

function assertDirectChild(root: string, candidate: string): void {
  if (resolve(candidate) !== candidate || resolve(root, candidate.slice(root.length + 1)) !== candidate) {
    throw new Error("asset destination escaped CARRYSTATE_MCP_ASSET_ROOT");
  }
}

async function verifyExistingAsset(
  path: string,
  metadata: AssetMetadata,
  root: RootIdentity,
): Promise<AssetFetchResult | undefined> {
  let handle;
  try {
    handle = await open(
      path,
      constants.O_RDONLY | constants.O_NOFOLLOW,
    );
  } catch (error) {
    if (isNodeError(error, "ENOENT")) {
      return undefined;
    }
    throw error;
  }
  try {
    const opened = await handle.stat({ bigint: true });
    if (
      !opened.isFile()
      || opened.nlink !== 1n
      || opened.size !== BigInt(metadata.sizeBytes)
      || (opened.mode & 0o777n) !== 0o600n
      || (
        typeof process.getuid === "function"
        && opened.uid !== BigInt(process.getuid())
      )
    ) {
      throw new Error(`existing asset destination is not a verified private file: ${path}`);
    }
    const hash = createHash("sha256");
    const buffer = Buffer.allocUnsafe(1024 * 1024);
    let offset = 0;
    while (offset < metadata.sizeBytes) {
      const result = await handle.read(
        buffer,
        0,
        Math.min(buffer.byteLength, metadata.sizeBytes - offset),
        offset,
      );
      if (result.bytesRead <= 0) {
        throw new Error(`existing asset destination is truncated: ${path}`);
      }
      hash.update(buffer.subarray(0, result.bytesRead));
      offset += result.bytesRead;
    }
    const trailing = await handle.read(buffer, 0, 1, offset);
    if (trailing.bytesRead !== 0) {
      throw new Error(`existing asset destination has unexpected trailing bytes: ${path}`);
    }
    const expectedHash = metadata.contentHash.slice("sha256:".length);
    if (hash.digest("hex") !== expectedHash) {
      throw new Error(`existing asset destination failed SHA-256 verification: ${path}`);
    }
    await assertRootIdentity(root);
    return {
      local_path: path,
      content_hash: metadata.contentHash,
      size_bytes: metadata.sizeBytes,
      media_type: metadata.mediaType,
    };
  } finally {
    await handle.close();
  }
}

async function writeEntireBuffer(
  handle: Awaited<ReturnType<typeof open>>,
  value: Uint8Array,
): Promise<void> {
  let offset = 0;
  while (offset < value.byteLength) {
    const result = await handle.write(value, offset, value.byteLength - offset);
    if (result.bytesWritten <= 0) {
      throw new Error("asset destination stopped accepting bytes");
    }
    offset += result.bytesWritten;
  }
}

async function syncDirectory(path: string): Promise<void> {
  const handle = await open(
    path,
    constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW,
  );
  try {
    await handle.sync();
  } finally {
    await handle.close();
  }
}

function assetFilename(metadata: AssetMetadata): string {
  const match = ASSET_REFERENCE.exec(metadata.assetRef);
  const uuid = match?.[2];
  if (!uuid) {
    throw new Error("entry_ref must contain a UUID");
  }
  const shortHash = metadata.contentHash
    .slice("sha256:".length, "sha256:".length + 12);
  return `${uuid.toLowerCase()}.v${metadata.version}.${shortHash}${safeExtension(metadata)}`;
}

function safeExtension(metadata: AssetMetadata): string {
  if (metadata.path) {
    const extension = extname(metadata.path);
    if (/^\.[a-z0-9]{1,12}$/i.test(extension)) {
      return extension.toLowerCase();
    }
  }
  return MEDIA_TYPE_EXTENSIONS.get(metadata.mediaType.toLowerCase()) ?? "";
}

function normalizeSha256(value: unknown, field: string): string {
  if (typeof value !== "string") {
    throw new Error(`asset ${field} must be a SHA-256 value`);
  }
  const digest = value.startsWith("sha256:") ? value.slice("sha256:".length) : value;
  if (!SHA256.test(digest)) {
    throw new Error(`asset ${field} must be a SHA-256 value`);
  }
  return digest.toLowerCase();
}

function parseHeaderInteger(value: string | null, field: string): number {
  if (value === null || !/^(0|[1-9][0-9]*)$/.test(value)) {
    throw new Error(`asset ${field} header must be a non-negative integer`);
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed)) {
    throw new Error(`asset ${field} header exceeds the safe integer range`);
  }
  return parsed;
}

function requiredSafeInteger(value: unknown, field: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value)) {
    throw new Error(`asset metadata ${field} must be a safe integer`);
  }
  return value;
}

function requiredString(value: unknown, field: string): string {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`asset metadata ${field} must be a non-empty string`);
  }
  return value;
}

function record(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined;
}

function isNodeError(error: unknown, code: string): boolean {
  return error instanceof Error && "code" in error && error.code === code;
}

const MEDIA_TYPE_EXTENSIONS = new Map<string, string>([
  ["application/json", ".json"],
  ["application/pdf", ".pdf"],
  ["application/sql", ".sql"],
  ["application/vnd.openxmlformats-officedocument.presentationml.presentation", ".pptx"],
  ["application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", ".xlsx"],
  ["application/vnd.openxmlformats-officedocument.wordprocessingml.document", ".docx"],
  ["application/zip", ".zip"],
  ["audio/mpeg", ".mp3"],
  ["image/gif", ".gif"],
  ["image/jpeg", ".jpg"],
  ["image/png", ".png"],
  ["image/webp", ".webp"],
  ["text/csv", ".csv"],
  ["text/plain", ".txt"],
  ["video/mp4", ".mp4"],
]);
