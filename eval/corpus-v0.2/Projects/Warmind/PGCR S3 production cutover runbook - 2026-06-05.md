Created: 2026-06-05 08:25 PDT
Updated: 2026-06-06
Status: draft production runbook; mode-map and monitoring gates added

Related: [[Projects/Warmind/Warmind|Warmind]], [[Projects/Warmind/Final parser perf and full validation handoff - 2026-06-04|Final parser perf and full validation handoff]], [[Projects/Warmind/Final parser validation use guide - 2026-06-03|Final parser validation use guide]], [[Projects/Warmind/PGCR archive validation - 2026-06-01|PGCR archive validation]]

# PGCR S3 production cutover runbook - 2026-06-05

This is the step-by-step production cutover plan for the converged D2 parser that also runs D1, the new parser table shape, and the new canonical PGCR S3 archive generation.

Do not execute the production-write steps until Rourke explicitly approves the exact maintenance window, production DB target, production S3 bucket/prefix, and parser commit.

## Current decision

- Old D1 and D2 S3 PGCR archives are not valid input. They are both corrupt and in the old/wrong shape.
- The cutover creates a new canonical archive generation. It does not repair old S3 objects in place.
- During initial cutover, the parser must fetch from Bungie/live DB truth, not from old S3.
- During initial cutover, `s3-prefix` and `s3-write-prefix` must point at the same new clean generation.
- `s3-first` is disabled until the new generation has verified canonical coverage for the ranges the parser will read.
- Parser DB continuity and S3 archive generation are separate decisions. The parser does not need to stop at a special PGCR id just because S3 starts a new generation.
- Live production parser must run normal production side effects. `parser.side-effects = "parser-only"` is acceptable for a shadow/clone proof only; it is not acceptable for live Charlemagne because it can miss carries, activity state, realtime population, and other user-facing parser side effects.
- D2 mode metadata is parser data. The reviewed Nyx mode map may be used as the cutover source when it has been rechecked against Nyx Redis and intentionally contains the latest reviewed fixes. Current production `d2modes:*` should still be inspected read-only for newer human edits before overwriting or rebuilding production Redis.
- For D2, "PGCR rollup/archive companion" means the PGCR cache-to-S3 rollup path. It can be run by the full `warmind-cortex` service with `features.pgcr-rollup=true`, or by the smaller standalone `dev/tools/d2-pgcr-rollup` entrypoint. For a minimal production cutover, prefer the standalone PGCR rollup/archive process unless the rest of `warmind-cortex` is intentionally being changed too.

## Do not violate

- Do not touch production DB or production S3 outside the approved window.
- Do not write production S3 unless Rourke explicitly approves.
- Do not set an old S3 prefix as a read/source prefix while writing a new prefix.
- Do not migrate old S3 archive data into new truth.
- Do not use validation-derived tables or manual repair rows to hide parser drift.
- Do not let the old D2 parser keep writing after adding `raw_pgcrs.status`; old failure writes would default to `parsed`.
- Do not run the old D1 parser after dropping `d1_nexus.raw_failed_pgcrs.terminal`.
- Do not run live production parser with `parser.side-effects = "parser-only"`.
- Do not rebuild `d2modes:*` Redis from manifest/default DB rows. Use a verified reviewed source: either the current Nyx reviewed mode map or current production Redis after read-only inspection.
- Do not manually mark modes reviewed from names alone. If a mode routes summaries or product stats differently, update code/tests and reparse the affected rows.
- Do not start the production cutover until Datadog monitors/dashboards have been checked against the changed parser and PGCR rollup metric names.

## Phase 0 - Freeze the target

1. Confirm the production parser commit.

   ```sh
   cd /Users/Shared/projects/warmind-code-intel
   git fetch --all --prune
   git rev-parse --abbrev-ref HEAD
   git rev-parse HEAD
   git status --short --branch --untracked-files=all
   git diff --check
   ```

2. Record the exact commit in the handoff note before touching production.

3. Build and test the production candidate from that exact commit.

   ```sh
   go test ./parser ./nexusdb ./bungie ./parserconfig ./d1parser ./sweeperbot -count=1
   go test ./d1cortex ./d1nexusdb ./pgcrarchive ./dev/tools/pgcr-archive-verify ./dev/tools/d2-pgcr-rollup -count=1
   go build ./cmd/warmind-parser ./cmd/warmind-d1-cortex ./dev/tools/d2-pgcr-rollup ./dev/tools/pgcr-archive-verify
   go build -tags devtools ./dev/tools/d2-mode-maps
   ```

4. Stop here if the commit is dirty, unpushed, unreviewed, or no longer matches the intended parser/schema change set.

## Phase 1 - Rehearse on Nyx/dev first

1. Confirm Nyx/dev schema before rehearsal.

   ```sql
   SELECT id
   FROM nexus.schema_migrations
   ORDER BY id;

   SELECT COLUMN_NAME
   FROM information_schema.COLUMNS
   WHERE TABLE_SCHEMA = 'nexus'
     AND TABLE_NAME = 'raw_pgcrs'
   ORDER BY ORDINAL_POSITION;
   ```

2. Do not trust an older Nyx schema snapshot. Recheck live Nyx/dev before rehearsal and record whether it has the new parser shape, including `20260605_0001_pgcr_parser_shape`, `20260605_0002_pgcr_failed_status_backfill`, and `20260605_0003_raw_pgcr_starting_phase_signed`.

3. Apply the same migration sequence to dev before production. The D2 migration must include the legacy failed-marker conversion in Phase 3.

4. Run a short dev parser/rollup/archive verifier pass after migration. Do not proceed to production if dev cannot parse, roll up, and verify a fresh new-prefix archive window.

## Phase 2 - Production prechecks

Run these as read-only checks first. Capture the output with timestamps.

1. Confirm D2 table shape before migration.

   ```sql
   SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT
   FROM information_schema.COLUMNS
   WHERE TABLE_SCHEMA = 'nexus'
     AND TABLE_NAME IN ('raw_pgcrs', 'raw_failed_pgcrs', 'cache_pgcrs')
   ORDER BY TABLE_NAME, ORDINAL_POSITION;
   ```

2. Confirm D1 table shape before migration.

   ```sql
   SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT
   FROM information_schema.COLUMNS
   WHERE TABLE_SCHEMA = 'd1_nexus'
     AND TABLE_NAME IN ('raw_pgcrs', 'raw_failed_pgcrs', 'cache_pgcrs')
   ORDER BY TABLE_NAME, ORDINAL_POSITION;
   ```

3. Count legacy D2 failed raw markers that must not be left as `status='parsed'`.

   ```sql
   SELECT COUNT(*) AS d2_failed_marker_rows
   FROM nexus.raw_pgcrs r
   JOIN nexus.raw_failed_pgcrs f ON f.instanceId = r.instanceId
   WHERE r.mode = 255;
   ```

4. Check whether the old D1 `terminal` column still exists.

   ```sql
   SELECT COUNT(*) AS has_d1_terminal_column
   FROM information_schema.COLUMNS
   WHERE TABLE_SCHEMA = 'd1_nexus'
     AND TABLE_NAME = 'raw_failed_pgcrs'
     AND COLUMN_NAME = 'terminal';
   ```

5. If `has_d1_terminal_column = 1`, count rows that still depend on it.

   ```sql
   SELECT COUNT(*) AS d1_terminal_failed_rows
   FROM d1_nexus.raw_failed_pgcrs
   WHERE terminal = 1;
   ```

6. Record current parser cursors and row edges.

   ```sql
   SELECT COALESCE(MAX(instanceId), 0) AS d2_max_raw FROM nexus.raw_pgcrs;
   SELECT COALESCE(MAX(instanceId), 0) AS d1_max_raw FROM d1_nexus.raw_pgcrs;
   SELECT COUNT(*) AS d2_cache_rows FROM nexus.cache_pgcrs;
   SELECT COUNT(*) AS d1_cache_rows FROM d1_nexus.cache_pgcrs;
   SELECT COUNT(*) AS d2_failed_rows FROM nexus.raw_failed_pgcrs;
   SELECT COUNT(*) AS d1_failed_rows FROM d1_nexus.raw_failed_pgcrs;
   ```

7. Stop here if any precheck fails or if the production schema does not match the migration assumptions.

8. Confirm D2 mode metadata and Redis cache before migration.

   ```sql
   SELECT COUNT(*) AS d2_activity_mode_maps
   FROM nexus.activity_mode_maps;

   SELECT modeType, bungieName, rootModeStr, nexusRollup, nexusDisplayName,
          levelerScoreTuple, levelerMinPlayers, rankBaseId, rankGroupMode,
          rankGroupModes, source, reviewStatus
   FROM nexus.activity_mode_maps
   WHERE modeType IN (2,3,4,5,6,7,16,17,40,82,84,85,87,93)
   ORDER BY modeType;

   SELECT mode, summarized, COUNT(*) AS rows
   FROM nexus.raw_new_modes
   GROUP BY mode, summarized
   ORDER BY mode, summarized;
   ```

   Run the DB-vs-Redis drift check from the deploy candidate with production config:

   ```sh
   go run -tags devtools ./dev/tools/d2-mode-maps -check-redis
   ```

   Stop if Redis and `nexus.activity_mode_maps` disagree in a way that would change `newMode`, display names, root mode, rollup, leveler fields, or ranking fields. Do not start parser until the mode map gate is understood.

   2026-06-06 Nyx check: live Nyx dev had `nexus.activity_mode_maps=74`, `raw_new_modes=0`, key fixed modes `2`, `6`, `40`, `78`, `82`, `85`, `87`, and `93` reviewed with `newMode=0`, and `d2-mode-maps -check-redis` passed. Re-run this check before using Nyx as the production seed.

9. Confirm D1 mode metadata if D1 parser is part of the same cutover.

   ```sql
   SELECT COUNT(*) AS d1_activity_mode_maps
   FROM d1_nexus.activity_mode_maps;

   SELECT modeType, bungieName, rootModeStr, nexusRollup, reviewStatus
   FROM d1_nexus.activity_mode_maps
   WHERE modeType IN (2,3,6,10,12,13)
   ORDER BY modeType;
   ```

   Stop if the D1 table is empty or missing ordinary modes. Run the D1 manifest/cache preparation path before launching D1 parser.

10. Run the Datadog metric/monitor preflight.

   The old D2 parser baseline emitted these monitor-facing names:

   - `warmind.pgcrs_parsed`
   - `warmind.pgcrsXProfs`
   - `warmind.preWadar`
   - `warmind.job_channel`
   - `warmind.retry_channel`
   - `warmind.sleep_mod`
   - `warmind.pause_spins`
   - `warmind.err_rate`
   - `parser.workerloop`
   - `cortex.pgcr_rollup`
   - `cortex.pgcr_rollup_parts`

   Current D2 still emits the first seven names, plus the core parser error/success counters. The current code no longer emits `warmind.err_rate` or `parser.workerloop`, and the old `cortex.pgcr_rollup` / `cortex.pgcr_rollup_parts` timings have been replaced by more specific PGCR rollup metrics such as `cortex.pgcr_rollup_drain`, `cortex.pgcr_rollup_success`, `cortex.pgcr_rollup_windows_completed`, `cortex.pgcr_rollup_pgcrs_per_sec`, and `cortex.pgcr_rollup_worker_window`, all tagged `game:d2`.

   The current parser also adds many new D2/D1 metrics under `warmind.parser.*`, `parser.*`, `warmind.d1_parser.*`, `parser.d1_*`, `cortex.d1_*`, and `cortex.pgcr_s3_repair_*`.

   This metric comparison was made against parser commit `089aff86d9b6edc05c39c5f3af4d63ae46b1b621`. Before production, confirm the actual deployed production commit and repeat the diff if it differs. Either update affected Datadog monitors/dashboards or add compatibility metric emissions for the removed names. Do not assume dashboards using `warmind.err_rate`, `parser.workerloop`, `cortex.pgcr_rollup`, or `cortex.pgcr_rollup_parts` will continue to work.

## Phase 3 - Apply schema with old parsers fenced

1. Stop or fence old parser writers before schema write steps.

   Fenced means stopped and unable to write parser/PGCR state. The service must be stopped, confirmed gone from the process list, and prevented from automatic restart by systemd/supervisor/container scaling/config. A process that is merely "not currently busy" is not fenced.

   Required fenced writers:

   - D2 parser
   - D2 PGCR rollup/archive writer, whether it is the `features.pgcr-rollup` path inside `warmind-cortex` or the standalone `warmind-d2-pgcr-rollup` process
   - D1 parser
   - D1 Cortex/archive writer

2. Confirm no old parser writer is running before applying DDL.

3. Apply D2 migration `20260605_0001_pgcr_parser_shape.sql`, but make sure the migration includes this conversion before new parser startup:

   ```sql
   UPDATE nexus.raw_pgcrs r
   JOIN nexus.raw_failed_pgcrs f ON f.instanceId = r.instanceId
   SET r.status = 'parse_failed'
   WHERE r.mode = 255
     AND r.status = 'parsed';
   ```

4. Verify the D2 conversion.

   ```sql
   SELECT r.status, COUNT(*) AS rows
   FROM nexus.raw_pgcrs r
   WHERE r.mode = 255
   GROUP BY r.status
   ORDER BY r.status;

   SELECT COUNT(*) AS bad_legacy_failed_markers
   FROM nexus.raw_pgcrs r
   JOIN nexus.raw_failed_pgcrs f ON f.instanceId = r.instanceId
   WHERE r.mode = 255
     AND r.status = 'parsed';
   ```

   The second query must return `0`.

5. Apply D1 migration `0012_drop_failed_terminal_flag.sql` only after the D1 terminal precheck is understood. If D1 has real terminal rows, confirm they are represented by the new D1 raw/cache status shape before dropping the column.

6. Apply D1 migration `0013_cache_repair_columns.sql`.

7. Verify schema after migration.

   D2 changed tables:

   - `nexus.raw_pgcrs`: adds `referenceId`, `directorActivityHash`, `modesJson`, `startingPhaseIndex`, `activityWasPrivate`, `status`, `source`, `rawSha256`, `createdAt`, `updatedAt`, plus indexes for reference/activity/status.
   - `nexus.raw_failed_pgcrs`: adds `bungieErrorCode`, `bungieErrorStatus`, hardens retry/failure fields, and adds `lastFail_numFails`.
   - `nexus.cache_pgcrs`: requires the existing `s3Repair` and `s3RepairReason` columns from the prior cache-repair migration, then adds `sha256`, `byteSize`, `createdAt`, and the `createdAt` index.

   D1 changed tables:

   - `d1_nexus.raw_failed_pgcrs`: drops `terminal`, drops the terminal index, and adds `lastFail_numFails`.
   - `d1_nexus.cache_pgcrs`: adds `s3Repair` and `s3RepairReason`.

8. Stop here if the schema verification does not match the new parser code.

## Phase 3A - Mode-map cutover gate

This phase must happen before production parser startup and before any manifest refresh that would materialize `d2modes:*` from DB defaults.

1. Treat D2 mode metadata as parser truth. A bad mode decision can commit rows successfully while sending weapons, carries, population, and summary rows to the wrong product buckets.

2. Choose the mode-map source deliberately.

   Preferred for this cutover, if the recheck still passes: use the reviewed Nyx `nexus.activity_mode_maps` table as the seed. That table is the Rhea production export plus the validation fixes made on Nyx/iota.

   Export from Nyx after the recheck:

   ```sh
   docker exec warmind-dev-mysql-1 \
     mysqldump -uwarmind -pwarmind_dev \
       --single-transaction --skip-triggers --no-create-info \
       --complete-insert --replace \
       nexus activity_mode_maps \
     > activity_mode_maps.nyx-reviewed.sql

   shasum -a 256 activity_mode_maps.nyx-reviewed.sql
   ```

   Import that SQL into production `nexus.activity_mode_maps` only after the table exists from migration and the production write window is approved. Then rebuild production Redis from DB:

   ```sh
   go run -tags devtools ./dev/tools/d2-mode-maps -rebuild-redis
   go run -tags devtools ./dev/tools/d2-mode-maps -check-redis
   ```

3. If production Redis may contain newer reviewed human edits than Nyx, inspect production `d2modes:*` read-only before importing Nyx. If it differs intentionally, reconcile those edits into the Nyx export or choose production Redis as the source.

   Production Redis seed path, only if deliberately chosen:

   ```sh
   go run -tags devtools ./dev/tools/d2-mode-maps -seed-from-redis
   ```

   Use production config and production Redis. The `d2-mode-maps` helper scans Redis cluster masters; if using any other export path, it must also scan all primaries. Do not trust a single-node `SCAN`.

4. Rebuild Redis from DB only after the DB has the chosen reviewed decisions.

   ```sh
   go run -tags devtools ./dev/tools/d2-mode-maps -rebuild-redis
   go run -tags devtools ./dev/tools/d2-mode-maps -check-redis
   ```

5. Verify ordinary reviewed modes and important parser-routing modes.

   ```sql
   SELECT modeType, bungieName, rootModeStr, nexusRollup, nexusDisplayName,
          levelerScoreTuple, levelerMinPlayers, rankBaseId, rankGroupMode,
          rankGroupModes, source, reviewStatus
   FROM nexus.activity_mode_maps
   WHERE modeType IN (2,3,4,5,6,7,16,17,40,82,84,85,87,93)
   ORDER BY modeType;
   ```

6. Confirm `raw_new_modes` is understood before launch.

   ```sql
   SELECT mode, summarized, COUNT(*) AS rows
   FROM nexus.raw_new_modes
   GROUP BY mode, summarized
   ORDER BY mode, summarized;
   ```

   Existing `raw_new_modes` rows are not automatically wrong, but each active mode needs an explanation. Do not clear rows just to make the launch clean.

7. Some common-looking aggregate modes can intentionally remain `newMode=1` if they were not reviewed. Do not "fix" those from their names alone.

8. For D1, confirm `d1_nexus.activity_mode_maps` and the D1 Redis mode cache are populated before D1 parser launch. If D1 mode metadata is empty, run the D1 Cortex manifest/cache preparation path first; do not let ordinary modes permafail as unknown.

9. Stop here if any mode-map drift remains unexplained.

## Phase 4 - Choose the new S3 generation

1. Pick a new generation name that clearly cannot collide with old archives.

   Example:

   ```text
   pgcr-v2/2026-06-cutover/
   ```

2. Set read and write prefixes to the same new generation for initial production cutover.

   D2 shape:

   ```toml
   [d2-nexus]
   s3-prefix = "pgcr-v2/2026-06-cutover/"
   s3-write-prefix = "pgcr-v2/2026-06-cutover/"
   s3-target = "production"
   s3-write-confirm = "PROD_D2_PGCR_BUCKET_CONFIRMED"
   ```

   D1 shape:

   ```toml
   [d1-nexus]
   s3-prefix = "d1/pgcr-v2/2026-06-cutover/"
   s3-write-prefix = "d1/pgcr-v2/2026-06-cutover/"
   s3-target = "production"
   s3-write-confirm = "PROD_D1_PGCR_BUCKET_CONFIRMED"
   ```

3. Do not configure old read prefix plus new write prefix. That shape can feed corrupt old archive data into new repair/write logic.

4. Keep the old S3 prefixes read-only and quarantined. They are historical artifacts, not runtime truth.

## Phase 5 - Choose the archive generation floor

This floor is for archive generation and rollup alignment. It is not the parser start cursor.

1. Compute the first full future 1000-PGCR archive window after the current DB edge.

   D2:

   ```sql
   SELECT
     max_id,
     CASE
       WHEN max_id = 0 THEN 1
       ELSE FLOOR((max_id - 1) / 1000) * 1000 + 1001
     END AS next_archive_generation_floor
   FROM (
     SELECT COALESCE(MAX(instanceId), 0) AS max_id
     FROM nexus.raw_pgcrs
   ) s;
   ```

   D1:

   ```sql
   SELECT
     max_id,
     CASE
       WHEN max_id = 0 THEN 1
       ELSE FLOOR((max_id - 1) / 1000) * 1000 + 1001
     END AS next_archive_generation_floor
   FROM (
     SELECT COALESCE(MAX(instanceId), 0) AS max_id
     FROM d1_nexus.raw_pgcrs
   ) s;
   ```

2. Record the D1 and D2 floors in the handoff note.

3. Configure D2 rollup and D1 Cortex/archive cursoring to begin new archive writes at the chosen floor.

4. Do not force parser DB start to that floor unless this is an intentional replay/backfill. Normal production parser should resume from DB/Redis truth.

## Phase 6 - Configure parser source policy

1. Initial production source policy must be one of:

   ```toml
   [parser]
   source-policy = "auto"
   ```

   or:

   ```toml
   [parser]
   source-policy = "bungie-only"
   ```

2. Do not use `s3-first` during initial cutover.

3. Confirm the parser logs identify the production target, the new S3 prefix, and the intended source policy before allowing work to proceed.

4. Live production must use normal production side effects.

   ```toml
   [parser]
   side-effects = "full"
   ```

   Do not use `parser-only` for live production cutover. That would keep parser/archive truth moving while product-facing Charlemagne side effects fall behind.

5. Confirm side-effect flags match the production plan. Do not use validation-only flags or dev-S3 confirmation strings in production.

## Phase 7 - Start the new parser

1. Start the new parser binary from the frozen commit.

2. Confirm startup reads DB/Redis truth and does not use old S3 source data.

3. Watch the first live rows.

   Required D2 checks:

   ```sql
   SELECT COUNT(*) AS cache_without_raw
   FROM nexus.cache_pgcrs c
   LEFT JOIN nexus.raw_pgcrs r ON r.instanceId = c.instanceId
   WHERE r.instanceId IS NULL;

   SELECT COUNT(*) AS failed_without_raw
   FROM nexus.raw_failed_pgcrs f
   LEFT JOIN nexus.raw_pgcrs r ON r.instanceId = f.instanceId
   WHERE r.instanceId IS NULL;

   SELECT status, source, COUNT(*) AS rows
   FROM nexus.raw_pgcrs
   WHERE instanceId >= ?
   GROUP BY status, source
   ORDER BY status, source;

   SELECT mode, summarized, COUNT(*) AS rows
   FROM nexus.raw_new_modes
   WHERE instanceId >= ?
   GROUP BY mode, summarized
   ORDER BY mode, summarized;
   ```

   Required D1 checks:

   ```sql
   SELECT COUNT(*) AS cache_without_raw
   FROM d1_nexus.cache_pgcrs c
   LEFT JOIN d1_nexus.raw_pgcrs r ON r.instanceId = c.instanceId
   WHERE r.instanceId IS NULL;

   SELECT COUNT(*) AS failed_without_raw
   FROM d1_nexus.raw_failed_pgcrs f
   LEFT JOIN d1_nexus.raw_pgcrs r ON r.instanceId = f.instanceId
   WHERE r.instanceId IS NULL;
   ```

4. Stop here if cache rows can exist without raw rows, failed rows can exist without raw rows, logs show reads from the old S3 prefix, or new-mode rows spike for ordinary reviewed modes.

## Phase 8 - Start rollup and D1 Cortex archive writing

1. Start D2 PGCR rollup/archive at the chosen D2 archive floor.

   For the minimal cutover, prefer the standalone `dev/tools/d2-pgcr-rollup` process. If using full `warmind-cortex`, confirm only the intended Cortex workers are enabled.

2. Start D1 Cortex/archive writer at the chosen D1 archive floor.

3. Confirm archive output is only written under the new prefix.

4. Confirm the new archive format is canonical `pgcr-jsonl-zstd-v1`:

   - payload object is written first;
   - object SHA and size are verified;
   - manifest is published last;
   - manifest points to the payload object;
   - per-record hashes and source fields are present.

5. Confirm rollup does not read old S3 as source.

## Phase 9 - Verify new S3 before trusting it

1. Run the archive verifier against the new D2 prefix only.

2. Run the archive verifier against the new D1 prefix only.

3. Required verifier result:

   - zero missing objects;
   - zero mismatches;
   - zero checksum errors;
   - zero wrong-game or wrong-range manifests;
   - manifest object and payload object hashes match.

4. Cross-check DB truth for verified windows.

   D2:

   ```sql
   SELECT COUNT(*) AS bad_mode255_status
   FROM nexus.raw_pgcrs r
   JOIN nexus.raw_failed_pgcrs f ON f.instanceId = r.instanceId
   WHERE r.mode = 255
     AND r.status = 'parsed';

   SELECT COUNT(*) AS missing_raw_sha
   FROM nexus.raw_pgcrs
   WHERE instanceId BETWEEN ? AND ?
     AND status = 'parsed'
     AND rawSha256 IS NULL;
   ```

   D1:

   ```sql
   SELECT COUNT(*) AS failed_without_raw
   FROM d1_nexus.raw_failed_pgcrs f
   LEFT JOIN d1_nexus.raw_pgcrs r ON r.instanceId = f.instanceId
   WHERE r.instanceId IS NULL;
   ```

5. Keep `s3-first` disabled if any verifier or DB check fails.

## Phase 10 - Enable S3-first only after proof

1. Enable `s3-first` only after the new generation has complete verified canonical coverage for the exact ranges the parser will read.

2. Prefer a narrow canary first.

3. After enabling `s3-first`, confirm these behaviors:

   - readable canonical batches are used as source;
   - whole missing or unreadable batches fall back to normal Bungie/cache work;
   - stale or missing slots inside a readable batch go through S3 repair;
   - repair rows are only used to repair an existing canonical batch;
   - rollup/verifier pass after repair drain.

4. If any canary fails, revert source policy to `auto` or `bungie-only` and keep the new S3 generation write-only until fixed.

## Phase 11 - Historical S3 backfill is separate

Do not treat historical S3 rebuild as part of the live cutover unless it has its own approved plan.

Acceptable historical rebuild sources:

- exact raw PGCR bytes from trusted local cache;
- fresh Bungie refetch;
- a trusted source ledger that includes raw bytes and hashes.

Unacceptable historical rebuild sources:

- old corrupt S3 archives;
- DB metadata alone;
- summary tables;
- validation-derived rows.

For each historical window:

1. Rebuild exactly one canonical 1000-PGCR window.
2. Validate every record id, raw byte hash, byte size, game id, and source.
3. Compare parser summary truth where relevant, but do not rewrite DB truth to satisfy old archive bytes.
4. Write to the new generation only after validation passes.
5. Verify the manifest and payload with the archive verifier.
6. Record the rebuilt range and input source.

If fresh raw bytes disagree with existing DB summaries, that is parser/data drift to investigate. It is not an S3 cutover problem to paper over.

## Rollback and fallback rules

If migration fails before new parser start:

- keep old parsers stopped;
- inspect partial DDL state because MySQL DDL autocommits;
- do not restart old D2 parser if `raw_pgcrs.status` was added and old failure writes can default to `parsed`;
- do not restart old D1 parser if `raw_failed_pgcrs.terminal` was dropped.

If new parser fails after schema migration:

- keep source policy on `auto` or `bungie-only`;
- pause rollup/archive writers if S3 output is suspect;
- fix or roll forward the new parser;
- restart old parser only after proving schema compatibility with the old write path.

If S3 writes fail:

- keep parser on Bungie/live source;
- pause archive rollup if needed;
- do not enable `s3-first`;
- quarantine any partial new-prefix range until verifier passes.

If verifier fails:

- quarantine the failed new-prefix window;
- keep old S3 quarantined;
- keep source policy on `auto` or `bungie-only`;
- repair from committed DB/cache/raw bytes or Bungie refetch only.

## Done criteria

The cutover is not complete until all of these are true:

- target commit, binary hashes, config, and schema migrations are recorded;
- old parser writers were fenced before incompatible schema changes;
- production parser used full side effects, not parser-only;
- D2 mode metadata was seeded from the verified reviewed source, preferably the current Nyx reviewed mode map unless production Redis has newer intentional edits;
- D2 `d2modes:*` Redis matches the DB-backed mode map before parser startup;
- D1 mode metadata is populated if D1 parser is in the cutover;
- Datadog monitors/dashboards were updated or compatibility metrics were added for removed/renamed parser and PGCR rollup metrics;
- D2 legacy mode-255 failed rows are not left as `status='parsed'`;
- D1 terminal-column removal is accounted for;
- new parser writes current PGCR truth from Bungie/live source;
- D2 rollup writes canonical batches to the new prefix;
- D1 Cortex/archive writer writes canonical batches to the new prefix;
- D2 verifier passes on the new prefix;
- D1 verifier passes on the new prefix;
- logs show no old-prefix reads during initial cutover;
- `s3-first` remains disabled until the new generation has verified coverage for the read range;
- final DB/Redis/process/S3 hygiene is recorded in the Warmind handoff note.
