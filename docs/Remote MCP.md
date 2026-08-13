# Hosted ChatGPT and Claude access

Status: production gateway live; ChatGPT Work qualified; Claude account install
pending its required interactive approval, 2026-07-31

Straylight exposes one authenticated Streamable HTTP MCP resource at:

```text
https://straylight.rourkem.com/mcp
```

The same endpoint is used for ChatGPT Work and Claude custom connectors. The
existing local Codex, Aether/OpenClaw, and Claude Code integrations remain
stdio clients and are not routed through this gateway.

## Trust and token model

The public web service proxies an exact allowlist of MCP and OAuth routes to a
private one-replica Railway service. The gateway implements OAuth 2.1
authorization code flow, S256 PKCE, dynamic client registration, RFC 8707
resource binding, and RFC 9728 protected-resource metadata.

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
web proxy. Before applying topology changes, always run a plan and require zero
unrelated deletions:

```bash
export RAILWAY_IAC_TS_BIN="$PWD/.railway/node_modules/.bin/railway-iac-ts"
railway config plan --verbose
railway config apply --yes --verbose
```

The service requires:

```text
PORT=8080
STRAYLIGHT_API_URL=http://api.railway.internal:8080
STRAYLIGHT_MCP_PUBLIC_URL=https://straylight.rourkem.com
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

For ChatGPT Work, enable Developer mode and add the MCP URL as a personal
connection on the web/desktop plugin page. Complete OAuth with the dedicated
ChatGPT credential. OpenAI currently documents plugins as web/desktop only,
not native mobile. The account-level connection is usable anywhere the Work
web surface is available; native-mobile access requires ChatGPT Remote through
a connected desktop host until native plugin support exists.

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
- [OpenAI remote connections](https://learn.chatgpt.com/docs/remote-connections)
- [OpenAI MCP authentication](https://developers.openai.com/plugins/build/auth)
- [Claude custom connectors](https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp)
- [Claude connector authentication](https://claude.com/docs/connectors/building/authentication)
- [Claude custom-connector install links](https://claude.com/docs/connectors/building/directory-vs-custom)
