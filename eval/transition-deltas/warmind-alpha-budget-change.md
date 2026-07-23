# Evaluation fixture: Warmind alpha budget change

This is controlled transition-test input, not production history.

Observed at: 2026-07-11T07:00:00Z
Authority: explicit operator constraint

The next parser experiment is now bounded to a 30-minute dev-only alpha soak. It must not change production and must not add durable controller persistence.

Acceptance requires all of the following:

- at least 900 PGCR/sec steady throughput during the final 10 minutes
- zero integrity or recovery violations
- no DB connection failure storm
- successful restart from DB truth after the controlled stop

Abort and roll back the trial if MySQL consumes more than 90 percent of host memory for two consecutive minutes or DB connection failures exceed three per minute. Keep batch 128 as the initial ceiling while allowing fetch to adapt. If the alpha gate passes, advance to the beta recovery test. If it fails, preserve the evidence as a bad trial and revise the controller before continuing.

This constraint replaces the earlier open-ended wording "run a longer alpha soak." It does not supersede the parser integrity and recovery rules.
