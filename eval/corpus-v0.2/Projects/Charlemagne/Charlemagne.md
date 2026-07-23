Created: 2026-04-18 12:08 PDT
Updated: 2026-06-24 PDT
Status: Active
Health: At risk

## Purpose
Charlemagne is the legacy/live Destiny bot and community leaderboard system, currently maintained more as an operating service than an expanding product.

## Current focus
Reducing AWS and storage cost while validating a smaller single-host footprint ahead of the July 2026 RI expiry.

## Next step
Use the new rhea storage and MySQL tuning findings to pick the next maintenance-window items, then prove the system is stable in a 128 GiB envelope before moving to a smaller host.

## Status summary
- Current work is mostly ops, cost, and risk management rather than feature expansion.
- Business pressure is real: MRR is modest and trending down while infra cost is still heavy.
- The strongest current recommendation is to keep the system single-host and validate downsizing before any commitment.
- Storage, snapshots, and MySQL memory are the main practical levers.

## Watch items
- July 2026 RI expiry and downsizing timing
- snapshot/archive cleanup
- MySQL memory footprint
- EBS shrink feasibility

## Key links
- [[Projects/Charlemagne/Research/Logbook Pantheon 2.0 research 2026-06-09|Logbook Pantheon 2.0 research - 2026-06-09]]
- [[Active projects]]
- [[Home]]
- [[Projects/Metis/Metis|Metis]]
- [[Projects/Charlemagne/Costs/Costs|Charlemagne costs]]
- [[Projects/Charlemagne/Costs/rhea-costs-2026-03/rhea-cost-files|AWS March 2026 cost files]]
- [[Projects/Charlemagne/Costs/Charlemagne top cost cuts - 2026-04|Charlemagne top cost cuts]]
- [[Projects/Charlemagne/Costs/Charlemagne storage cost action plan - 2026-04|Charlemagne storage cost action plan]]
- [[Projects/Charlemagne/Research/Charlemagne infrastructure research|Charlemagne infrastructure research]]
- [[Projects/Charlemagne/Research/Charlemagne rhea storage and MySQL tuning findings - 2026-05-22|Charlemagne rhea storage and MySQL tuning findings - 2026-05-22]]
- [[Projects/Charlemagne/Research/Charlemagne rhea storage bleed triage - 2026-06-24|Charlemagne rhea storage bleed triage - 2026-06-24]]
- [[Charlemagne alerts diagnostic handoff - 2026-06-24|Charlemagne alerts diagnostic handoff - 2026-06-24]]
- [[Projects/Charlemagne/Research/Charlemagne lore naming check - 2026-05-31|Charlemagne lore naming check - 2026-05-31]]
- [[Projects/Charlemagne/Logbook|Logbook]]
- [[Projects/Charlemagne/Research/Charlemagne Guardian Profile Homepage Plan - 2026-06-02|Guardian profile homepage plan]]
- [[Projects/Charlemagne/Research/Charlemagne guardian profile homepage research - 2026-06-02|Guardian profile research]]
- [[Projects/Charlemagne/Research/Charlemagne Destiny activity social network plan - 2026-06-02|Destiny activity social network plan]]
- [[Projects/Charlemagne/Research/Charlemagne session tracking backend implementation - 2026-06-04|Session tracking backend implementation - 2026-06-04]]
- [[Projects/Charlemagne/Research/Charlemagne session tracking storage retention plan - 2026-06-16|Session tracking storage retention plan - 2026-06-16]]
- [[Projects/Charlemagne/Research/Charlemagne SPA page-family adoption telemetry plan - 2026-06-07|SPA page-family adoption telemetry plan - 2026-06-07]]
- [[Projects/Charlemagne/Research/Charlemagne Destiny 2 engagement feature ideas - 2026-06-02|D2 engagement feature ideas]]
- [[Projects/Charlemagne/Research/Charlemagne SPA static asset ownership - 2026-06-06|SPA static asset ownership - 2026-06-06]]
- [[Projects/Charlemagne/Charlemagne coding standards|Charlemagne coding standards]]
- [[Projects/Charlemagne/Charlemagne legacy web repo map - 2026-05-18|Charlemagne legacy web repo map - 2026-05-18]]
- [[Projects/Charlemagne/Charlemagne GA4 route migration analysis - 2026-05-19|Charlemagne GA4 route migration analysis - 2026-05-19]]
- [[Projects/Charlemagne/Charlemagne SPA big-bang migration plan - 2026-05-19|Charlemagne SPA big-bang migration plan - 2026-05-19]]
- [[Final SPA feedback - 2026-06-03|Final SPA feedback - 2026-06-03]]
- [[Projects/Charlemagne/Charlemagne SPA migration test data gaps - 2026-05-19|Charlemagne SPA migration test data gaps - 2026-05-19]]
- [[Projects/Charlemagne/Charlemagne production routing audit - 2026-05-19|Charlemagne production routing audit - 2026-05-19]]
- [[Projects/Charlemagne/Charlemagne route fixture Erebus prompt - 2026-05-19|Charlemagne route fixture Erebus prompt - 2026-05-19]]
- [[Projects/Charlemagne/Research/Charlemagne MySQL storage reduction guide - 2026-04|Charlemagne MySQL storage reduction guide]]
- [[Projects/Charlemagne/Charlemagne EC2 runtime inventory|Charlemagne EC2 runtime inventory]]
- [[Projects/Charlemagne/Charlemagne runtime inventory - 2026-04-15|Charlemagne runtime inventory - 2026-04-15]]

## UpNote imports

- [[Projects/Charlemagne/UpNote imported Charlemagne notes]] — 2 imported items
