use super::*;

fn merge(current: &mut Value, response: &Value, state_version: &mut i64) {
    if let Some(version) = response["state_version"].as_i64() {
        *state_version = version;
    }
    if let (Some(current), Some(response)) = (current.as_object_mut(), response.as_object()) {
        current.extend(response.clone());
    }
}

fn envelope(current: &Value, state_version: i64, operation_id: &str) -> Value {
    json!({"attempt_id":current["attempt_id"],"fence":current["fence"],
        "expected_state_version":state_version,"operation_id":operation_id,
        "subject_ref":current["research"]["subject_ref"],
        "research_version":current["research"]["version"]})
}

fn progress(step: &research::Step, status: &str, current: &Value) -> Value {
    let mut value = json!({"notes":step.notes,"reviewed_sources":step.reviewed_sources,
        "pending_queries":step.pending_queries,"pending_targets":step.pending_targets,"status":status});
    if research::checkpoints_enabled(current) {
        value["checkpoint_protocol"] = json!(research::CHECKPOINT_PROTOCOL);
        value["reconciled_checkpoints"] = json!(step.reconciled_checkpoints);
        value["findings"] = json!(step.findings);
    }
    if let Some(pointer) = &step.covers_existing {
        value["covers_existing"] = pointer.clone();
    }
    if let Some(pointer) = &step.supersedes_existing {
        value["supersedes_existing"] = pointer.clone();
    }
    if let Some(follow_up) = &step.follow_up {
        value["follow_up"] = follow_up.clone();
    }
    if step
        .follow_up
        .as_ref()
        .is_some_and(|route| route.get("origin_source").is_some())
        || step.resolved_follow_ups.is_some()
    {
        value["follow_up_protocol"] = json!(research::FOLLOW_UP_PROTOCOL);
        value["findings"] = json!(step.findings);
    }
    if let Some(resolved) = &step.resolved_follow_ups {
        value["resolved_follow_ups"] = json!(resolved);
    }
    value
}

fn follow_up_ack(
    step: &research::Step,
    operation_id: &str,
    response: &Value,
    accepted: bool,
) -> Result<(), ClientError> {
    if step
        .follow_up
        .as_ref()
        .is_none_or(|route| route.get("origin_source").is_none())
        && step.resolved_follow_ups.is_none()
    {
        return Ok(());
    }
    let ack = &response["follow_up_receipt"];
    let expected = if accepted {
        step.resolved_follow_ups
            .as_ref()
            .map_or(json!([]), |resolved| json!(resolved))
    } else {
        json!([])
    };
    if ack["protocol"] != research::FOLLOW_UP_PROTOCOL
        || ack["operation_id"] != operation_id
        || ack["recorded"] != true
        || ack["replayed"].as_bool().is_none()
        || ack["resolved_follow_ups"] != expected
    {
        return Err(ClientError::Failed(
            "source follow-up acknowledgement was missing or did not match the operation".into(),
        ));
    }
    Ok(())
}

fn draft_ack(
    operation_id: &str,
    response: &Value,
    candidate_hash: &str,
    expected_pointer: Option<&research::DraftPointer>,
    retired: bool,
) -> Result<research::DraftPointer, ClientError> {
    let ack = &response["draft_receipt"];
    let pointer = serde_json::from_value::<research::DraftPointer>(ack["pointer"].clone())
        .ok()
        .filter(research::DraftPointer::valid);
    if ack["protocol"] == research::DRAFT_PROTOCOL
        && ack["operation_id"] == operation_id
        && ack["recorded"] == true
        && ack["replayed"].as_bool().is_some()
        && ack["retired"] == retired
        && let Some(pointer) = pointer
        && pointer.candidate_hash == candidate_hash
        && expected_pointer.is_none_or(|expected| expected == &pointer)
    {
        return Ok(pointer);
    }
    Err(ClientError::Failed(
        "unaccepted draft custody acknowledgement is missing or mismatched; reconcile before claiming acceptance".into(),
    ))
}

struct CheckpointAck {
    new_source_coverage: bool,
    reconciled: bool,
    subject_complete: bool,
    replayed: bool,
}

fn checkpoint_ack(
    current: &Value,
    operation_id: &str,
    response: &Value,
) -> Result<Option<CheckpointAck>, ClientError> {
    if !research::checkpoints_enabled(current) {
        return Ok(None);
    }
    let ack = &response["checkpoint_receipt"];
    let booleans = (
        ack["new_source_coverage"].as_bool(),
        ack["reconciled"].as_bool(),
        ack["subject_complete"].as_bool(),
    );
    if ack["operation_id"] == operation_id
        && ack["protocol"] == research::CHECKPOINT_PROTOCOL
        && ack["recorded"] == true
        && (ack.get("replayed").is_none() || ack["replayed"].is_boolean())
        && let (Some(new_source_coverage), Some(reconciled), Some(subject_complete)) = booleans
    {
        return Ok(Some(CheckpointAck {
            new_source_coverage,
            reconciled,
            subject_complete,
            replayed: ack["replayed"] == true,
        }));
    }
    Err(ClientError::Failed(
        "incremental research checkpoint was not acknowledged; work retained for reconciliation"
            .into(),
    ))
}

/// Compare reviewed line coverage across the whole selected turn. Reordering,
/// splitting ranges or alternating earlier selectors cannot restart its budget.
#[derive(Default)]
struct ReviewedCoverage(BTreeMap<(String, i64), Vec<(u64, u64)>>);

impl ReviewedCoverage {
    fn include(&mut self, selectors: &Value) -> bool {
        let mut novel = false;
        for selector in selectors.as_array().into_iter().flatten() {
            let (Some(reference), Some(version), Some(start), Some(end)) = (
                selector["entry_ref"].as_str(),
                selector["version"].as_i64(),
                selector["start_line"].as_u64(),
                selector["end_line"].as_u64(),
            ) else {
                continue;
            };
            if start == 0 || end < start {
                continue;
            }
            let ranges = self.0.entry((reference.to_owned(), version)).or_default();
            novel |= !ranges.iter().any(|(a, b)| *a <= start && *b >= end);
            ranges.push((start, end));
            ranges.sort_unstable();
            let mut merged: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
            for &(a, b) in ranges.iter() {
                if let Some(previous) = merged.last_mut()
                    && a <= previous.1.saturating_add(1)
                {
                    previous.1 = previous.1.max(b);
                } else {
                    merged.push((a, b));
                }
            }
            *ranges = merged;
        }
        novel
    }

    fn acknowledged_progress(
        &mut self,
        before: &Value,
        response: &Value,
        step: &research::Step,
        ack: Option<&CheckpointAck>,
    ) -> bool {
        let novel = self.include(&json!(step.reviewed_sources));
        let unresolved = |value: &Value| -> Option<usize> {
            (value["research"]["checkpoint_context_status"] == "available").then(|| {
                value["research"]["checkpoint_contexts"]
                    .as_array()
                    .map_or(0, Vec::len)
                    + usize::from(value["research"]["current_checkpoint"].is_object())
            })
        };
        let reduced = match (unresolved(before), unresolved(response)) {
            (Some(before), Some(after)) => after < before,
            _ => false,
        };
        ack.is_some_and(|ack| {
            !ack.replayed && ((ack.new_source_coverage && novel) || (ack.reconciled && reduced))
        })
    }
}

fn add_fields(body: &mut Value, fields: Value) {
    body.as_object_mut()
        .expect("request object")
        .extend(fields.as_object().expect("fields").clone());
}

fn discovery_progress(current: &Value) -> Value {
    let research = &current["research"];
    json!([
        research["sources"],
        research["coverage"]["change_cursor"],
        research["coverage"]["change_upper"],
        research["coverage"]["change_status"]
    ])
}

fn retained_repair(current: &Value) -> Option<research::RepairFeedback> {
    serde_json::from_value::<research::RepairFeedback>(
        current["research"]["repair_feedback"].clone(),
    )
    .ok()
    .filter(research::RepairFeedback::valid)
}

fn subject_allowance(remaining: Duration) -> Duration {
    // Source review and final composition share this fixed allowance. Ultra
    // reasoning needs room to finish after its discovery/checkpoint rounds.
    Duration::from_secs(1_200).min(remaining / 2)
}

impl Dreamer {
    async fn retain_research_draft(
        &self,
        current: &mut Value,
        state_version: &mut i64,
        step: &research::Step,
        deadline: tokio::time::Instant,
    ) -> Result<Option<research::DraftPointer>, ClientError> {
        if !research::draft_custody_required(current) {
            return Ok(None);
        }
        if step.candidates.len() != 1 {
            return Err(ClientError::Failed(
                "draft custody requires one complete candidate".into(),
            ));
        }
        let operation_id = uuid::Uuid::now_v7().to_string();
        let mut body = envelope(current, *state_version, &operation_id);
        add_fields(
            &mut body,
            json!({"draft_protocol":research::DRAFT_PROTOCOL,
                "draft_candidate":step.candidates[0],"findings":step.findings,
                "processed_inputs":[]}),
        );
        let offered = &current["research"]["unaccepted_draft"];
        if offered["status"] == "unaccepted_revalidation_only" {
            let pointer: research::DraftPointer =
                serde_json::from_value(offered["pointer"].clone()).map_err(|_| {
                    ClientError::Failed(
                        "the offered draft identity is invalid; draft retained".into(),
                    )
                })?;
            if !pointer.valid() {
                return Err(ClientError::Failed(
                    "the offered draft identity is invalid; draft retained".into(),
                ));
            }
            body["replaces_draft"] = json!(pointer);
        }
        let response = self
            .research_request("research-progress", body, deadline)
            .await?;
        let pointer = draft_ack(
            &operation_id,
            &response,
            &research::draft_hash(&step.candidates[0]),
            None,
            false,
        )?;
        merge(current, &response, state_version);
        Ok(Some(pointer))
    }

    async fn record_research_repair(
        &self,
        current: &mut Value,
        state_version: &mut i64,
        repair: &research::RepairFeedback,
        status: &str,
        deadline: tokio::time::Instant,
    ) -> Result<(), ClientError> {
        let operation_id = uuid::Uuid::now_v7().to_string();
        let mut body = envelope(current, *state_version, &operation_id);
        add_fields(
            &mut body,
            json!({"status":status,"repair_feedback":repair,"processed_inputs":[]}),
        );
        let response = self
            .research_request("research-progress", body, deadline)
            .await?;
        // Safe source projection may withhold the diagnostic. Its explicit
        // receipt still confirms custody; an older API ignoring this field
        // must not look like a successful repair checkpoint.
        if response["repair_feedback_receipt"]["operation_id"] != operation_id
            || response["repair_feedback_receipt"]["recorded"] != true
        {
            return Err(ClientError::Failed(
                "research repair checkpoint was not acknowledged; work retained for reconciliation"
                    .into(),
            ));
        }
        merge(current, &response, state_version);
        Ok(())
    }

    /// Retry transport ambiguity with the exact operation identity/payload.
    /// Validation feedback is handled by another reasoning round, never by
    /// silently weakening or partially publishing a rejected candidate.
    async fn research_request(
        &self,
        operation: &str,
        body: Value,
        deadline: tokio::time::Instant,
    ) -> Result<Value, ClientError> {
        let request = async {
            match self.runner.dreamer(operation, body.clone()).await {
                Ok(value) => Ok(value),
                Err(ClientError::Failed(detail)) if detail.starts_with("POST ") => {
                    self.runner.dreamer(operation, body).await
                }
                Err(error) => Err(error),
            }
        };
        tokio::time::timeout_at(deadline, request)
            .await
            .unwrap_or_else(|_| {
                Err(ClientError::Failed(format!(
                    "{operation} timed out; server progress remains retained for reconciliation"
                )))
            })
    }

    async fn refresh_research_evidence(
        &self,
        current: &mut Value,
        state_version: &mut i64,
        deadline: tokio::time::Instant,
    ) -> Result<(), ClientError> {
        let mut body = envelope(current, *state_version, &uuid::Uuid::now_v7().to_string());
        add_fields(&mut body, json!({"queries":[],"targets":[]}));
        let response = self
            .research_request("narrative-discover", body, deadline)
            .await?;
        merge(current, &response, state_version);
        Ok(())
    }

    pub(super) async fn research_loop(
        &self,
        admission: &Value,
        state_version: &mut i64,
        report: &mut RunReport,
        run_home: &RunHome,
        env: &BTreeMap<String, String>,
        deadline: tokio::time::Instant,
        location_outcome: Option<RunOutcome>,
    ) -> RunOutcome {
        let initial_remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        // A fixed entry-time margin stops repeated half-budget subject turns
        // from selecting ever smaller work at the end of the run.
        let selection_margin = Duration::from_secs(60).min(initial_remaining / 10);
        let mut current = admission.clone();
        current["state_version"] = json!(*state_version);
        let mut rounds = 0usize;
        let mut change_pages = 0usize;
        let mut routed_discoveries = 0usize;
        let mut completed_subjects = 0usize;
        let mut yielded_subjects = 0usize;
        let mut processed = 0usize;
        let mut accepted = report.research["new_review_items"].as_u64().unwrap_or(0) as usize;
        let mut stop = "selected_work_complete".to_owned();
        let mut quota_limited = false;
        let mut exhausted = false;
        let mut failure = None;
        let mut subjects_seen = std::collections::BTreeSet::new();

        'subjects: loop {
            if subject_allowance(deadline.saturating_duration_since(tokio::time::Instant::now()))
                <= selection_margin
            {
                stop = "time_exhausted".into();
                break;
            }
            report.stage = "research_selection".into();
            let body = envelope(&current, *state_version, &uuid::Uuid::now_v7().to_string());
            let response = match self.research_request("research-next", body, deadline).await {
                Ok(value) => value,
                Err(error) => {
                    failure = Some(format!("research selection retained for retry: {error}"));
                    break;
                }
            };
            merge(&mut current, &response, state_version);
            let Some(subject) = current["research"]["subject_ref"]
                .as_str()
                .map(str::to_owned)
            else {
                exhausted = true;
                break;
            };
            if subject_allowance(deadline.saturating_duration_since(tokio::time::Instant::now()))
                <= selection_margin
            {
                stop = "time_exhausted".into();
                break;
            }
            // A faulty or old server must not cause an endless same-job loop.
            if !subjects_seen.insert(subject.clone()) {
                failure = Some("research selection repeated a subject already serviced in this attempt; progress retained".into());
                break;
            }
            // Retained corrections are read from the current safe projection
            // in each prompt, never cached across a source-access refresh.
            let mut feedback = String::new();
            let mut pending_repair = None;
            let mut repairs = 0usize;
            // Successful discovery cannot erase a rejected checkpoint. Only a
            // subsequently accepted checkpoint resets this part of the budget.
            let mut refresh_rejections = 0usize;
            let mut no_progress = 0usize;
            let mut subject_round = 0usize;
            let mut routed_attempted = std::collections::BTreeSet::new();
            let mut reviewed_coverage = ReviewedCoverage::default();
            reviewed_coverage.include(&current["research"]["reviewed_sources"]);
            // Leave time for another subject even if this model invocation
            // stalls. Evidence already admitted survives the bounded turn.
            let now = tokio::time::Instant::now();
            let subject_deadline = now + subject_allowance(deadline.saturating_duration_since(now));
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    stop = "time_exhausted".into();
                    break 'subjects;
                }
                let pass_exhausted = current["research"]["pass"]["rounds_remaining"]
                    .as_u64()
                    .is_some_and(|remaining| remaining == 0);
                let yield_subject = repairs + refresh_rejections >= 2
                    || no_progress >= 2
                    || pass_exhausted
                    || subject_round >= 32
                    || subject_deadline.saturating_duration_since(tokio::time::Instant::now())
                        <= selection_margin;
                if let Some(repair) = pending_repair.take() {
                    let status = if yield_subject {
                        "waiting"
                    } else {
                        "researching"
                    };
                    if let Err(error) = self
                        .record_research_repair(
                            &mut current,
                            state_version,
                            &repair,
                            status,
                            deadline,
                        )
                        .await
                    {
                        failure = Some(format!(
                            "research correction could not be checkpointed: {error}"
                        ));
                        break 'subjects;
                    }
                    feedback.clear();
                    // Custody is operational, not accepted model progress.
                    // It does not reset either validation-repair counter.
                    if yield_subject {
                        yielded_subjects += 1;
                        break;
                    }
                }
                // Each subject retains its continuation if it needs more than
                // one run. The next subject still gets an opportunity today.
                if yield_subject {
                    let mut body =
                        envelope(&current, *state_version, &uuid::Uuid::now_v7().to_string());
                    add_fields(
                        &mut body,
                        json!({"status":"waiting",
                        "findings":[if pass_exhausted{"This pass used its model-round allowance; retained notes and drafts continue in a new pass at a newer evidence cutoff."}else if feedback.is_empty(){"Further research remains; this subject yielded so other work can proceed."}else{&feedback}],
                        "processed_inputs":[]}),
                    );
                    match self
                        .research_request("research-progress", body, deadline)
                        .await
                    {
                        Ok(value) => merge(&mut current, &value, state_version),
                        Err(error) => {
                            failure = Some(format!(
                                "research continuation could not be checkpointed: {error}"
                            ));
                            break 'subjects;
                        }
                    }
                    yielded_subjects += 1;
                    break;
                }
                let coverage = &current["research"]["coverage"];
                let cursor = coverage["change_cursor"].as_i64().unwrap_or(0);
                if coverage["change_status"] == "unchecked"
                    && coverage["change_upper"]
                        .as_i64()
                        .is_some_and(|upper| cursor < upper)
                    && matches!(
                        coverage["change_reason"].as_str(),
                        Some(
                            "subject_change_check_limit"
                                | "subject_change_coverage_incomplete"
                                | "research_change_tail_pending"
                        )
                    )
                {
                    let mut body =
                        envelope(&current, *state_version, &uuid::Uuid::now_v7().to_string());
                    add_fields(&mut body, json!({"queries":[],"targets":[]}));
                    report.stage = "research_reconciliation".into();
                    match self
                        .research_request("narrative-discover", body, deadline)
                        .await
                    {
                        Ok(value) => {
                            merge(&mut current, &value, state_version);
                            change_pages += 1;
                            if current["research"]["coverage"]["change_cursor"]
                                .as_i64()
                                .unwrap_or(0)
                                <= cursor
                                && current["research"]["coverage"]["change_status"] != "complete"
                            {
                                feedback = "The bounded source-change scan made no further progress. Its cursor and remaining work are retained.".into();
                                repairs = 2;
                            }
                        }
                        Err(error) => {
                            feedback = error.to_string();
                            repairs = 2;
                        }
                    }
                    continue;
                }
                // Routed enrichment is durable work, not a model-owned hint.
                // Admit it through the normal evidence boundary before asking
                // the model to assess it. Unavailable references stay retained
                // and are tried at most once in this subject's bounded turn.
                let targets: Vec<String> = current["research"]["routed_targets"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter(|reference| !routed_attempted.contains(*reference))
                    .take(32)
                    .map(str::to_owned)
                    .collect();
                if !targets.is_empty() {
                    routed_attempted.extend(targets.iter().cloned());
                    let mut body =
                        envelope(&current, *state_version, &uuid::Uuid::now_v7().to_string());
                    add_fields(&mut body, json!({"queries":[],"targets":targets}));
                    report.stage = "research_follow_up_discovery".into();
                    match self
                        .research_request("narrative-discover", body, deadline)
                        .await
                    {
                        Ok(value) => {
                            merge(&mut current, &value, state_version);
                            routed_discoveries += 1;
                        }
                        Err(error) => {
                            feedback = format!(
                                "Routed primary evidence could not be admitted: {error}. Keep unresolved routed input retained."
                            );
                            repairs += 1;
                        }
                    }
                    continue;
                }
                subject_round += 1;
                let round_budget =
                    subject_deadline.saturating_duration_since(tokio::time::Instant::now());
                // Discovery or repair custody may have used the useful tail.
                // Return through the normal yield checkpoint without a child.
                if round_budget <= selection_margin {
                    subject_round -= 1;
                    continue;
                }
                rounds += 1;
                report.stage = "subject_research".into();
                let input = research::prompt(&current, &feedback, round_budget.as_secs());
                let name = format!("research-{}-{subject_round}-answer.md", subjects_seen.len());
                let model_started = tokio::time::Instant::now();
                let result = self
                    .exec_codex(run_home, env, &input, round_budget, &name)
                    .await;
                match result {
                    ExecResult::Finished => {}
                    ExecResult::TimedOut => {
                        report.record_model_failure(
                            &name,
                            model_started.elapsed(),
                            codex::ExecutionFailure::new(codex::FailureKind::Timeout),
                        );
                        let status = "The subject's time allowance ended. Its admitted evidence and saved conclusions remain available for another attempt.";
                        feedback = retained_repair(&current).map_or_else(
                            || status.to_owned(),
                            |repair| format!("{} {status}", repair.message),
                        );
                        repairs = 2;
                        continue;
                    }
                    ExecResult::Failed(failure) => {
                        let usage_limited = failure.kind == codex::FailureKind::UsageLimit;
                        let diagnostic = failure.summary();
                        report.record_model_failure(&name, model_started.elapsed(), failure);
                        if usage_limited {
                            quota_limited = true;
                            stop = "account_limits".into();
                            break 'subjects;
                        }
                        feedback = format!(
                            "The previous model process did not return a usable response: {diagnostic}. Resume from the persisted evidence and return the documented JSON object."
                        );
                        repairs += 1;
                        continue;
                    }
                }
                let parsed = std::fs::read_to_string(run_home.work_dir.join(&name))
                    .map_err(|_| "research response file is missing".to_owned())
                    .and_then(|raw| research::parse(&raw, &current));
                let step = match parsed {
                    Ok(step) => step,
                    Err(error) => {
                        pending_repair = Some(research::RepairFeedback::new(
                            research::RepairPhase::ResponseValidation,
                            &error,
                        ));
                        feedback = error;
                        repairs += 1;
                        continue;
                    }
                };
                let mut body =
                    envelope(&current, *state_version, &uuid::Uuid::now_v7().to_string());
                let operation_id = body["operation_id"]
                    .as_str()
                    .expect("operation id")
                    .to_owned();
                match step.action {
                    research::Action::Checkpoint => {
                        add_fields(&mut body, progress(&step, "researching", &current));
                        body["processed_inputs"] = json!([]);
                        report.stage = "research_checkpoint".into();
                        match self
                            .research_request("research-progress", body, deadline)
                            .await
                        {
                            Ok(value) => {
                                let ack = match checkpoint_ack(&current, &operation_id, &value) {
                                    Ok(ack) => ack,
                                    Err(error) => {
                                        failure = Some(error.to_string());
                                        break 'subjects;
                                    }
                                };
                                let made_progress = reviewed_coverage.acknowledged_progress(
                                    &current,
                                    &value,
                                    &step,
                                    ack.as_ref(),
                                );
                                merge(&mut current, &value, state_version);
                                if !ack.as_ref().is_some_and(|ack| ack.replayed) {
                                    repairs = 0;
                                    refresh_rejections = 0;
                                }
                                if made_progress {
                                    no_progress = 0;
                                    feedback.clear();
                                } else {
                                    no_progress += 1;
                                    feedback = "The checkpoint is retained, but it adds no new reviewed coverage or reduction of unresolved checkpoints. Continue with the next useful source, submit the supported overview, or yield.".into();
                                }
                            }
                            Err(ClientError::ResearchRefreshRequired(detail)) => {
                                refresh_rejections += 1;
                                let message = format!(
                                    "The previous research checkpoint was not saved: {detail}. Reread the refreshed sources and reconsider the rejected conclusions before saving them."
                                );
                                pending_repair = Some(research::RepairFeedback::new(
                                    research::RepairPhase::CheckpointValidation,
                                    &message,
                                ));
                                feedback = message;
                                report.stage = "research_reconciliation".into();
                                // The server identified stale evidence. Refresh
                                // its headers before another child, preserving
                                // the rejected work's repair and retry bounds.
                                match self
                                    .refresh_research_evidence(
                                        &mut current,
                                        state_version,
                                        deadline,
                                    )
                                    .await
                                {
                                    Ok(()) => change_pages += 1,
                                    Err(error) => {
                                        feedback = format!(
                                            "{feedback} Evidence refresh remains unresolved: {error}"
                                        );
                                        repairs += 1;
                                    }
                                }
                            }
                            Err(error) => {
                                if let ClientError::ResearchValidation(repair) = &error {
                                    pending_repair = Some(repair.clone());
                                }
                                feedback = error.to_string();
                                repairs += 1;
                            }
                        }
                    }
                    research::Action::Discover => {
                        let mut rejected_checkpoint = None;
                        let mut checkpoint_progress = false;
                        if !step.reviewed_sources.is_empty() {
                            add_fields(&mut body, progress(&step, "researching", &current));
                            body["processed_inputs"] = json!([]);
                            body["findings"] = json!(step.findings);
                            match self
                                .research_request("research-progress", body, deadline)
                                .await
                            {
                                Ok(value) => {
                                    let ack = match checkpoint_ack(&current, &operation_id, &value)
                                    {
                                        Ok(ack) => ack,
                                        Err(error) => {
                                            failure = Some(error.to_string());
                                            break 'subjects;
                                        }
                                    };
                                    checkpoint_progress = reviewed_coverage.acknowledged_progress(
                                        &current,
                                        &value,
                                        &step,
                                        ack.as_ref(),
                                    );
                                    merge(&mut current, &value, state_version);
                                    if !ack.as_ref().is_some_and(|ack| ack.replayed) {
                                        repairs = 0;
                                        refresh_rejections = 0;
                                    }
                                }
                                Err(ClientError::ResearchRefreshRequired(detail)) => {
                                    refresh_rejections += 1;
                                    pending_repair = Some(research::RepairFeedback::new(
                                        research::RepairPhase::CheckpointValidation,
                                        &format!(
                                            "The previous research checkpoint was not saved: {detail}. Reread the refreshed sources and reconsider the rejected conclusions before saving them."
                                        ),
                                    ));
                                    rejected_checkpoint = Some(detail);
                                }
                                Err(error) => {
                                    if let ClientError::ResearchValidation(repair) = &error {
                                        pending_repair = Some(repair.clone());
                                    }
                                    feedback = error.to_string();
                                    repairs += 1;
                                    continue;
                                }
                            }
                            body = envelope(
                                &current,
                                *state_version,
                                &uuid::Uuid::now_v7().to_string(),
                            );
                        }
                        body["queries"] = json!(step.queries);
                        body["targets"] = json!(step.targets);
                        let before = discovery_progress(&current);
                        report.stage = "research_discovery".into();
                        match self
                            .research_request("narrative-discover", body, deadline)
                            .await
                        {
                            Ok(value) => {
                                merge(&mut current, &value, state_version);
                                if discovery_progress(&current) == before && !checkpoint_progress {
                                    no_progress += 1;
                                    feedback = "The last discovery added no new source versions. Inspect its receipt for unavailable/capped targets. Use different supported references, produce the supported overview, or yield with the specific missing evidence.".into();
                                } else {
                                    no_progress = 0;
                                    feedback.clear();
                                }
                                if let Some(detail) = rejected_checkpoint {
                                    feedback = format!(
                                        "The previous research checkpoint was not saved: {detail}. Discovery has refreshed the admitted evidence. Reread the refreshed sources and reconsider the rejected conclusions before saving them. {feedback}"
                                    );
                                }
                            }
                            Err(error) => {
                                feedback = error.to_string();
                                repairs += 1;
                            }
                        }
                    }
                    research::Action::Submit => {
                        report.stage = "research_draft_custody".into();
                        let draft_pointer = match self
                            .retain_research_draft(&mut current, state_version, &step, deadline)
                            .await
                        {
                            Ok(pointer) => pointer,
                            Err(ClientError::ResearchValidation(repair)) => {
                                feedback = format!(
                                    "The draft was not acknowledged and no candidate was submitted: {}",
                                    repair.message
                                );
                                pending_repair = Some(repair);
                                repairs += 1;
                                continue;
                            }
                            Err(error) => {
                                failure = Some(error.to_string());
                                break 'subjects;
                            }
                        };
                        body = envelope(&current, *state_version, &operation_id);
                        add_fields(
                            &mut body,
                            json!({"candidates":step.candidates,
                            "processed_inputs":step.processed_inputs,"findings":step.findings,
                            "research_progress":progress(&step,"waiting",&current)}),
                        );
                        if let Some(pointer) = &draft_pointer {
                            body["draft_protocol"] = json!(research::DRAFT_PROTOCOL);
                            body["draft_pointer"] = json!(pointer);
                        }
                        report.stage = "research_validation".into();
                        match self.research_request("candidates", body, deadline).await {
                            Ok(value) => {
                                let ack = match checkpoint_ack(&current, &operation_id, &value) {
                                    Ok(ack) => ack,
                                    Err(error) => {
                                        failure = Some(error.to_string());
                                        break 'subjects;
                                    }
                                };
                                let count = value["accepted_candidate_ids"]
                                    .as_array()
                                    .map_or(0, Vec::len);
                                if let Some(pointer) = &draft_pointer
                                    && let Err(error) = draft_ack(
                                        &operation_id,
                                        &value,
                                        &pointer.candidate_hash,
                                        Some(pointer),
                                        count > 0,
                                    )
                                {
                                    failure = Some(error.to_string());
                                    break 'subjects;
                                }
                                if let Err(error) =
                                    follow_up_ack(&step, &operation_id, &value, count > 0)
                                {
                                    failure = Some(error.to_string());
                                    break 'subjects;
                                }
                                accepted += count;
                                processed += step.processed_inputs.len();
                                if count > 0 {
                                    report.research["last_review_ref"] =
                                        value["run_entry_ref"].clone();
                                    report.research["last_review_version"] =
                                        value["run_version"].clone();
                                }
                                merge(&mut current, &value, state_version);
                                if count > 0 && ack.as_ref().is_none_or(|ack| ack.subject_complete)
                                {
                                    completed_subjects += 1;
                                } else {
                                    yielded_subjects += 1;
                                }
                                break;
                            }
                            Err(ClientError::ResearchRefreshRequired(detail)) => {
                                refresh_rejections += 1;
                                let message = format!(
                                    "The previous candidate was not accepted: {detail}. Review the refreshed evidence and revise the unaccepted draft before submitting it again."
                                );
                                pending_repair = Some(research::RepairFeedback::new(
                                    research::RepairPhase::CandidateValidation,
                                    &message,
                                ));
                                feedback = message;
                                report.stage = "research_reconciliation".into();
                                match self
                                    .refresh_research_evidence(
                                        &mut current,
                                        state_version,
                                        deadline,
                                    )
                                    .await
                                {
                                    Ok(()) => change_pages += 1,
                                    Err(error) => {
                                        feedback = format!(
                                            "{feedback} Evidence refresh remains unresolved: {error}"
                                        );
                                        repairs += 1;
                                    }
                                }
                            }
                            Err(error) => {
                                if let ClientError::ResearchValidation(repair) = &error {
                                    pending_repair = Some(repair.clone());
                                }
                                feedback = format!(
                                    "Candidate was not accepted: {error}. Correct the same proposal using exact evidence. Request updated or missing sources if needed. Do not discard previously accepted work or mark rejected input processed."
                                );
                                repairs += 1;
                            }
                        }
                    }
                    research::Action::Yield | research::Action::Done => {
                        let status = if step.action == research::Action::Done {
                            "no_change"
                        } else {
                            "waiting"
                        };
                        add_fields(&mut body, progress(&step, status, &current));
                        body["processed_inputs"] = json!(step.processed_inputs);
                        body["findings"] = json!(step.findings);
                        match self
                            .research_request("research-progress", body, deadline)
                            .await
                        {
                            Ok(value) => {
                                if let Err(error) = follow_up_ack(
                                    &step,
                                    &operation_id,
                                    &value,
                                    step.action == research::Action::Done,
                                ) {
                                    failure = Some(error.to_string());
                                    break 'subjects;
                                }
                                let ack = match checkpoint_ack(&current, &operation_id, &value) {
                                    Ok(ack) => ack,
                                    Err(error) => {
                                        failure = Some(error.to_string());
                                        break 'subjects;
                                    }
                                };
                                processed += step.processed_inputs.len();
                                merge(&mut current, &value, state_version);
                                if step.action == research::Action::Done
                                    && ack.as_ref().is_none_or(|ack| ack.subject_complete)
                                {
                                    completed_subjects += 1;
                                } else {
                                    yielded_subjects += 1;
                                }
                                break;
                            }
                            Err(error) => {
                                if let ClientError::ResearchValidation(repair) = &error {
                                    pending_repair = Some(repair.clone());
                                }
                                feedback = error.to_string();
                                repairs += 1;
                            }
                        }
                    }
                }
            }
        }
        if accepted > 0
            && let (Some(reference), Some(version)) = (
                report.research["last_review_ref"].as_str(),
                report.research["last_review_version"].as_i64(),
            )
        {
            let key = format!("dreaming-review-{}", report.attempt_id);
            let retries = report
                .notification
                .get("retry_results")
                .cloned()
                .unwrap_or(json!([]));
            report.notification = match self
                .runner
                .review_ready(&key, reference, version, accepted)
                .await
            {
                Ok(ack) => json!({"status":"accepted","event_key":key,"run_entry_ref":reference,
                        "run_version":version,"count":accepted,"target_kind":"review","ack":ack,"retry_results":retries}),
                Err(_) => json!({"status":"failed","event_key":key,"run_entry_ref":reference,
                        "run_version":version,"count":accepted,"target_kind":"review","retry_results":retries,
                        "detail":"review notification remains retained for retry"}),
            };
        }
        report.research = json!({"rounds":rounds,"change_pages":change_pages,"routed_discoveries":routed_discoveries,"subjects_completed":completed_subjects,
            "subjects_yielded":yielded_subjects,"processed_inputs":processed,"new_review_items":accepted,
            "stop_reason":if failure.is_some(){"operation_failed"}else{&stop}});
        if let Some(detail) = failure {
            return RunOutcome::Partial { detail };
        }
        if quota_limited && accepted == 0 && completed_subjects == 0 {
            return RunOutcome::SkippedLimits;
        }
        if let Some(outcome) = location_outcome
            && !matches!(outcome, RunOutcome::Completed)
        {
            return RunOutcome::Partial {
                detail: format!(
                    "Subject research saved its progress; location phase: {}",
                    outcome_detail(&outcome).unwrap_or_else(|| outcome.label().into())
                ),
            };
        }
        if exhausted && yielded_subjects == 0 {
            RunOutcome::Completed
        } else {
            RunOutcome::Partial {
                detail: format!(
                    "Research saved {completed_subjects} completed subject passes and {yielded_subjects} continuations; {stop}. Remaining work is retained."
                ),
            }
        }
    }
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;

    #[test]
    fn subject_allowance_bounds_each_turn_and_reserves_later_work() {
        let minute = Duration::from_secs(60);
        assert_eq!(subject_allowance(60 * minute), 20 * minute);
        assert_eq!(subject_allowance(40 * minute), 20 * minute);
        assert_eq!(subject_allowance(30 * minute), 15 * minute);
        assert_eq!(subject_allowance(2 * minute), minute);
        assert_eq!(subject_allowance(Duration::ZERO), Duration::ZERO);
    }

    fn selectors(ranges: &[(u64, u64)]) -> Value {
        json!(
            ranges
                .iter()
                .map(
                    |(start, end)| json!({"entry_ref":"entry:source","version":1,
            "start_line":start,"end_line":end})
                )
                .collect::<Vec<_>>()
        )
    }

    #[test]
    fn reviewed_coverage_ignores_reordering_splits_and_alternation() {
        let mut coverage = ReviewedCoverage::default();
        assert!(coverage.include(&selectors(&[(1, 10), (15, 20)])));
        assert!(!coverage.include(&selectors(&[(17, 20), (15, 16), (1, 5), (6, 10)])));
        assert!(coverage.include(&selectors(&[(8, 17)])));
        assert!(!coverage.include(&selectors(&[(1, 20)])));
        assert!(!coverage.include(&selectors(&[(1, 10)])));
        assert!(!coverage.include(&selectors(&[(15, 20)])));
        let mut changed = selectors(&[(1, 10)]);
        changed[0]["version"] = json!(2);
        assert!(coverage.include(&changed));
    }
}
