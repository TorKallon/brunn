# September 6 location pilot

This preparation implements the plan-v2 4C measurement protocol. The question file is frozen before summary generation. Keep `rubric.json`, expected anchors, synthetic truth, and results away from the summary compiler and answer arms. The checked-in files contain questions, reviewed semantic anchors, and synthetic cases, not a private raw archive.

The runner compares one equal-budget batch of seven untouched questions in each arm:

- A: existing retrieval through an explicit bounded HTTP read plan, with ordinary exact source reads.
- B: validated cached-summary `current_state` plus exact-version source follow-up.
- C: the frozen complete packet, an offline-prepared evidence ceiling.

The read plans are fixed and reviewed before running; this harness does not measure adaptive model tool selection. C has zero online HTTP calls because the packet is supplied directly; report its packet-collection cost separately and do not interpret this as a production latency gain. Each arm uses a fresh ChatGPT-authenticated `gpt-6-astra` / `ultra` invocation, the same ten-minute deadline, 250,000-byte complete-input limit, eight HTTP-call limit, seven questions, and 32-KiB answer limit. Oversized inputs fail instead of truncating evidence. Token usage comes from CLI events; missing usage remains null.

1. After deployment, obtain one complete `location.evidence.v1` packet for `[2026-09-06T07:00:00Z, 2026-09-07T07:00:00Z)`, America/Los_Angeles, through the authorized wrapper/owner path. Record collection time, source cutoff, packet fingerprint, deployed revision, canonical versions, and legacy receipt limitations. Keep private files outside the repository or under ignored `operator-output/` with mode 0600.
2. Freeze before summary compilation:

   ```sh
   python3 scripts/location_pilot.py freeze --packet /private/path/packet.json --output /private/path/pilot --cutoff 2026-09-08T00:00:00Z
   ```

   Replace that example cutoff with the actually recorded cutoff. Freeze stores the packet and question hashes. Do not feed questions, rubric, expected anchors, or earlier answers to the compiler. Compile from the frozen packet only. Record compilation wall time, model/effort, input/output tokens, source calls, auth status, and exact accepted candidate/summary version in a separate compilation ledger. Preserve owner approval and report-only/full-mode gates.
3. Fill the read-plan template with actual frozen canonical references, versions, and line selectors. Compare returned source identities/content to the frozen packet. B must return the accepted validated summary, not a fallback; otherwise the arm is ineligible. A must not be hydrated with that summary. Capture both plans and their hashes before model runs. An evidence correction makes a new pilot cohort; never mix it into the current cohort. Independently grade all returned sources before attributing quality or speed differences to summary use.
4. Provide an exclusively assigned, already ChatGPT-authenticated Codex home. The harness passes no Brunn credentials, API keys, proxy variables, or model routing overrides to Codex; user configuration, rules, apps, and plugins are ignored. Native Codex owns refresh in that home, with no copied-auth writeback. `codex login status` must report `Logged in using ChatGPT`. Plan limits or auth failure stop the arm without API billing fallback. This script does not provision or mutate credentials.
5. Execute one explicitly authorized arm at a time:

   ```sh
   python3 scripts/location_pilot.py run --execute --bundle /private/path/pilot --plans /private/path/plans.json --arm A --cache cold --api-url http://127.0.0.1:18110 --codex /absolute/path/codex --codex-home /private/path/dedicated-codex
   ```

   A/B read `BRUNN_PILOT_READ_TOKEN` in the parent only. Do not put its value on the command line. C needs no service credential. Repeat warm requests and all three arms with the same fixed bundle/cutoffs, and record arm order. Cold/warm are honest operator labels: the script does not flush shared caches. Preserve failed answers. Existing arm/cache results cannot be overwritten. Use multiple fresh cohorts for counterbalanced order if making a statistical claim.
6. Grade against exact sources using `rubric.json`. Populate interval containment **and width**, observed-stop coverage, unsupported stops/venues/activities/routes, continuous-presence claims, correction retention, and citation validity. The known four evening anchors are evidence cases, not exhaustive physical truth. Report exhaustive all-day recall as unverified. Run the synthetic independent-stop cases separately; they include short adjacent stops, sparse vehicle travel, traffic pauses, tied POIs, cross-midnight boundaries, late correction, and unknown departure/receipt.
7. Report supported-answer latency only after a passing quality grade. Keep failed/unsupported attempts in the denominator. Compare B to A for the >=30% online token or source-call target, equal answer quality, and no client latency regression. Packet collection and summary compilation are offline costs and must remain visible. No retrospective arm can pass prospective capture-timeliness acceptance; unknown legacy receipt times remain null.

The harness is prepared and unit-tested, not a completed pilot or a release-quality result. The real production packet, accepted summary, reviewed read plans, compilation ledger, model runs, and independent grading remain required.
