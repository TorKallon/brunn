# Hosted ChatGPT and Claude access

Status: production gateway live; ChatGPT Chat/Work account setup and mobile use
supported when the plugin is available to the account; Claude account install
pending its required interactive approval, 2026-08-27

Straylight exposes one authenticated Streamable HTTP MCP resource at:

```text
https://straylight.rourkem.com/mcp
```

The same endpoint is used for ChatGPT Chat, Work, and Claude custom connectors.
The existing local Codex, Aether/OpenClaw, and Claude Code integrations remain
stdio clients and are not routed through this gateway.

## Trust and token model

The public web service proxies an exact allowlist of MCP and OAuth routes to a
private one-replica Railway service. The gateway implements OAuth 2.1
authorization code flow, S256 PKCE, dynamic client registration, RFC 8707
resource binding, and RFC 9728 protected-resource metadata.

The MCP route permits browser access only from the exact HTTPS origins in
`STRAYLIGHT_MCP_ALLOWED_ORIGINS`. Requests without an `Origin` remain valid for
non-browser MCP clients. Browser preflights run before bearer authentication,
and authenticated or 401 responses expose only the MCP session and OAuth
challenge headers required by browser clients. Protected-resource metadata is
public and remains readable from any origin.

Every product gets a distinct Straylight `read_write` credential scoped to
`scope:root`. The approval page rejects read-only credentials and owner tokens
with `admin` or `credential:manage`. It verifies a pasted credential through
`/v1/me`, does not persist or log it, and returns only encrypted gateway access
and refresh tokens to the connector. Revoking the dedicated Straylight
credential revokes its actual data access without affecting another client.

The persistent `STRAYLIGHT_MCP_SEALING_KEY` is a 32-byte random Railway secret.
Rotating it invalidates all remote registrations and sessions. Preserve it
across ordinary deployments.

## Hosted tool surface

The gateway exposes:

- `memory.open`, `memory.query`, `memory.read`, and `memory.changes`
- `memory.capture`, `memory.write`, and `memory.checkpoint`
- `memory.status`
- `asset.list` and `asset.metadata`
- `briefing.publish`, `briefing.dedupe`, and `briefing.topics`
- `document.publish` and `document.get`
- `notification.publish`

It does not expose `memory.stage` or `asset.fetch`. Those operations read or
write the MCP adapter host's filesystem; in Railway that filesystem is not the
user's phone or computer. Text reads are capped at 120,000 characters per item
for hosted-client result limits. The local stdio adapter also exposes the two
filesystem-dependent tools.

## Railway deployment

The checked-in topology creates private service `mcp` from
`apps/mcp/Dockerfile.remote`, then passes its private hostname to the public
web proxy. Before applying topology changes, always run a plan and require the
diff to contain only the intended changes—no unrelated updates or deletions:

```bash
export RAILWAY_IAC_TS_BIN="$PWD/.railway/node_modules/.bin/railway-iac-ts"
railway config plan --verbose
railway config apply --yes --verbose
```

Do not apply a mixed-drift plan. Use a service-scoped variable update and
MCP-only deployment for a narrow gateway release, or reconcile the unrelated
topology drift as a separate change.

The service requires:

```text
PORT=8080
STRAYLIGHT_API_URL=http://api.railway.internal:8080
STRAYLIGHT_MCP_PUBLIC_URL=https://straylight.rourkem.com
STRAYLIGHT_MCP_ALLOWED_ORIGINS=https://chatgpt.com,https://claude.ai,https://straylight.rourkem.com
STRAYLIGHT_MCP_SEALING_KEY=<base64 for exactly 32 random bytes>
```

Deploy `mcp` before `web`, then verify health, discovery, an unauthenticated
OAuth challenge, a full authorization-code exchange, the complete hosted tool
surface, and a read/write canary using a dedicated client credential.

The checked-in canary consumes a credential only from its environment and does
not print or persist it:

```bash
cd apps/mcp
STRAYLIGHT_REMOTE_TOKEN="$(security find-generic-password -s straylight.rourkem.com -a CLIENT_ACCOUNT -w)" \
STRAYLIGHT_REMOTE_LABEL="client label" \
STRAYLIGHT_REMOTE_CANARY_PATH="operations/canaries/client.md" \
STRAYLIGHT_REMOTE_MARKER="UNIQUE_MARKER" \
node scripts/remote-canary.mjs
```

## Product setup

On ChatGPT web or desktop, enable Developer mode under **Settings → Security
and login**, then use the plus control on the Plugins page to register the full
MCP URL above. Complete OAuth with the dedicated ChatGPT credential and review
the discovered tools and metadata before enabling it. Developer mode can be
account- or policy-dependent.

This creates the cloud/account connection used by new Chat and Work
conversations. OpenAI documents account-available plugins as usable on mobile,
although web/desktop are the documented browse and install surfaces. On the
same account, open a new mobile Chat or Work conversation and select Straylight
from Plugins or `@` autocomplete. A local Codex stdio configuration or a local
plugin-creator personal-marketplace entry does not create this account-level
connection and is not a mobile provisioning path.

For Claude, add the URL under custom connectors on web or desktop and complete
OAuth with the dedicated Claude credential. Claude remote connectors sync to
the same account's web, desktop, iOS, and Android surfaces. Set the connector's
tool access to always available when the client offers that setting.

Anthropic's supported prefilled installer is:

```text
https://claude.ai/customize/connectors?modal=add-custom-connector&connectorName=Straylight&connectorUrl=https%3A%2F%2Fstraylight.rourkem.com%2Fmcp
```

It may be opened on any device with the intended Claude account. Anthropic
requires an interactive account sign-in, connector review, and OAuth approval;
Claude Code's local `mcp add` command does not install an account connector and
does not provision Claude mobile.

The production qualification record, including credential identifiers,
canaries, product-side results, and the remaining Claude client-state blocker,
is in
[`results/2026-07-31-chatgpt-claude-remote-cutover.md`](../results/2026-07-31-chatgpt-claude-remote-cutover.md).

Current product references:

- [OpenAI plugin availability](https://learn.chatgpt.com/docs/plugins)
- [OpenAI ChatGPT connection setup](https://developers.openai.com/plugins/deploy/connect-chatgpt)
- [OpenAI MCP connections](https://learn.chatgpt.com/docs/extend/mcp)
- [OpenAI MCP authentication](https://developers.openai.com/plugins/build/auth)
- [Claude custom connectors](https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp)
- [Claude connector authentication](https://claude.com/docs/connectors/building/authentication)
- [Claude custom-connector install links](https://claude.com/docs/connectors/building/directory-vs-custom)
