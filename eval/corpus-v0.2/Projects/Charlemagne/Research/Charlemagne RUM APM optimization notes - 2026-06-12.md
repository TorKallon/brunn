Created: 2026-06-12
Status: research note
Updated: 2026-06-12 16:30 PDT
Related: [[Projects/Charlemagne/Charlemagne|Charlemagne]], [[Projects/Charlemagne/Logbook|Logbook]], [[Projects/Warmind/Warmind|Warmind]], [[Projects/Charlemagne/Research/Charlemagne SPA page-family adoption telemetry plan - 2026-06-07|SPA page-family adoption telemetry plan]]

# Charlemagne RUM APM optimization notes - 2026-06-12

Scope: Datadog RUM/APM review for Warmind/Charlemagne optimization outside the profile/logbook profile workstream.

## Retrieval routing
- Use with [[Projects/Charlemagne/Logbook|Logbook]] performance work, Warmind SPA/APM triage, and [[Projects/Charlemagne/Charlemagne|Charlemagne]] cost/performance planning.
- Repo context starts in /Users/Shared/projects/charlemagne and /Users/Shared/projects/warmind; telemetry context starts from Datadog RUM Warmind.io and APM service warmind-api.

## Signal sources

- Repo telemetry contract: `docs/specs/spa-product-telemetry.md`.
- Datadog RUM app: `Warmind.io`, service `warmind.io`.
- APM service: `warmind-api`, env `prod`, host `rhea`.
- Canonical product traffic metric: `charlemagne.warmind.spa.product.pageview.human`.

## Traffic shape

Seven-day canonical human pageviews by `page_area`:

- `stats`: 39,586 - intentionally excluded here because another session is handling profile/stats.
- `analytics`: 32,242.
- `system`: 5,304.
- `account`: 4,057.
- `leaderboard`: 3,997.
- `pgcr`: 3,838.
- `guild`: 1,706.
- `lookup`: 1,225.

Top non-profile analytics families over the same window:

- `analytics.home`: 16,389.
- `analytics.legacy.item`: 5,145.
- `analytics.legacy.emblem`: 3,607.
- `analytics.activity`: 2,000.
- `analytics.srl`: 1,564.
- `analytics.legacy.title`: 1,475.

## Optimization candidates

1. `/in/playerActivity` cache miss path.
   - RUM 7d analytics resource: 13,210 calls, avg 3.69 s, p75 1.09 s, p95 12.03 s.
   - APM 7d exact `GET /in/playerActivity`: 23,255 spans, avg 1.99 s, p75 2 ms, p95 13.03 s.
   - Datadog logs in the last 24h show 68 foreground rebuilds, 204 shard logs, shard-scan p95 about 18 s, math p95 about 16 s.
   - Code shape: `apiPlayerActivity` reads `apiPlayerActivity` cache then falls back to `memcache.GetPlayerActivity`; `sweeperbot.cachePlayerActivity` writes the cache every 5 minutes with 5 minute TTL.
   - Best likely fix: stale-while-revalidate/longer stale TTL plus background refresh/lock, so web requests do not pay the Redis scan. Discord already treats missing cache as no-data rather than rebuilding in command path.

2. `/in/meta` weapon meta.
   - Live probe on 2026-06-12: `/in/meta` TTFB about 7.2 s.
   - APM 7d exact `GET /in/meta`: 4,641 spans, avg 1.16 s, p75 649 ms, p95 5.41 s.
   - Code has cache logic commented out in `apiMeta`; candidate for normalized query-key caching and/or SQL/index review.

3. Analytics home fan-out.
   - `analytics.home` RUM 7d: 9,770 initial views, p75 load 3.41 s, p75 LCP 3.03 s, average 17.5 resources.
   - Top API resources include `/in/playerActivity`, `/in/destiny2/telemetry/home`, `/in/destiny2`, and `/spa/telemetry/view`.
   - `GetAnalyticsHome` already uses `Promise.all`; the backend/cache behavior is the main issue.
   - `/in/destiny2/telemetry/home` is cheap in APM when cached, but RUM still sees network/payload time. Public JSON endpoints return `cf-cache-status: DYNAMIC` with no useful cache-control observed.

4. Legacy item/emblem analytics waterfall.
   - `analytics.legacy.item` RUM 7d: 1,865 views, p75 load 3.93 s, average 74.5 resources.
   - `analytics.legacy.emblem` RUM 7d: 2,110 views, p75 load 3.77 s, average 140.4 resources.
   - 24h waterfall showed very high normalized external image counts: about 83k for emblem views and 58k for item views.
   - Code already paginates to 24 cards and uses lazy images; remaining work may be list/detail image policy, smaller initial cards, stricter screenshot rendering, or image proxy/cache strategy.

5. Bungie OAuth callback.
   - APM 7d exact `GET /auth/bungie/callback`: 3,748 spans, avg 2.68 s, p75 3.41 s, p95 6.74 s.
   - Code performs token exchange, Bungie Net user lookup, linked profile lookup, possible per-profile `GetD2ProfileStats`, DB writes, role checks, DM send, default profile selection, and JWT redirect synchronously.
   - Candidate: keep only registration-critical writes synchronous; move role/nickname refresh, some profile refresh, and DM work to queued jobs after redirect.

6. CSP/RUM error noise.
   - RUM errors are dominated by CSP violations, especially report-only Trusted Types and blocked third-party/extension URLs.
   - Live headers still show `content-security-policy-report-only` with `images.ctfassests.net` typo; RUM shows blocked `images.ctfassets.net` content images.
   - Candidate: fix CSP typo/allowlist, then filter expected report-only CSP noise in Datadog RUM `beforeSend` so real errors stand out.

## Lower-priority outliers

- `GET /in/gboards/:leaderboardId/csv`: 24h APM count 28, avg about 20 s, p95 about 75.8 s. Very slow but low volume; consider async/pre-generated CSV if exports matter.
- PGCR detail has high page volume, but current backend resources are fast in 24h RUM/APM. RUM view load can still be high, likely SPA/document/resource timing rather than `GET /in/pgcr` backend.

## Profile-family raid subpage

Question: what is slowing down `/p/:identifier/raids`?

Datadog RUM 7d for `stats.raids` shows this is mostly SPA route-change traffic, and the slow resources are API calls rather than JS/CSS/document load:

- `https://api.warmind.io/in/logbook/profile/:identifier/raids`: 163 resources, avg 1.22 s, p75 386 ms, p95 4.00 s.
- `https://api.warmind.io/in/logbook/profile/:identifier/pantheon`: 163 resources, avg 782 ms, p75 248 ms, p95 2.85 s.
- `https://api.warmind.io/in/logbook/profile/:identifier`: 155 resources, avg 681 ms, p75 50 ms, p95 2.43 s.
- `https://external-referrer/`: 2,308 image resources, avg 55 ms, p75 20 ms, p95 264 ms.

APM 24h confirms the backend long pole:

- `GET /in/logbook/profile/:identifier/raids`: 195 spans, avg 1.81 s, p75 2.33 s, p95 3.77 s.
- `GET /in/logbook/profile/:identifier/pantheon`: 203 spans, avg 1.41 s, p75 2.15 s, p95 3.34 s.
- `GET /in/logbook/profile/:identifier`: 27,081 spans, avg 7.58 s, p75 12.77 s, p95 17.59 s.
- `GET /in/logbook/profile/:identifier/dungeons`: 137 spans, avg 1.71 s, p75 2.48 s, p95 3.48 s.

Source metric `charlemagne.warmind.logbook.source.*` points at Bungie profile/collectibles, not raid SQL:

- `source:bungieprofile,state:ok`: avg about 2.70 s, 95th about 3.77 s.
- `source:pantheonbungieprofile,state:ok`: avg about 1.30 s, 95th about 1.32 s in the 24h fanout rollup, with max minute around 4.40 s.
- `source:raiddifficultysummaries,state:ok`: avg about 2.7 ms, 95th about 2.7 ms.
- `source:raidcarries,state:ok`: avg about 6.1 ms, 95th about 10.1 ms.
- `source:raidseals,state:ok`: avg/95th about 0.6 ms.

Code shape:

- `LogbookRaidsPage` calls `useLogbookProfileShell(identifier)`, then starts `GetLogbookProfileRaids(identifier)` and, for raid pages, `GetLogbookProfilePantheon(identifier)` immediately.
- The page renders `RaidsLoading` until the raid response resolves, so the shell can improve skeleton/header context but cannot render the real raid scaffold/cards early.
- `loadLogbookRaidSources` waits for all concurrent sources before responding. The slow source is `bungie.GetD2ProfileStats(..., bungie.ProfileWCollectibles)`, used for raid exotic, armor, triumph, guardian-rank/avatar data.
- Pantheon repeats a separate `ProfileWCollectibles` fetch immediately on the same route, even before the user opens the Pantheon tab.

Optimization direction:

1. Split or defer collectible/progress enrichment from the raid cards so clears/full clears/carries/fastest render from the fast local sources first.
2. Lazy-load Pantheon only when the Pantheon tab is selected, or share/cache the Bungie `ProfileWCollectibles` result across raids and Pantheon on the server.
3. Keep the shell path for identity, but make the raid route render a ready scaffold from shell plus local raid stats instead of blocking on Bungie collectibles.
4. Treat images as secondary; they add many requests but their p95 is hundreds of ms, not the 3-4 s blocker.
