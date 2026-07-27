# Straylight Alpha Launch Runbook

Status: retained single-host qualification procedure; not the approved managed
production path. Do not launch until `Alpha Readiness.md` records a go decision
and the selected provider has a replacement runbook.

This procedure still exercises the local MinIO-backed candidate topology. The
production architecture now requires managed PostgreSQL and a managed,
versioned cloud S3 store. MinIO remains useful for local development and
destructive qualification, but this runbook must not be used for production.

This runbook deploys the tested retrieval, write, capture, and dreaming
contracts unchanged. Production validation rejects changes to the embedding
provider/model, OpenAI endpoint, capture or dream model, capture output budget,
and materialization token budget.

## Release Candidate

Work only from a clean `main` checkout. Record the candidate commit, run every
deterministic and live gate, then create release artifacts:

```bash
git status --short --branch
make release-artifacts
```

The artifact directory contains exact binaries and bundles, saved container
images, image inspection records, CycloneDX SBOMs, vulnerability reports,
source/dependency fingerprints, and checksums. A `RELEASE-BLOCKED` marker means
the candidate must not be deployed.

For a different deployment host, transfer the complete checksummed artifact
directory through an approved private channel and load its image archives.
Never rebuild a candidate on the production host.

## Secrets And Configuration

Create local files containing the OpenAI and Datadog API keys without putting
either value in shell history. Generate the remaining secrets in one local
command:

```bash
make production-secrets \
  SECRETS_DIR=/approved/private/path/straylight-secrets \
  OPENAI_KEY_FILE=/approved/private/path/openai.key \
  DATADOG_KEY_FILE=/approved/private/path/datadog.key
```

Copy `production.env.example` to `production.env`, keep it mode `0600`, and
replace the release, image, hostname, email, notification, and secrets-path
placeholders. Do not put application or provider secrets in the environment
file.

Validate before every production Compose operation:

```bash
make production-config ENV_FILE=production.env
```

The validator requires a real candidate commit, immutable candidate images,
digest-pinned database/object-store/Agent images, file-backed secrets, an
approved HTTPS hostname, and the frozen reasoning/token configuration.

The Straylight PostgreSQL image contains pinned PostgreSQL 17 and pgvector
0.8.5 builds. A new cluster is initialized with built-in `C.UTF-8` collation
and page checksums. Its healthcheck fails closed if either invariant changes.
Never mount a libc-created data directory into the Alpine image; move existing
data through a verified coordinated backup and logical restore.

## Deploy

The production topology exposes only Caddy on ports 80 and 443. PostgreSQL,
object storage, the Rust API, and the SPA remain on the private Compose
network.

Use the gated deploy command. It validates configuration and image identity,
takes a coordinated backup when replacing a complete running stack, qualifies
the object store through the application credential, runs migrations, waits
for every healthcheck, verifies the public edge, and writes a checksummed
deployment record:

```bash
make production-deploy \
  ENV_FILE=production.env \
  BACKUP_ROOT=/approved/backup/path
```

Confirm every long-running service is healthy, the migration container exited
successfully, and the public readiness endpoint reports every required
dependency ready:

```bash
docker compose \
  --env-file production.env \
  --file compose.yaml \
  --file compose.production.yaml \
  --profile observability \
  ps
curl -fsS "https://<approved-hostname>/api/ready"
```

`/api/v1/admin/*` must return 404 through the public edge. Administrative
evaluation import and user provisioning are never public HTTP surfaces. The
deploy fails unless the selected object store proves bucket versioning,
conditional create, content deduplication, metadata round-trip, object version
IDs, delete markers, exact-version purge, and prefix purge.

## Provision Users

Production forbids development bootstrap credentials. Provision the first
owner through the one-shot operator command and capture its response in a
private file:

```bash
umask 077
mkdir -m 0700 -p operator-output
docker compose \
  --env-file production.env \
  --file compose.yaml \
  --file compose.production.yaml \
  run --rm -T migrate operator provision-user \
  --external-ref '<stable private user reference>' \
  --display-name '<display name>' \
  > operator-output/initial-owner.json
```

The token appears only in that file and is stored only as a hash by Straylight.
Move it to the approved password manager, remove the local output, and use the
owner credential only for credential administration. Create separate
read/write and read-only agent tokens in the SPA.

If an owner loses all owner credentials and there is no sign of compromise, use
the same private path:

```bash
umask 077
docker compose \
  --env-file production.env \
  --file compose.yaml \
  --file compose.production.yaml \
  run --rm -T migrate operator recover-user \
  --user-id 'user:<uuid>' \
  > operator-output/recovered-owner.json
```

This lost-token mode leaves any other owner credentials active. Store the
replacement and remove the output file.

If any owner credential may be compromised, create the replacement
and revoke every other active owner credential:

```bash
umask 077
docker compose \
  --env-file production.env \
  --file compose.yaml \
  --file compose.production.yaml \
  run --rm -T migrate operator recover-user \
  --user-id 'user:<uuid>' \
  --revoke-existing-owner-credentials \
  > operator-output/recovered-owner.json
```

The replacement is committed in the same transaction as the revocations, so a
failure cannot leave the account with half-applied recovery state. Both
recovery modes create content-free audit events.

## Observability

Datadog metrics and structured logs are approved for production. The
application sends DogStatsD metrics to a private Agent, while the selected
hosting platform forwards service logs through its supported Datadog log
stream. The Agent does not need Docker-socket, host-process, or host-filesystem
access for the managed deployment.

This retained single-host procedure leaves container logs local and therefore
does not satisfy the production logging gate.

After first traffic, verify runtime, HTTP, worker, model, embedding, queue,
deletion, database, and object-store series. Configure distribution
percentiles and monitors from an operator shell holding short-lived Datadog API
and application keys:

```bash
make datadog-configure
python3 infra/datadog/configure_percentiles.py --strict
```

Do not launch until monitor destinations are approved and test notifications
arrive.

## Backup And Restore

A coordinated backup stops API and worker writes, captures a serializable
PostgreSQL dump plus every object version, records inventories, immutable
runtime identity, database storage invariants, and expiry, then restarts and
verifies the bundle:

```bash
make production-backup \
  ENV_FILE=production.env \
  BACKUP_ROOT=/approved/backup/path
```

Copy the verified bundle to the approved encrypted off-host destination. Prune
only with the manifest-aware command:

```bash
make backup-prune BACKUP_ROOT=/approved/backup/path
```

Run an isolated restore after each candidate and on the agreed recurring
schedule:

```bash
make production-restore-drill \
  ENV_FILE=production.env \
  BACKUP_DIR=/approved/backup/path/<backup-id>
```

The drill checks checksums, database inventory, built-in collation, page
checksums, every object version, no-op migrations, API health, and operator
provisioning/recovery before destroying the isolated restore project. The
isolated database, object store, and API have lower memory and CPU ceilings
than production so a drill cannot claim the entire shared Docker host.

## Upgrade And Rollback

Before an upgrade, take and verify a coordinated backup. Deploy only a clean,
fingerprinted candidate whose full quality and token harness passed. Replace
all revision-tagged image references and `DD_VERSION` together.

For an application-only rollback, restore every image reference and
`STRAYLIGHT_RELEASE_REVISION` to the prior fingerprinted candidate and rerun
the production validator. Database migrations are forward-only; never run
ad-hoc down migrations. If a migration or canonical-data change must be
reversed, stop the deployment and restore the coordinated pre-upgrade bundle
into an isolated project first.

## Incident Procedure

1. Preserve the request ID, UTC interval, affected user/scope, candidate
   revision, and symptoms without copying source content into metrics or chat.
2. Inspect readiness, container status, structured logs, Datadog service
   metrics, queue age, deletion status, and dependency health.
3. Disable or revoke affected credentials. Use read-only credentials while the
   write path is uncertain.
4. Stop API and worker before any store-level intervention. Never edit
   canonical rows or object versions by hand during diagnosis.
5. Restore into an isolated project and compare inventories before deciding
   whether to roll back or recover.
6. Record the incident, evidence, action, validation, and follow-up. Reopen
   writes only after the relevant live and reasoning gates pass.
