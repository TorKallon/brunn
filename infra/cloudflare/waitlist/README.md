# brunn.ai/waitlist — early-access email capture

A Cloudflare Worker on the route `brunn.ai/waitlist` (zone `brunn.ai`)
receives the front page's one-field form and stores addresses in the
`WAITLIST` KV namespace. The page has no JavaScript: the Worker answers every
request with a 303 back to a fragment the page reveals with CSS.

| Request | Result |
| --- | --- |
| `GET /waitlist` | `303 /#soon` (the form) |
| `POST` with a valid `email` | stored once, `303 /#joined` |
| `POST` with an invalid address | `303 /#retry` |
| `POST` with the honeypot field filled | nothing stored, `303 /#joined` |
| more than five posts per IP per hour | nothing stored, `303 /#joined` |

Each record is `email:<address>` → `{email, joined_at, country, referer}`.

## Operate

```bash
npm ci
export CLOUDFLARE_ACCOUNT_ID=84491e6f4c07cb5dbb50d0b5bbafa513
# Authenticate with `npx wrangler login`, or the vault's
# cloudflare-global-api-credential as CLOUDFLARE_EMAIL + CLOUDFLARE_API_KEY.
npm run list                       # every address
npx wrangler kv key get --binding WAITLIST --remote 'email:someone@example.com'
npx wrangler kv key delete --binding WAITLIST --remote 'email:someone@example.com'
npm run deploy                     # after editing worker.js
```

Deployed 2026-09-12 (Worker `brunn-waitlist`, KV `5b26d035ddc04c1a9c736659eb3853ea`).
