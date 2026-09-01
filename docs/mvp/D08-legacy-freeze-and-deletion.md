# D08 — Legacy Freeze and Deletion

Status: Simplified production route, import proof, and client cutover passed; destructive legacy-code deletion deferred to a future restore-backed change
Date: 2026-07-31
Depends on: D14 (migration and authority cutover)
Gated by: zero-diff production import and restore proof before destructive legacy removal
Runtime flag: n/a

## Current decision

Railway now serves the simplified API. The old statement that the hosted
service is legacy at migration 50 while Nyx has empty simplified tables is no
longer current: Railway has all 56 migrations and the layered replay/overlay
has passed zero-diff audits. Nyx is test/operator infrastructure only.

Do not delete the legacy export/replay code while it is still part of the
recovery path. The service, full-history byte, fresh-source overlay, and both
client audits now pass. The PostgreSQL+S3 restore attempt could not start
because locked Nyx prevented Docker access. That exception does not block the
direct owner cutover, but a successful future restore and a CI-backed deletion
change remain prerequisites before legacy routes and modules can be removed as
ordinary technical debt. None of this pretends the historical reasoning
comparison proved exact parity.

## Historical evidence

The original deletion caution remains valid:

- The 57-case strict draw scored legacy 170/228, simplified 160/228, and direct
  Markdown 171/228. A targeted repeat narrowed the observed gap, but exact
  parity was not established.
- E01's n=3 comparison did not establish the specified non-inferiority margin.
- The first strict simplified/source audit found accepted evidence in 21 of 22
  disputed responses, so many misses were context-use rather than absent-byte
  failures. A fidelity audit cannot convert that reasoning result into a pass.
- The 2026-07-26 collapse showed that unbudgeted legacy bookkeeping can harm
  production. The current production build keeps dreaming and context-shaping
  treatments disabled.

These findings support retaining the simplified baseline with treatments off. They
do not justify reopening the vault as a second writable authority after the
owner-directed cutover.

## Freeze boundary

Until deletion:

- accept no new legacy feature work;
- allow only security fixes and changes required to complete or verify the
  bounded migration/recovery path;
- keep legacy usage telemetry out of the simplified schema;
- keep dreaming disabled; and
- ensure ordinary production traffic uses only `/v1/workspace/*`.

The previously proposed `legacy-core` compile-time feature has not been proven
complete and is not a current-state claim. If implemented before deletion, CI
must build both feature states. Otherwise delete the dead code once recovery no
longer depends on it rather than introducing a temporary flag during cutover.

## Deletion gates

All are required:

1. **Passed:** Railway history replay, checkpoint import, service audit, and
   downloaded 20,047-copy full-history byte audit have zero differences.
2. **Passed:** the fresh 4,267-file source overlay reproduces its exact ledger;
   all ten moved-path soft deletions retain history and active replacements.
3. **Passed:** Codex and Aether/OpenClaw pass D13 from fresh production-facing
   processes with Brunn-only durable persistence.
4. **Not yet satisfied for destructive legacy-code deletion:** a PostgreSQL
   plus versioned-S3 restore reproduces the same fidelity audits. The 2026-07-31
   attempt was environment-blocked before Docker created a container; this does
   not block the direct owner cutover.
5. **Passed for source evidence:** retained images/source and exact aggregate
   fingerprints keep the migration tooling recoverable; final publication is
   recorded with gate 6.
6. Repository CI is enabled and green for the deletion change.

The historical proposal also required a new n≥3 legacy-versus-simplified
parity experiment. The owner chose not to purchase that additional experiment
for this direct owner cutover. Record that as an explicit risk acceptance, not
as a statistical pass, when deletion is proposed.

## Removal scope after the gates

- legacy read/write services and `/v1/memory/*` route registration;
- legacy worker/dream pipeline and unused usage telemetry;
- MCP residue not reachable from the 12-tool workspace surface; and
- migration-only evaluation routes after the final retained recovery artifact
  is built.

The read-only operations SPA and the simplified `/v1/workspace/*` contract are
not part of the deletion.

## Rollout and recovery

Land deletion as one reviewable change after tagging the last migration-capable
revision. Roll back code from that tag only; do not silently route clients to a
legacy database or the vault. Any unexplained missing history, binary receipt,
checkpoint identity, or restore difference blocks deletion.

## References

- [D14 migration and authority cutover](D14-migration-and-authority-tiers.md)
- [Tier-A legacy fidelity runbook](Tier-A-legacy-fidelity-runbook.md)
- [2026-07-31 aggregate cutover record](../../results/2026-07-31-railway-simplified-cutover.md)
- [2026-07-28 experiment program report](../../results/2026-07-28-experiment-program-report.md)
