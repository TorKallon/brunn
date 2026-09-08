# Dreamer production canary

`scripts/dreamer-production-canary.py` defaults to seven read-only HTTP GETs.
It uses the verified Keychain item `straylight.rourkem.com` /
`owner-api-token` in process memory. It refuses redirects and environment
proxies. Credentials never enter arguments, environment variables, reports,
stdout, or files.

```bash
python3 scripts/dreamer-production-canary.py \
  --report /tmp/brunn-dreamer-preflight-UNIQUE.json
```

The private report records the exact API revision, readiness, runtime flags,
current text count/bytes, physical binary version count/bytes, credential count,
unread notification count, and Review counts/status when that route exists.
Counts are observations of the authenticated owner's scope, not whole-service
inventory. It retains no personal content, names, credential labels, or owner ID.

## Executing the synthetic fixture after deployment

The release owner must explicitly run `--execute` with the deployed full API
build revision. The script refuses old revisions and a missing Review route.
Summary preference must be enabled with
`BRUNN_DREAMER_SUMMARY_READS_ENABLED=true`; the canary checks actual
`derived_summary` responses, so a disabled gate fails visibly and still queues
fixture cleanup.

For a phased release, start with the flag **false** and add
`--summary-preference disabled_then_enabled --flag-wait-seconds 600`.
The first fixture approval must produce a validated direct summary while a
source `current_state` read still returns canonical text. The script then writes
and prints an `awaiting_summary_preference_enable` milestone. The release owner
can enable the flag and restart the API through the normal deployment tooling;
the canary only polls GET `/v1/status` during that interval, including transient
restart failures. Once the flag is true, it verifies source substitution,
source-edit fallback, and a fresh replacement through a second owner approval,
then requests fixture deletion. Timeout fails visibly and queues cleanup.
The script itself never changes the production flag.

Public Nginx deliberately returns 404 for `/api/v1/admin/*`. Provisioning thus
requires either an **existing trusted loopback connection** to the private API
or a freshly provisioned fixture response through an inherited file descriptor:

```bash
# PRIVATE_API_BASE is a separately established, trusted loopback transport.
# The canary never opens listeners, tunnels, or production administration routes.
python3 scripts/dreamer-production-canary.py --execute \
  --expected-revision "$DEPLOYED_REVISION" \
  --admin-base "$PRIVATE_API_BASE" \
  --report /tmp/brunn-dreamer-canary-UNIQUE.json
```

The private endpoint is `POST /v1/admin/users`. The script first checks that
private `/v1/me` resolves to the same verified administrator. It provisions a
unique `brunn-dreamer-canary:<UUID>` external reference and a fixture owner
credential. Public HTTP fixture operations then use that **fixture's own** token.
The real owner's token never approves, edits, or deletes workspace records.

Alternatively, a trusted operator process may call the existing private
provisioning route, retain the response only in memory, and pass its JSON
`{user:{id,external_ref,created_at},credential:{token}}` through an inherited
descriptor above 2 via `--fixture-fd`. Do not save that response to disk or print
it. The external reference must use the exact canary prefix and a canonical
UUID. The fixture must be less than an hour old, differ from the real owner,
and start with zero text records and an empty inbox. No public proxy exception
or evaluation feature flag should be enabled for this canary.

The fixture workflow uses no model or raw location evidence:

1. Issue restricted `dreamer_runner` and `read_only` fixture credentials;
   assert forbidden reads, writes, Review decisions, and publisher inbox access.
2. Write synthetic source text and full-mode fixture CONTROL; admit one bounded
   manual run and submit a summary with an exact source version/line citation.
3. Replay the candidate and terminal requests; check stable identities and the
   strict canonical 16-field latest receipt, pinned to its actual terminal run.
4. Publish and replay a restricted notification referencing that accepted exact
   run. Assert `no_installations`, zero deliveries, no returned private content,
   and the correct pinned source in the fixture owner's inbox.
5. Approve only the newly submitted fixture candidate through its owner token;
   check idempotent application, preserved operational receipt identity, a fresh
   reader `current_state` summary, and an unchanged exact-version source read.
6. Edit the source. Both source-preferred and direct-summary current reads must
   return current source text; the exact old source remains historical. Admit,
   submit, finish, and approve a new candidate to verify recovery.
7. Request account deletion for the fixture, including on ordinary failures.

This tests the actual HTTP/backend/credential/receipt/notification/Review/read
path. It does not test Codex authentication, model quality, scheduled execution,
real-device delivery, raw location compilation, or production latency p95.
Source/summary character counts are synthetic measurements only.

## Cleanup and recovery limits

Cleanup rechecks `/v1/me` against the recorded fresh fixture identity and sends
`POST /v1/account/deletion` with `DELETE <exact fixture external_ref>`. The
existing lifecycle revokes other fixture credentials and reduces the cleanup
credential to status only. The script polls the deletion receipt for up to
180 seconds (configurable up to 600).

`awaiting_backup_expiry` with all records completed verifies canonical purge;
it **does not** establish backup erasure. Only `completed` does that. The script
reports those separately and does not shorten retention, alter backup
watermarks, enable evaluation cleanup, issue SQL, or delete production objects
itself. Failed or timed-out cleanup remains a visible failing canary result.

An uncertain provision result or process termination can leave a fixture. The
private report records the generated unique external reference before the
request and its user ID immediately after a successful response. Never guess a
different account. Use the existing private admin recovery route for that
verified fixture if credential recovery is required; inspect its identity and
queue its normal account deletion. The helper `cleanup(client, fixture,
real_owner_id, timeout)` also resumes a status-only deletion credential by GETs.
Recovery credentials should remain in operator process memory. This script
does not silently repeat an uncertain provisioning request or recover a token
for an unverified user ID.

Preparation gates:

```bash
python3 -m unittest discover -s tests -p test_dreamer_production_canary.py -v
python3 -m py_compile scripts/dreamer-production-canary.py
```
