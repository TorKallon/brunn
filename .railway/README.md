# Straylight on Railway

`.railway/railway.ts` is the source of truth for the owner-alpha topology:

- one public `web` service;
- private `api`, `worker`, PostgreSQL, and Datadog Agent services;
- a persistent PostgreSQL volume in `us-west2`;
- external, private, versioned AWS S3 storage in `us-west-2`.

Railway's infrastructure-as-code SDK is beta and installed locally at an exact
version. Always invoke it through the pinned runner:

```bash
export RAILWAY_IAC_TS_BIN="$PWD/.railway/node_modules/.bin/railway-iac-ts"
railway config plan
railway config apply --yes
```

Install the pinned runner with:

```bash
npm ci --prefix .railway
```

Do not use `railway config pull` casually. It can rewrite the reviewed source
file from live state. `preserve()` entries deliberately keep secret values out
of Git; set those values with `railway variable set --stdin`.

## Deployment order

1. Commit a clean, tested candidate and record its full Git revision.
2. Set `DD_VERSION` and `STRAYLIGHT_BUILD_REVISION` to that revision on the
   API, worker, web, and Datadog services.
3. Qualify the external S3 bucket and its bucket-scoped credential.
4. Deploy `db`, then `datadog-agent`, then `worker`, `api`, and `web`.
5. Verify migrations, `/ready`, public-route isolation, logs, metrics, object
   storage, backup, and restore before importing owner data.
6. Add the custom domain only after the Railway-generated HTTPS URL passes.
7. Import the vault without deleting or modifying the local source, export into
   a fresh directory, and compare Markdown and binary hashes before cutover.

`straylight.rourkem.com` is the selected owner-alpha hostname. The API remains
reachable only through the web proxy, and `/api/v1/admin/*` is not exposed by
that proxy.
