import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import test from "node:test";

import {
  type AssetMetadata,
  configuredAssetRoot,
  parseAssetMetadata,
  storeVerifiedAsset,
} from "./asset-download.js";

const ASSET_REF = "asset:019f84f2-98ce-7f1b-b680-6f7920bb4723";
const SESSION_HASH = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

test("asset metadata parsing requires exact identity and integrity fields", () => {
  assert.deepEqual(parseAssetMetadata({
    asset_ref: ASSET_REF,
    version: 4,
    content_hash: SESSION_HASH,
    size_bytes: 12,
    media_type: "application/octet-stream",
    path: "records/receipt.bin",
  }, ASSET_REF), {
    assetRef: ASSET_REF,
    version: 4,
    contentHash: SESSION_HASH,
    sizeBytes: 12,
    mediaType: "application/octet-stream",
    path: "records/receipt.bin",
  });

  assert.throws(
    () => parseAssetMetadata({
      asset_ref: "asset:019f84f2-98ce-7f1b-b680-6f7920bb4724",
      version: 4,
      content_hash: SESSION_HASH,
      size_bytes: 12,
      media_type: "application/octet-stream",
    }, ASSET_REF),
    /unexpected asset_ref/,
  );
  assert.throws(
    () => parseAssetMetadata({
      asset_ref: ASSET_REF,
      version: 4,
      content_hash: "not-a-hash",
      size_bytes: 12,
      media_type: "application/octet-stream",
    }, ASSET_REF),
    /must be a SHA-256 value/,
  );
});

test("asset root configuration prefers CarryState and accepts the legacy alias", () => {
  assert.equal(
    configuredAssetRoot({
      CARRYSTATE_MCP_ASSET_ROOT: "/private/carrystate",
      STRAYLIGHT_MCP_ASSET_ROOT: "/private/legacy",
    }),
    "/private/carrystate",
  );
  assert.equal(
    configuredAssetRoot({ STRAYLIGHT_MCP_ASSET_ROOT: "/private/legacy" }),
    "/private/legacy",
  );
  assert.throws(
    () => configuredAssetRoot({}),
    /CARRYSTATE_MCP_ASSET_ROOT is required/,
  );
  assert.throws(
    () => configuredAssetRoot({ CARRYSTATE_MCP_ASSET_ROOT: "relative/path" }),
    /must be an absolute path/,
  );
});

test("verified streaming creates private files and reuses only an exact verified destination", async () => {
  const parent = await mkdtemp(join(tmpdir(), "carrystate-asset-test-"));
  const root = join(parent, "assets");
  const bytes = Buffer.from([0, 1, 2, 3, 255, 254, 128, 64]);
  const metadata = assetMetadata(bytes, "image/png", "images/map.png");

  try {
    const result = await storeVerifiedAsset(assetResponse(bytes, metadata), metadata, root);
    assert.deepEqual(Object.keys(result).sort(), [
      "content_hash",
      "local_path",
      "media_type",
      "size_bytes",
    ]);
    assert.deepEqual(await readFile(result.local_path), bytes);
    assert.equal((await lstat(root)).mode & 0o777, 0o700);
    assert.equal((await lstat(result.local_path)).mode & 0o777, 0o600);

    const repeated = await storeVerifiedAsset(
      assetResponse(bytes, metadata),
      metadata,
      root,
    );
    assert.deepEqual(repeated, result);
    assert.deepEqual(await readFile(result.local_path), bytes);
    assert.deepEqual(
      (await readdir(root)).sort(),
      [basename(result.local_path)],
    );

    await writeFile(result.local_path, Buffer.from("replacement"), { mode: 0o600 });
    await assert.rejects(
      storeVerifiedAsset(assetResponse(bytes, metadata), metadata, root),
      /existing asset destination/,
    );
  } finally {
    await rm(parent, { recursive: true, force: true });
  }
});

test("corrupt or truncated streams never publish an asset", async () => {
  const parent = await mkdtemp(join(tmpdir(), "carrystate-asset-corrupt-"));
  const corruptRoot = join(parent, "corrupt");
  const truncatedRoot = join(parent, "truncated");
  const expected = Buffer.from("expected asset bytes");
  const metadata = assetMetadata(expected);

  try {
    const corrupt = Buffer.from("corrupted asset byte");
    const corruptMetadata = { ...metadata, sizeBytes: corrupt.byteLength };
    await assert.rejects(
      storeVerifiedAsset(
        assetResponse(corrupt, corruptMetadata),
        corruptMetadata,
        corruptRoot,
      ),
      /SHA-256 verification failed/,
    );
    assert.deepEqual(await readdir(corruptRoot), []);

    const truncated = expected.subarray(0, expected.byteLength - 1);
    await assert.rejects(
      storeVerifiedAsset(
        assetResponse(truncated, metadata, { contentLength: metadata.sizeBytes }),
        metadata,
        truncatedRoot,
      ),
      /size verification failed/,
    );
    assert.deepEqual(await readdir(truncatedRoot), []);
  } finally {
    await rm(parent, { recursive: true, force: true });
  }
});

test("asset storage rejects a symlink root without writing through it", async () => {
  const parent = await mkdtemp(join(tmpdir(), "carrystate-asset-symlink-"));
  const target = join(parent, "target");
  const root = join(parent, "assets");
  const marker = join(target, "keep.txt");
  const bytes = Buffer.from("never written through a symlink");
  const metadata = assetMetadata(bytes);

  try {
    await mkdir(target);
    await writeFile(marker, "keep");
    await symlink(target, root);
    await assert.rejects(
      storeVerifiedAsset(assetResponse(bytes, metadata), metadata, root),
      /real directory, not a symlink/,
    );
    assert.deepEqual(await readdir(target), ["keep.txt"]);
  } finally {
    await rm(parent, { recursive: true, force: true });
  }
});

function assetMetadata(
  bytes: Uint8Array,
  mediaType = "application/octet-stream",
  path?: string,
): AssetMetadata {
  return {
    assetRef: ASSET_REF,
    version: 7,
    contentHash: sha256(bytes),
    sizeBytes: bytes.byteLength,
    mediaType,
    ...(path === undefined ? {} : { path }),
  };
}

function assetResponse(
  bytes: Uint8Array,
  metadata: AssetMetadata,
  options: { contentLength?: number } = {},
): Response {
  const midpoint = Math.max(1, Math.floor(bytes.byteLength / 2));
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(bytes.subarray(0, midpoint));
      if (midpoint < bytes.byteLength) {
        controller.enqueue(bytes.subarray(midpoint));
      }
      controller.close();
    },
  });
  return new Response(body, {
    status: 200,
    headers: {
      "content-length": String(options.contentLength ?? bytes.byteLength),
      "content-type": metadata.mediaType,
      "x-carrystate-asset-ref": metadata.assetRef,
      "x-carrystate-asset-version": String(metadata.version),
      "x-carrystate-sha256": metadata.contentHash,
    },
  });
}

function sha256(bytes: Uint8Array): string {
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}
