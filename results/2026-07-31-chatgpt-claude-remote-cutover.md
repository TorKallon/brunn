# ChatGPT and Claude remote cutover results — 2026-07-31

This report contains credential identifiers and Keychain account names only. It
does not contain access tokens, credential values, passwords, or cookie values.

## Result

The production OAuth Streamable HTTP endpoint is:

```text
https://straylight.rourkem.com/mcp
```

The endpoint exposes exactly 10 remote tools:

- `memory.open`
- `memory.query`
- `memory.read`
- `memory.changes`
- `memory.capture`
- `memory.write`
- `memory.checkpoint`
- `memory.status`
- `asset.list`
- `asset.metadata`

`memory.stage` and `asset.fetch` remain local-only. The deployment and product
setup are documented in [`docs/Remote MCP.md`](../docs/Remote%20MCP.md).

| Area | Status | Qualification |
| --- | --- | --- |
| Production remote MCP | Passed | OAuth endpoint deployed; all 10 hosted tools surfaced; production canaries passed. |
| ChatGPT Work web/desktop | Complete | Developer mode, app installation, OAuth, tool permissions, and a selected-plugin exact-read test all passed. |
| ChatGPT mobile web | Available | The account-level ChatGPT Work connection works on the mobile web surface. |
| ChatGPT native mobile | Not directly supported | Native mobile plugins are unsupported. Use ChatGPT Remote through a connected desktop host. |
| Claude server path | Passed | The dedicated credential and server-level production canary passed. |
| Claude account connector | Requires account approval | Anthropic's prefilled installer is ready; Claude requires one interactive sign-in, connector review, and OAuth approval. |
| Claude web/desktop/mobile use | Pending account install | Remote connectors sync across those surfaces after account-side installation and OAuth are complete. |

## Dedicated credentials

| Client | Straylight credential ID | Keychain account |
| --- | --- | --- |
| ChatGPT Work | `credential:0c7810af-1bb6-4ed7-9b2c-91db9260ab5e` | `chatgpt-work-read-write` |
| Claude web/mobile | `credential:cbfc2475-00b8-4e54-a1cb-4523f8933deb` | `claude-web-mobile-read-write` |

## Production canaries

These are server-level tests of each dedicated credential through the
production OAuth Streamable HTTP endpoint.

| Client | Straylight path | Marker | Entry reference |
| --- | --- | --- | --- |
| ChatGPT Work | `operations/canaries/2026-07-31-chatgpt-work-remote.md` | `CHATGPT_WORK_REMOTE_STRAYLIGHT_CANARY_2026_07_31` | `entry:019fbb72-2a53-7700-829a-2be8a0e7d6c2` |
| Claude web/mobile | `operations/canaries/2026-07-31-claude-web-mobile-remote.md` | `CLAUDE_WEB_MOBILE_REMOTE_STRAYLIGHT_CANARY_2026_07_31` | `entry:019fbb72-2fe1-7c01-9dcf-920d1c26cffa` |

The ChatGPT canary was also read through the live ChatGPT Work selected-plugin
path. The exact-read result returned the expected marker and entry reference.
The Claude canary proves the production server and credential path; it is not a
Claude account-side connector test.

## ChatGPT Work qualification

- Developer mode was enabled.
- App ID: `asdk_app_6a6d6e098e988191954f8c11ca13438b`.
- OAuth connected successfully.
- Tool access was set to **Allow all actions**.
- The app surfaced the exact 10-tool hosted allowlist above.
- A Work-mode selected-plugin live exact-read returned
  `CHATGPT_WORK_REMOTE_STRAYLIGHT_CANARY_2026_07_31` and
  `entry:019fbb72-2a53-7700-829a-2be8a0e7d6c2`.

Native ChatGPT mobile plugins are not supported. Mobile web works. Native
mobile access requires ChatGPT Remote through a connected desktop host.

## Claude qualification and blocker

Claude remote connectors sync across web, desktop, iOS, and Android for the
same account after installation. The production server path and dedicated
Claude credential passed their canary, but account-side installation remains
blocked:

- Chrome was signed out of Claude.
- Computer Use reported that the Nyx Mac was locked.
- `tor@warmind.io` reached a new-account Terms screen, proving it is not an
  existing Claude account; no account was created and no terms were accepted.
- Google sign-in with `tor.kallon@gmail.com` did not yield an authenticated
  Claude session in the controlled browser.

The existing Claude desktop Cookies database contained `sessionKey` metadata
with an expiry of `2026-08-07`. No cookie values are included here, and that
metadata alone does not establish a usable current browser session.

Anthropic provides no supported headless API or CLI for creating an account
connector. To finish Claude client qualification, open the following prefilled
installer on any device already signed into the intended Claude account,
complete OAuth, and run an exact-read of the Claude canary from Claude itself:

```text
https://claude.ai/customize/connectors?modal=add-custom-connector&connectorName=Straylight&connectorUrl=https%3A%2F%2Fstraylight.rourkem.com%2Fmcp
```

Claude Code's local `mcp add` command is not a substitute: it configures Claude
Code only and does not provision Claude web or mobile.

## Verification summary

- MCP package: 39/39 tests passed.
- Web package: 19/19 tests passed.
- Python suite: 465 passed, 5 skipped.
- Docker/local smoke tests passed.
- Public discovery passed, and an unauthenticated request returned the expected
  `401` OAuth challenge.
- Railway `mcp` deployment `67c1cb80-ee67-4155-abea-6040399dc612` and `web`
  deployment `f45782a6-261e-4bd7-a1d1-74c1cbf89f9b` completed successfully.
- No embeddings or other OpenAI API-billed inference was used for this work.
