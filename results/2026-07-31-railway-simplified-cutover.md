# Railway simplified-schema owner cutover — 2026-07-31

Status: **production cutover and repository publication complete**. This
is a privacy-safe aggregate record. It contains no owner paths, note text,
credentials, or per-entry manifest rows. Machine-readable evidence is in the
companion [`JSON`](2026-07-31-railway-simplified-cutover.json).

## Outcome so far

The least-loss layered migration is complete. It preserved the verified legacy
history/native composite, overlaid the exact fresh Markdown and binary source,
retained the history of ten moved or absent paths, imported current agent
memory, and recovered an additional dormant Aether backup corpus before
archiving its old live source.

Codex and Aether/OpenClaw passed fresh production canaries. The guarded
embedding backfill, final one-replica worker qualification, and web deployment
also pass. Evidence commit
`dff91a210293483d95c9ea61c7bab865b5a60f49` is published on `origin/main`.
The restore drill was environment-blocked on locked Nyx and is
recorded honestly as not performed, non-blocking for this direct owner cutover.
The non-blocking follow-ups are listed explicitly below rather than being
collapsed into the publication status.

## Production API

- Deployment `6388d74a-000c-4faa-a924-16069e5b4c6c` is `SUCCESS` at build
  `39761166d21b0cfa44d11e3ba18a52112693d0cd`.
- Health and readiness pass; database, object store, and embeddings report
  ready; all 56 migrations are applied.
- The ordinary request limit is restored to 600/minute. Legacy and evaluation
  APIs are disabled, wrong variable names were removed, and all three disabled
  route probes return 404.
- Context-shaping treatments are off. The operational embedding cache,
  backfill guard, and timing instrumentation remain on.
- Dreaming remains off.
- Web deployment `316d90eb-d807-4091-84d4-8ba10b49a2f2` succeeded; `/` returns
  200 and its proxied API reports the intended build.
- Railway Pro is active. Its subscription update was confirmed and carries a
  $20/month minimum; that is infrastructure spend, not embedding spend.
- The database volume was live-resized from 5 GB to 20 GB. The checked-in
  topology declares 20,000 MB.
- Temporary two-replica finalizer deployment `0792432f` succeeded. Permanent
  worker deployment `7af78da7-3b01-4a66-9923-3aa8184d1978` is `SUCCESS` with
  exactly one replica running; prior worker deployments were removed. Its
  expected image digest is
  `a3c84d4ecfa228f4eb3ba5ac85cfce563d54bbfb03f773579d31b4d51879e85e`.

## Historical fidelity

The composite fingerprint is
`4e55b2582aa56944a3e1bf1f076faef426131afb91309716820f65c11b313ffd`.
The production service audit matched all 4,926 legacy paths, 4,955 legacy
versions, 5,079 native records, and 10,038 remote history versions with zero
differences.

The full round trip exported 20,047 copies totaling 797,775,263 bytes. Manifest
SHA-256
`de37b0df888e2c1ddc6644eea8665592cd2f2c1ca7113fa001b39a04cd143941`
matched with zero differences.

## Exact fresh source overlay

The fresh capture has 4,267 regular files and 298,682,825 bytes: 3,557 text and
710 binary. Its direct import fingerprint is
`5278bbc282201d47d870666c2243547c8d7600135427cb0f57cf4a2f451dafd3`.

The disjoint delta was 4,173 exact skips, 12 metadata-only changes, 21 content
changes, 61 additions, and 10 absent/moved old paths. The first overlay skipped
4,173 and uploaded exactly 94; the replay verification skipped all 4,267 and
uploaded zero. All ten old paths were soft-deleted, their history remained, and
their replacements are active.

A post-cutover source re-audit reproduced exactly 4,267 files, 298,682,825
bytes, and the same fingerprint.

## Agent memory preservation

The primary agent-memory capture contains 398 files and 5,373,439 bytes: 329
text and 69 binary. Fingerprint
`64d33e0c4263cf2344594e160933b99850c4b4c0965cb7525f6ac0a5955ec09c`
passed import and replay verification. The 69 binaries received deterministic
archival descriptions with zero inference API calls.

The additional dormant Aether backup capture contains 2,793 files and
93,627,020 bytes: 2,415 text and 378 binary. Its source fingerprint is
`8271693f85254bdf349d5536f740c4107208be17c2cba070e055ba8a948f93b1`;
its wrapped import fingerprint is
`34697fa8408dc96a3890532436aafc9eff44c112d892f8daf8c0a42ab5102990`.
It includes 2,386 files not byte-identical to earlier captures. Import and
replay verification passed, then the old live source was archived.

## Worker backfill and final qualification

The pre-worker snapshot had 13,702 active entries, 13,831 history versions, and
12,727 queued jobs: 12,658 embeddings and 69 deterministic no-model
descriptions. Backfill finished at zero queued, zero running, and zero failed
jobs. The resulting service has 126,536 search chunks and zero missing
embeddings. Current counts are 13,709 active entries, ten deleted current paths
retained in history, and 13,838 history versions.

The old worker on the 5 GB volume produced PostgreSQL `53100` through
`2026-07-31T23:53:28.867017Z`, but no job failed. Across the superseded worker
runs, 1,030 jobs completed: 69 deterministic descriptions and 961 embeddings.
The permanent worker on the 20 GB volume produced no `53100`, error, fatal, or
job-failure event. After the queue was already zero, its foreground guard
correctly paused 118 times from 2026-08-01 00:04:55 through 00:05:55 UTC while
an intentionally slow broad-open sample ran; no warning or error appeared
after 2026-08-01 00:05:56 UTC.

Fresh one-replica qualification ran 30 opens and 30 exact searches with zero
failures. Service p95 was 31.809529 ms for open against the 120 ms limit and
29.295206 ms for search against the 107 ms limit.

## Production storage

The final filesystem is 18.3 GiB: 4.6 GiB used, 13.6 GiB free, 25% utilization.
`PGDATA` is 4,924,295,483 bytes and `pg_wal` is 805,318,656 bytes. PostgreSQL
reports 4,094,842,547 database bytes, of which the Straylight schema accounts
for 4,081,106,944 bytes.

The original fresh source was 298,682,825 bytes, but production is not a
one-copy representation of that source. It retains additional agent memory and
history and now has 13,709 active entries. About 12,551 active Markdown entries
expand into 126,536 overlapping search chunks using a 1,600-character target
and 220-character overlap. Each chunk stores text, a full-text vector, metadata,
and a 1,536-dimensional float embedding of about 6,148 bytes, plus indexes.

The largest measured relation is `search_chunks` at 2,424,954,880 bytes. That
total already includes approximately 1,175,175,168 bytes of TOAST storage,
920,354,816 bytes for the simplified HNSW index, and 75,956,224 bytes for the
GIN full-text index; those nested values must not be added to the relation total
again. Legacy embeddings use 721,092,608 bytes, already including their
326,213,632-byte HNSW. Legacy chunks use 170,991,616 bytes. Historical
`corpus_members` uses 483,811,328 bytes for 2,493,355 membership rows and is not
a disposable cache.

TOAST is PostgreSQL's compressed and/or out-of-line storage for oversized
column values. HNSW is an approximate-nearest-neighbor graph index over
embeddings, while GIN is the inverted index used for full-text search.

Both HNSW indexes report `idx_scan=0` since July 25 because the semantic lane
and legacy API are disabled. They are distinct, derived, rebuildable
accelerators—not authoritative source or history—and were not dropped.
Rebuilding an index from retained vectors costs database CPU/I/O, while
deleting and later recreating the embedding values would incur embedding API
work. `VACUUM ANALYZE` passed on `search_chunks` and `jobs`.

The safe storage follow-up is: prevent new embedding enqueue while semantic
retrieval is off; after a successful restore proof, drop the legacy HNSW; then
decide whether semantic retrieval is shipping before retaining or dropping the
simplified HNSW. Retain `corpus_members` until restore-backed legacy retirement.

## Client cutover

Both clients use separate credentials and pinned wrappers with private roots.
The MCP bundle SHA-256 is
`2c7e200f2ee015cdb69ab0b0a8ad86b96391ea6573be8e4b3e2001719b8cb39c`.
Their old client wiring, vault symlink, and retired local-memory, report, and
backup persistence paths are absent. The source vault itself remains intact as
read-only recovery evidence; neither client is configured to write to it.

Codex passed fresh `open`, exact `read`, `write`, idempotency replay, and
checkpoint canaries. Its stale write returned HTTP 409
`entry_version_conflict`, as required.

Aether/OpenClaw's strict post-archive rerun passed cross-read, byte-identical
path/ref replay without a new write, checkpoint/resume, no-delivery, and
no-API-key-reasoning checks. Seven calls produced zero failures, fallbacks, or
outbound events. Checkpoint `7e1c41e2-6577-2bc6-2997-a5ac1c2083db` resumed.
The normal-gateway MCP read passed; the gateway and channels are healthy/running,
the memory plugin is disabled, and retired live memory, report, vault-link, and
backup paths are absent. The post-gateway source re-audit remains exactly 4,267
files, 298,682,825 bytes, and the same direct fingerprint.

Automation retirement is explicit. OpenClaw has 22 jobs: three safe jobs are
enabled and 19 old vault/local-memory jobs are disabled. Codex has five active
automations, including the two rewritten to use Straylight; its legacy Gmail
automation remains paused. Absolute-retired-path scans pass across every active
job file.

## Safety, cost, and remaining work

The pre-cutover PostgreSQL dump remains 273,563,054 bytes with SHA-256
`e7e61af0656747ffc7edd3dafb4273b2a5781d7b1ee029f137185e7afd617137`;
catalog validation passed. A restore drill was attempted but could not start:
locked Nyx prevented Docker daemon access and no restore container was created.
The dump checksum/catalog were reverified. This is recorded as
`not_performed_environment_blocked` and is non-blocking for the owner-directed
production cutover, not misreported as a pass.

Reasoning used only the owner's ChatGPT-authenticated Codex plan. Observed
inference API calls for archival descriptions were zero. The updated absolute
embedding upper bound is $3.61, including at most $0.084 for the newly found
backup corpus without assuming deduplication. Actual provider billing is
unavailable, but the absolute bound remains below the $20 embedding-warning
threshold. The separate $20 Railway Pro minimum charge is infrastructure spend.

Final verification passes 79 targeted tests, 28 MCP tests, 18 web tests, and
10 Railway contract tests after the checked-in 20 GB topology change.

GitHub's scheduled Dependabot configuration is removed, 21 bot pull requests
are closed, and zero pull requests remain open. GitHub rejected every job in
the last CI run before executing any step because the account has failed
payments or an Actions spending limit that must be increased. CI therefore
remains deliberately disabled so it cannot recreate the failed-build emails.
Local verification is authoritative for this publication; re-enable hosted CI
only after GitHub billing is repaired.

The operational cutover and repository publication are complete. The
non-blocking follow-ups are:

1. Complete a PostgreSQL-plus-S3 restore drill when Nyx Docker is available.
2. Rotate the locally configured Tavily API key because its existing value was
   exposed in diagnostic output; the value is intentionally absent here.
3. Apply the storage policy above and finish synthetic-monitor qualification.
4. Repair GitHub Actions billing before re-enabling hosted CI.

The honest verdict is **production cutover complete**.
