# Aether heartbeat policy

Run the automation-chief health check and treat its JSON as current truth.

If the check is healthy and nothing else needs attention, reply exactly:

`HEARTBEAT_OK`

If it is unhealthy, report only:

- the failing or stale task names;
- each task's last error;
- the status path.

Do not rerun managed jobs from a heartbeat. The scheduler and supervisor own
recovery. Do not infer or repeat old tasks merely because they appeared in
prior conversations.
