Created: 2026-05-03 16:40 PDT
Updated: 2026-06-18 PDT
Status: active reference

## Purpose
Warmind is the legacy multi-service Go backend behind Charlemagne-era Destiny 2 community services, including the Discord bot, REST API, parsers, background workers, donation handling, and game-data systems.

## Routing
- Repo on Nyx: `/Users/Shared/projects/warmind`
- Use this note as the vault landing page for Warmind reference material.
- Treat [[Projects/Charlemagne/Charlemagne|Charlemagne]] as the adjacent active project context unless Warmind becomes active on its own.

## Read first
- [[Projects/Warmind/Bungie API throttle incident - 2026-06-09|Bungie API throttle incident - 2026-06-09]] and [[Projects/Warmind/Mode 0 Override Europa investigation - 2026-06-09|Mode 0 Override Europa investigation - 2026-06-09]] - current production API throttle and mode-zero routing investigations.
- [[Projects/Warmind/Mode 0 Override Moon investigation - 2026-06-24|Mode 0 Override Moon investigation - 2026-06-24]] - production evidence and local workaround for `3480889442` / `2301670494` routing to `ModeOffensive`.
- [[Projects/Warmind/Mode 0 Override Last City investigation - 2026-06-16|Mode 0 Override Last City investigation - 2026-06-16]] - production evidence and local workaround for `3343628502` / `883763122` routing to `ModeOffensive`.
- [[Projects/Warmind/Mode 0 Last City social investigation - 2026-06-12|Mode 0 Last City social investigation - 2026-06-12]] - production evidence and local workaround for `directorActivityHash=3738486654` routing to `ModeSocial`.
- [[Projects/Warmind/D1 parser|D1 parser]] — active D1 PGCR parser/autoscaling hub.
- [[Projects/Warmind/Session tracking Nyx soak progress - 2026-06-08|Session tracking Nyx soak progress]] — current Nyx dev soak for Logbook session-tracking hardening and parser/Cortex pressure.
- [[Projects/Charlemagne/Research/Charlemagne session tracking storage retention plan - 2026-06-16|Session tracking storage retention plan]] — one-week plan to keep Logbook session tracking enabled while reducing rhea MySQL/EBS growth.
- [[Projects/Warmind/OpenSkill individual ratings plan - 2026-06-09|OpenSkill individual ratings plan]] and [[Projects/Warmind/OpenSkill Go library implementation prompt - 2026-06-09|OpenSkill Go library implementation prompt]] — individual competitive skill ratings plan using Python OpenSkill as oracle and a Warmind-owned Go Weng-Lin Plackett-Luce implementation.
- [[Projects/Warmind/SRL raw_new_modes capture - 2026-06-12|SRL raw_new_modes capture]] — completed and verified week 458 SRL PGCR capture staged for later reviewed raw_new_modes loading.
- [[Projects/Warmind/SRL raw_new_modes one-off gaps - 2026-06-12|SRL raw_new_modes one-off gaps]] — review finding that the SRL one-off does not refresh WLR, session tracking, or global profile leaderboards by itself.
- [[Projects/Warmind/D2 first played backfill retired - 2026-06-12|D2 first played backfill retired]] — `update_d2_first_played` queue-pressure retirement, verification, and Rhea Redis cleanup command.
- [[Projects/Warmind/D2 first played production integrity plan - 2026-06-08|D2 first played production integrity plan]] — older incident-window reset and controlled requeue plan for `profile_play_bounds.d2.activity_history`.
- [[Projects/Warmind/Final parser Omega-3 playbook - 2026-06-07|Final parser Omega-3 playbook]], [[Projects/Warmind/Final parser Omega-3 handoff - 2026-06-07|Omega-3 handoff]], and [[Projects/Warmind/Final parser Omega-3 next-thread prompt - 2026-06-07|Omega-3 next-thread prompt]] — current non-lazy parser validation reset, full scorecard, guardrails, and next-thread starting point.
- [[Projects/Warmind/Final parser scorecard validation progress - 2026-06-05|Final parser scorecard validation progress]] — active current-target parser validation progress for branch `codex/parser-d1-d2-common-main`, starting at commit `893836c7c26deb42563e2270857a9bc471ec5b4b`.
- [[Projects/Warmind/PGCR S3 production cutover runbook - 2026-06-05|PGCR S3 production cutover runbook]] — production schema/S3 cutover steps for the converged parser and new canonical PGCR archive generation.
- [[Projects/Warmind/PGCR cache cleanup scratch - 2026-06-06|PGCR cache cleanup scratch]] and [[Projects/Warmind/Parser data corruption - 2026|Parser data corruption]] — compact scratch/evidence notes for PGCR cache cleanup and parser corruption reprocessing bounds.
- [[Projects/Warmind/Final parser perf and full validation handoff - 2026-06-04|Final parser perf and full validation handoff]] — latest Stage 2 and Stage 3 handoff plan and prior target caveats.
- [[Projects/Warmind/Final parser validation use guide - 2026-06-03|Final parser validation use guide]] — validation workflow entrypoint; use repo skill, repo docs, previous playbooks, and Codex memory/session search before resuming.
- [[Projects/Warmind/Final parser EC2 revalidation progress - 2026-06-03|Final parser EC2 revalidation progress]] — frozen prior progress and diagnostic history.
- [[Projects/Warmind/Destiny 2 concurrent player calibration tracker - 2026-06-02|D2 concurrent player calibration tracker]] — current D2 observed/concurrent-player calibration work and launch-day estimate evidence.
- [[Projects/Warmind/Destiny 2 external PGCR torrent dataset - d2.asun.co|Destiny 2 external PGCR torrent dataset - d2.asun.co]] — external D2 PGCR torrent archive; quality unknown and not canonical Warmind truth without validation.
- [[Projects/Warmind/Nyx code intelligence pilot - 2026-06-04|Nyx code intelligence pilot]] — local `gopls`, SQLite, and OpenAI-embedding search layer for duplicate-helper prevention in Warmind.
- [[Projects/Warmind/Logbook public profile fallback plan - 2026-06-05|Logbook public profile fallback plan]] — public/non-registered profile teaser plan, concurrent API aggregation, PGCR sharing, and handoff prompt.
- [[Projects/Warmind/Exotic mission hash rollup research 2026-06-17|Exotic mission hash rollup research]] — manifest-backed completion record groups for Logbook campaign/exotic mission profile rollups.
- [[Projects/Warmind/Pantheon materializer backlog investigation - 2026-06-18|Pantheon materializer backlog investigation]] — production Datadog/code-shape note for `pantheon_pgcr_materialize` queue stacking versus row backlog.
- [[Projects/Warmind/D2 error reparse lock-wait storm - 2026-06-18|D2 error reparse lock-wait storm]] — Datadog-backed decision note for waiting out scaled D2 `error_reparse`, returning to `opportunistic`, and then reassessing residual `UpdateWeaponMetaTX weekly` lock waits.
- [[Projects/Warmind/Bungie auth nginx route follow-up - 2026-06-23|Bungie auth nginx route follow-up]] — current evidence and exact production nginx route needed for authenticated extra Bungie account linking.
- [[Projects/Warmind/Repo docs/AGENTS|Warmind agent instructions]] — shared repository guidance for Codex and other agents.
- [[Projects/Warmind/Repo docs/discord/ddata/README|Warmind Discord data README]]
- [[Projects/Warmind/Repo docs/donator/README|Warmind donator README]]

- [[Projects/Warmind/Warmind SPA build and upload runbook - 2026-06-06|Warmind SPA build and upload runbook]] - Nyx-local production SPA artifact build/upload/install note; avoid the legacy yarn build deploy path.

## Useful context
Warmind integrates Bungie Destiny 2 APIs, Discord, AWS, Stripe, MySQL, MongoDB, Redis-backed workers, and multiple Go services. For project status and operational decisions, start with [[Projects/Charlemagne/Charlemagne|Charlemagne]] unless the task is specifically about Warmind code or docs.

## Containerized dev pass
Updated 2026-05-17: the repo now has a development-only container setup in `/Users/Shared/projects/warmind/compose.dev.yaml`. It uses local-only Nyx ports from [[Projects/Operations/Project Port Map|Project Port Map]], dev config at `dev/config/warmind_config.toml`, and keeps production-impacting integrations disabled by default.

Verified on 2026-05-17: `docker compose -f compose.dev.yaml up -d --build`, `docker compose -f compose.dev.yaml --profile tools run --rm smoke`, host API/SPA curls, and Chrome/Computer Use checks all worked. The setup uses cached serialized app builds to avoid first-run compile OOMs.

Discord interactions tunnel: `torbot-cmd.ngrok.io -> 127.0.0.1:18091`; started successfully in detached tmux session `warmind-ngrok`. The Discord Developer Portal URL still requires a user-side account edit before live interaction webhook verification:

```text
https://torbot-cmd.ngrok.io/discord-whooks/
```

Erebus dev config import on 2026-05-17: `rourkem@erebus:/Users/rourkem/etc/warmind/warmind_config.toml` and legacy comparison `rourkem@erebus:/Users/rourkem/projects/warmind/nexus_config.toml` were copied to Nyx under ignored path `/Users/Shared/projects/warmind/dev/secrets/erebus-import/`. Keep those files quarantined because the main TOML mixes dev markers with external service credentials and production-looking markers. Only non-secret Discord dev identifiers were imported into ignored `/Users/Shared/projects/warmind/dev/.env`: public key, client ID, and dev guild ID.

After the import, `warmind-dcmds` had `WARMIND_DISCORD_PUBLIC_KEY`, `WARMIND_DISCORD_CLIENT_ID`, and `WARMIND_DISCORD_DEV_GUILDS` set. Unsigned POST through `https://torbot-cmd.ngrok.io/discord-whooks/` returned `401` instead of the previous `503`, confirming the route reaches Discord signature verification. The user saved the Interactions Endpoint URL in the Discord Developer Portal; `warmind-dcmds` logged `Received a PING with good signature, so responding with PONG!` and returned `200`.

Final dev verification on 2026-05-17 after the portal save: `go build ./...`, `docker compose -f compose.dev.yaml config`, `docker compose -f compose.dev.yaml --profile tools run --rm smoke`, host curls, and Computer Use checks in Chrome all passed. The stack was healthy with API, SPA, `warmind-dcmds`, and dwork listening on local-only mapped Nyx ports, and ngrok forwarding `https://torbot-cmd.ngrok.io` to `127.0.0.1:18091`.

Follow-up on 2026-05-17: the real Warmind SPA is the sibling checkout `/Users/Shared/projects/spa`, and Warmind now has `dev/scripts/build-spa.sh` to build it safely into ignored `dev/spa-dist` without running the SPA repo's deploy-era `yarn build`. Verified tailnet/local access:

- SPA: `http://100.126.232.13:13090`, `http://localhost:13090`
- API: `http://100.126.232.13:18090`, `http://localhost:18090`

The short `nyx` hostname depends on MagicDNS/search resolution on the calling client. The container services bind correctly for it, but the Tailscale IP is the verified tailnet URL from Nyx.

The dev SPA serves real Charlemagne assets, deep SPA routes return `index.html`, and SPA login sends Discord OAuth back to the API callback on `:18090`. `/daily` and `/pve` are covered by a dev-only container smoke test:

```sh
docker compose -f compose.dev.yaml exec -T warmind-dcmds \
  go test -tags warminddev ./discord/dcmds \
  -run 'TestWarmindDev(Daily|Pve)Command' -count=1 -v
```

The `/pve` smoke needed a dev-only schema compatibility init for newer Nexus columns/tables: `seals.seals2Bitmask`, `seals.equipped2Bitmask`, `seals_gilded.gildedSeals2Bitmask`, `summary_player_weapons_y7`, and `summary_player_weapons_y8`.

SPA repo guide update on 2026-05-18: `/Users/Shared/projects/spa/AGENTS.md` now documents the standalone Parcel/React/MobX SPA structure, the settings-driven Warmind API pattern, env var names without values, verification commands, and cautions that the SPA repo's `yarn build` performs deploy-era packaging/scp/start behavior. It also calls out that the standalone `yarn start` default is port `5000`, while the shared Nyx Warmind containerized SPA path uses the claimed `13090` port range from [[Projects/Operations/Project Port Map|Project Port Map]].

GA4 migration analysis update on 2026-05-19: [[Projects/Charlemagne/Charlemagne GA4 route migration analysis - 2026-05-19|Charlemagne GA4 route migration analysis - 2026-05-19]] captures the manual GA4 Pages and screens plus Landing page exports for `warmind.io` from `2024-04-20` through `2026-05-17`. The headline is that active legacy Python routes still represent about `562,728` landing sessions and `1,447,583` page views in the export, so SPA migration should preserve or rebuild guild leaderboard detail URLs, item/title analytics, global leaderboards, raid/crucible stats, and command/article content before retiring the legacy web surface.

Dev database bootstrap update on 2026-05-17: Nyx now has ignored prod discovery and bounded prod sample archives under `/Users/Shared/projects/warmind/dev/secrets/`. Run `/Users/Shared/projects/warmind/dev/scripts/bootstrap-data-stores.sh` to reset only the local dev MySQL/Mongo containers, import the exported prod MySQL table/trigger schema for `charlemagne`, `discord`, `nexus`, and `nexustoo`, and load the bounded sample MySQL/Mongo data set. Verified table inventory parity against the prod discovery export, all Compose smoke checks, and dev-only `/register`, `/daily`, and `/pve` command tests. The prod export still lacks the diagnostic `explain_statement` routine bodies because the prod DB user did not have permission to export them.

Seed handoff update on 2026-05-17: Warmind now has `/Users/Shared/projects/warmind/dev/scripts/export-dev-seed.sh` and `/Users/Shared/projects/warmind/dev/scripts/restore-dev-seed.sh`. The current verified local seed is `/Users/Shared/projects/warmind/dev/secrets/seed/warmind-dev-seed-20260517-221742.tar.gz` with matching SHA file beside it; it is intentionally ignored because it contains bounded sample user data. The new-machine guide is `/Users/Shared/projects/warmind/docs/agents/container-dev-from-scratch.md`. Recommended long-term seed hosting is a private Cloudflare R2 bucket with checksum/manifest pointers in repo docs, not direct git or default Git LFS.

Legacy web discovery update on 2026-05-17: the production web split and old Python/Flask Charlemagne runtime details are captured in [[Projects/Warmind/Warmind production web split and legacy Python runtime - 2026-05-17|Warmind production web split and legacy Python runtime - 2026-05-17]]. Use that note when adding the legacy Python website to the development containers and later when writing the production containerization manual.

Production containerization discovery update on 2026-05-17: the verified rhea discovery tarball and extracted local cache are indexed in [[Projects/Warmind/Warmind production containerization discovery bundle - 2026-05-17|Warmind production containerization discovery bundle - 2026-05-17]]. It includes redacted Nginx/systemd/runtime config, full schema-only MySQL export, bounded sample SQL, Redis/Mongo findings, service-memory/restart details, and migration implications for the future new-machine production containerization plan.

Legacy web container rehearsal update on 2026-05-17: the production-shaped dev integration of the old Charlemagne Flask/Gunicorn web stack is captured in [[Projects/Warmind/Warmind legacy web container integration rehearsal - 2026-05-17|Warmind legacy web container integration rehearsal - 2026-05-17]]. It records the Warmind-owned Compose orchestration, nginx split routing, Python 3.7/Gunicorn container, Warmind API dependency, item analytics sample-data seeding, route verification, and the Docker DNS resolver issue that matters for production containerization.

Build-pattern reminder: [[Projects/Warmind/Warmind Docker multi-stage build note|Warmind Docker multi-stage build note]] keeps the original multi-stage Dockerfile prompt linked to this containerization thread.

Rhea storage cleanup update on 2026-05-27: [[Projects/Warmind/Warmind rhea storage cleanup todo - 2026-05-27|Warmind rhea storage cleanup todo - 2026-05-27]] tracks the current production space-reduction runbook, including the manifest archive backup, the `summary_weapon_meta_daily` reclaim window, and the post-deploy plan to drop legacy `raid_reparse` / `dungeon_reparse` carry-marker tables after Warmind commit `3f146c3b`.

D2 raw failed PGCR audit on 2026-05-28: [[Projects/Warmind/D2 raw_failed_pgcr stale failure audit - 2026-05-28|D2 raw_failed_pgcr stale failure audit - 2026-05-28]] records that a 10,000-row production `raw_failed_pgcrs` export from instance IDs `14039571480-14131722513` now returns `Success` for every live Bungie probe. Treat the stored failure reasons in that export as stale; use `raw_pgcrs` coverage gaps plus fresh Bungie probes to find real persistent missing ranges.

Dependency risk update on 2026-05-18: [[Projects/Warmind/Warmind dependency vulnerability exposure analysis - 2026-05-18|Warmind dependency vulnerability exposure analysis - 2026-05-18]] triages 19 Dependabot alerts by reachable code path, with the embedded JWT signing secret and direct gRPC/HTTP2 exposure as the practical risks to handle first.

Missing route fixture import on 2026-05-20: imported `/Users/Shared/projects/warmind/dev/secrets/missing-route-fixtures/warmind-missing-route-fixtures-20260520-150730` into the local Nyx dev stack. The tarball SHA256 matched `a3de4ed3f60eaefcbf12c0171ada2c961d12198c1329cf9bb7bd0fe2d570b241`, and `sha256.txt` validated. MySQL import needed two in-stream local-only adjustments while leaving the verified artifact unchanged: normalize the stray header `Z--` comment and use session-scoped `FOREIGN_KEY_CHECKS=0` plus `INSERT IGNORE` because the fixture orders `charlemagne.users` before `user_bungie_profiles` and the already-bootstrapped DB has reference rows. Verified local counts included Cool D2 People guild row, 66 Charlemagne guild members, 60 linked Charlemagne users, 149 linked user Bungie profiles, 66 Discord guild members/users, 76 scoped `nexus.profiles`, 28 lost sectors, 35 fractaline rows, and 98 Seraph tower rows.

Elasticsearch search follow-up on 2026-05-20: reran `dev/scripts/seed-elasticsearch.sh`, which rebuilt `players1` from local `nexus.profiles` to 2,994 docs. The main Tor_Kallon row has `valid=0`, so the seeder still excluded it; index `elasticsearch/tor_kallon_players1_docs.json` directly after normalizing `lastPlayed` from MySQL datetime to ISO `...Z`. Final `players1` count was 2,995, and `http://127.0.0.1:18090/in/profileSearch?q=Tor_Kallon` returned membership `4611686018428592074`.

Dev parser live-run update on 2026-05-20: in the Nyx dev Compose stack, `nexus.raw_pgcrs` was truncated and seeded with a single dummy row at `instanceId=16846444934`, which made `warmind-parser` start at `16846444935`. Enabling live parser/cortex required ignored `dev/.env` overrides for `WARMIND_FEATURES_SKIP_BUNGIE=false`, `WARMIND_WARMIND_PARSER_SKIP=false`, `WARMIND_FEATURES_CORTEX_BUNGIE_API=true`, `WARMIND_FEATURES_GLOBAL_PROFILES=true`, `WARMIND_FEATURES_GLOBAL_LEADERBOARDS=true`, `WARMIND_FEATURES_RT_UPDATES=true`, `WARMIND_FEATURES_D2_POP_ANALYTICS=true`, `WARMIND_FEATURES_D2_PACTIVITY_TRACKING=true`, and cortex worker/concurrency tuning. Before enabling Bungie startup, verify the Redis cluster has `d2_mani_version` and `d2mani_computed`; if `d2mani_computed` is missing, run the forced manifest update from the dev image with `WARMIND_FEATURES_SKIP_BUNGIE=true` so startup does not panic while computed manifest data is being rebuilt. The observed parser rate was about `520-540 PGCR/min`; cortex profile scans were active but lagged parser throughput even after raising cortex to 8 workers, so queue backlog is the main thing to monitor during future live dev runs.

Parser throttle follow-up on 2026-05-20: the live dev parser initially could not be slowed with `WARMIND_WARMIND_PARSER_MAX_WORKERS` because `parser.RunParser` hard-coded non-prod runs to 3 parser workers. The repo was patched to honor configured `warmind-parser.max-workers`, `max-retry-workers`, and `max-retries` with the old non-prod values as fallbacks only when config is unset. The ignored dev env was then set to `WARMIND_WARMIND_PARSER_MAX_WORKERS=1`, `WARMIND_WARMIND_PARSER_MAX_RETRY_WORKERS=1`, `WARMIND_WARMIND_PARSER_JOB_QUEUE_SIZE=2`, `WARMIND_WARMIND_PARSER_FORCED_LAG_SEC=3600`, and `WARMIND_WARMIND_PARSER_PAUSE=true`; after restarting `warmind-parser`, measured rate dropped to `1-2 PGCR/min`.

SPA dev fixture mode note added on 2026-05-21: [[Projects/Warmind/Warmind SPA dev fixture mode|Warmind SPA dev fixture mode]] documents the explicit fixture flag, Compose-local opt-in, prod refusal, destructive-tool confirmations, and route/API verification rule for SPA migration fixtures.

Devbox always-on update on 2026-05-21: Yellow & Blue Dev was missing from SPA `/s` server lists because dev `warmind-dbot` gateway/cache sync was off, not because the SPA filtered it out. `warmind-dcmds` can still receive Discord interactions while `discord.guilds`, `discord.guild_members`, roles, channels, and `charlemagne.guilds` are stale; `/spa/guilds` depends on those local dbot-maintained tables. Nyx dev should run the isolated dev app with live read-side/dev-isolated flags: `WARMIND_FEATURES_SKIP_BUNGIE=false`, `WARMIND_FEATURES_CORTEX_BUNGIE_API=true`, `WARMIND_WARMIND_PARSER_SKIP=false`, `WARMIND_WARMIND_DBOT_SKIP_DISCORD=false`, `WARMIND_DISCORD_FEATURES_SKIP_EVENT_HANDLING=false`, and `WARMIND_WARMIND_DCMDS_SKIP_COMMAND_SYNC=false`. Keep outbound side-effect loops such as SES, RSS sends, social posts, donator processing, Datadog, and notification fanout disabled unless a specific dev-only test needs them. Verified after restart: dbot synced 16 servers, Yellow & Blue Dev had `discord.guilds=1`, `guild_members=8`, `guild_channels=8`, `guild_roles=15`, `charlemagne.guilds=1`, and dev guild command status rows existed. The Compose smoke script now fails if configured dev guilds are not present in the local Discord cache.

Redis dev recovery update on 2026-05-21: after a reboot, the Nyx Warmind dev stack had Redis AOF failures in the local dev cache volumes (`appendonly.aof.5.incr.aof` and `appendonly.aof.10.incr.aof` bad file format). Only Redis dev-cache volumes were cleared; MySQL, Mongo, Elasticsearch, source files, and fixture bundles were not removed. Warmind now has `/Users/Shared/projects/warmind/dev/scripts/restore-dev-redis.sh` to rebuild dev Redis from deterministic sources: Bungie manifest cache, item analytics HLLs, SPA fixture Redis keys, parser/status keys, dbot condition, and Charlemagne membership maps. The map restore uses `/Users/Shared/projects/warmind/dev/tools/sync-redis-maps.go`, which refuses prod/prod-like environments. The Redis cluster init helper was fixed to avoid pre-creating cluster-meet state before `redis-cli --cluster create`, and to seed `nexus:stats:rawFailedPgcrCount` with the correct key name. Verification after restore: Compose smoke OK, `/in/sealAnalytics` returned JSON with `totalProfiles=72222122`, SPA visual route audit covered 99 routes across desktop/wide/mobile with `issueCount=0`, and `go test -tags warminddev ./discord/dcmds -count=1 -v` passed the command registry plus `/daily`, `/register`, and `/pve` fixture-backed command smokes. Follow-up in the same pass: `discord/dwork/dwork.go` now registers dev-disabled notification producer jobs as no-ops; after clearing stale local `dwork:dead` entries and restarting, `dwork:dead` stayed at 0 and the requeuer error stopped. Remaining dev log noise to watch: parser mode/hash warnings during live PGCR consumption and non-fatal rarity/manifest misses.

Logbook Tor_Kallon fixture import on 2026-06-04: imported `/Users/rourkem/warmind-logbook-fixtures/warmind-logbook-profile-fixture-tor-kallon-20260605-041058.sql` into Nyx Warmind dev MySQL via `docker compose -f compose.dev.yaml exec -T mysql mysql -uroot -pwarmind_root_dev`. Manifest `/Users/rourkem/warmind-logbook-fixtures/warmind-logbook-profile-fixture-tor-kallon-20260605-041058.manifest.txt` records membership `4611686018428592074`, clan `223562`, week `457`, and season `29`. Post-import counts matched the manifest: `charlemagne.users=1`, `charlemagne.user_bungie_net_accounts=1`, `discord.users=1`, `charlemagne.user_bungie_profiles=1`, `charlemagne.stats=457`, `nexus.profiles=1`, `nexus.clans=1`, `nexus.seals=1`, `nexus.seals_gilded=13`, `nexus.characters=3`, current-season `nexus.summary_atime=0`, `nexus.summary_crucible=141`, `nexus.summary_gambit=255`, `nexus.summary_gambit_prime=8`, and `nexus.summary_nightfalls=28`.

## Related
- [[Projects/Warmind/Parser scratch - 2026-06-04|Parser scratch - 2026-06-04]]
- [[Projects/Warmind/OpenSkill individual ratings plan - 2026-06-09|OpenSkill individual ratings plan - 2026-06-09]]
- [[Projects/Warmind/OpenSkill Go library implementation prompt - 2026-06-09|OpenSkill Go library implementation prompt - 2026-06-09]]
- [[Projects/Warmind/Logbook public profile fallback plan - 2026-06-05|Logbook public profile fallback plan - 2026-06-05]]
- [[Projects/Warmind/Final parser scorecard validation progress - 2026-06-05|Final parser scorecard validation progress - 2026-06-05]]
- [[Projects/Warmind/PGCR S3 production cutover runbook - 2026-06-05|PGCR S3 production cutover runbook - 2026-06-05]]
- [[Projects/Warmind/Final parser perf and full validation handoff - 2026-06-04|Final parser perf and full validation handoff - 2026-06-04]]
- [[Projects/Warmind/Final parser validation use guide - 2026-06-03|Final parser validation use guide - 2026-06-03]]
- [[Projects/Warmind/Final parser EC2 revalidation progress - 2026-06-03|Final parser EC2 revalidation progress - 2026-06-03]]
- [[Projects/Warmind/PGCR archive validation - 2026-06-01|PGCR archive validation - 2026-06-01]]
- [[Projects/Warmind/Unified D1 D2 parser AWS data validation progress - 2026-06-01|Unified D1 D2 parser AWS data validation progress - 2026-06-01]]
- [[Projects/Warmind/Unified D1 D2 parser final validation completion - 2026-05-31|Unified D1 D2 parser final validation completion - 2026-05-31]]
- [[Projects/Warmind/Unified D1 D2 parser final validation progress - 2026-05-28|Unified D1 D2 parser final validation progress - 2026-05-28]]
- [[Projects/Warmind/Warmind Codex onboarding tips for partner - 2026-05-22|Warmind Codex onboarding tips for partner - 2026-05-22]]
- [[Projects/Warmind/Mode 0 Override Europa investigation - 2026-06-09|Mode 0 Override Europa investigation - 2026-06-09]]
- [[Projects/Warmind/Mode 0 Override Moon investigation - 2026-06-24|Mode 0 Override Moon investigation - 2026-06-24]]
- [[Projects/Warmind/Mode 0 Override Last City investigation - 2026-06-16|Mode 0 Override Last City investigation - 2026-06-16]]
- [[Projects/Warmind/Mode 0 Last City social investigation - 2026-06-12|Mode 0 Last City social investigation - 2026-06-12]]
- [[Projects/Warmind/Mode 0 workaround investigation - 2026-05-27|Mode 0 workaround investigation - 2026-05-27]]
- [[Projects/Warmind/D2 raw_failed_pgcr stale failure audit - 2026-05-28|D2 raw_failed_pgcr stale failure audit - 2026-05-28]]
- [[Projects/Warmind/D2 parser D1-shape autotuning handoff - 2026-05-26|D2 parser D1-shape autotuning handoff - 2026-05-26]]
- [[Projects/Warmind/D1 parser milestone 1 task list - 2026-05-21|D1 parser milestone 1 task list]]
- [[Projects/Warmind/D1 parser milestone 1 cleanup and product queues - 2026-05-22|D1 parser milestone 1 cleanup and product queues - 2026-05-22]]
- [[Projects/Warmind/Warmind dependency vulnerability exposure analysis - 2026-05-18|Warmind dependency vulnerability exposure analysis - 2026-05-18]]
- [[Projects/Warmind/Warmind Docker multi-stage build note|Warmind Docker multi-stage build note]]
- [[INDEX|Shared knowledge index]]
- [[Active projects]]
- [[Projects/Charlemagne/Charlemagne|Charlemagne]]
- [[Projects/Joyeuse/Joyeuse|Joyeuse]]
