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

fn progress(step: &research::Step, status: &str) -> Value {
    let mut value = json!({"notes":step.notes,"reviewed_sources":step.reviewed_sources,
        "pending_queries":step.pending_queries,"pending_targets":step.pending_targets,"status":status});
    if let Some(pointer) = &step.covers_existing {
        value["covers_existing"] = pointer.clone();
    }
    if let Some(pointer) = &step.supersedes_existing {
        value["supersedes_existing"] = pointer.clone();
    }
    if let Some(follow_up) = &step.follow_up {
        value["follow_up"] = follow_up.clone();
    }
    value
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

impl Dreamer {
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
            if deadline.saturating_duration_since(tokio::time::Instant::now()) <= selection_margin {
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
            if deadline.saturating_duration_since(tokio::time::Instant::now()) <= selection_margin {
                stop = "time_exhausted".into();
                break;
            }
            // A faulty or old server must not cause an endless same-job loop.
            if !subjects_seen.insert(subject.clone()) {
                failure = Some("research selection repeated a subject already serviced in this attempt; progress retained".into());
                break;
            }
            let mut feedback = String::new();
            let mut repairs = 0usize;
            // Successful discovery cannot erase a rejected checkpoint. Only a
            // subsequently accepted checkpoint resets this part of the budget.
            let mut refresh_rejections = 0usize;
            let mut no_progress = 0usize;
            let mut subject_round = 0usize;
            let mut routed_attempted = std::collections::BTreeSet::new();
            // Leave time for another subject even if this model invocation
            // stalls. Evidence already admitted survives the bounded turn.
            let now = tokio::time::Instant::now();
            let subject_deadline =
                now + Duration::from_secs(600).min(deadline.saturating_duration_since(now) / 2);
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    stop = "time_exhausted".into();
                    break 'subjects;
                }
                // Each subject retains its continuation if it needs more than
                // one run. The next subject still gets an opportunity today.
                if repairs + refresh_rejections >= 2
                    || no_progress >= 2
                    || subject_round >= 32
                    || tokio::time::Instant::now() >= subject_deadline
                {
                    let mut body =
                        envelope(&current, *state_version, &uuid::Uuid::now_v7().to_string());
                    add_fields(
                        &mut body,
                        json!({"status":"waiting",
                        "findings":[if feedback.is_empty(){"Further research remains; this subject yielded so other work can proceed."}else{&feedback}],
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
                rounds += 1;
                report.stage = "subject_research".into();
                let input = research::prompt(&current, &feedback);
                let name = format!("research-{}-{subject_round}-answer.md", subjects_seen.len());
                let round_budget =
                    subject_deadline.saturating_duration_since(tokio::time::Instant::now());
                let result = self
                    .exec_codex(run_home, env, &input, round_budget, &name)
                    .await;
                match result {
                    ExecResult::Finished => {}
                    ExecResult::Failed(detail) if detail.starts_with("plan limits mid-run:") => {
                        quota_limited = true;
                        stop = "account_limits".into();
                        break 'subjects;
                    }
                    ExecResult::TimedOut => {
                        feedback = "The subject's time allowance ended. Its admitted evidence and saved conclusions remain available for another attempt.".into();
                        repairs = 2;
                        continue;
                    }
                    ExecResult::Failed(_) => {
                        feedback = "The previous model process did not return a usable response. Resume from the persisted evidence and return the documented JSON object.".into();
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
                        feedback = error;
                        repairs += 1;
                        continue;
                    }
                };
                let mut body =
                    envelope(&current, *state_version, &uuid::Uuid::now_v7().to_string());
                match step.action {
                    research::Action::Discover => {
                        let mut rejected_checkpoint = None;
                        if !step.reviewed_sources.is_empty() {
                            add_fields(&mut body, progress(&step, "researching"));
                            body["processed_inputs"] = json!([]);
                            body["findings"] = json!(step.findings);
                            match self
                                .research_request("research-progress", body, deadline)
                                .await
                            {
                                Ok(value) => {
                                    merge(&mut current, &value, state_version);
                                    repairs = 0;
                                    refresh_rejections = 0;
                                }
                                Err(ClientError::ResearchRefreshRequired(detail)) => {
                                    refresh_rejections += 1;
                                    rejected_checkpoint = Some(detail);
                                }
                                Err(error) => {
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
                                if discovery_progress(&current) == before {
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
                                repairs = 0;
                            }
                            Err(error) => {
                                feedback = error.to_string();
                                repairs += 1;
                            }
                        }
                    }
                    research::Action::Submit => {
                        add_fields(
                            &mut body,
                            json!({"candidates":step.candidates,
                            "processed_inputs":step.processed_inputs,"findings":step.findings,
                            "research_progress":progress(&step,"waiting")}),
                        );
                        report.stage = "research_validation".into();
                        match self.research_request("candidates", body, deadline).await {
                            Ok(value) => {
                                let count = value["accepted_candidate_ids"]
                                    .as_array()
                                    .map_or(0, Vec::len);
                                accepted += count;
                                processed += step.processed_inputs.len();
                                if count > 0 {
                                    report.research["last_review_ref"] =
                                        value["run_entry_ref"].clone();
                                    report.research["last_review_version"] =
                                        value["run_version"].clone();
                                }
                                merge(&mut current, &value, state_version);
                                completed_subjects += 1;
                                break;
                            }
                            Err(error) => {
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
                        add_fields(&mut body, progress(&step, status));
                        body["processed_inputs"] = json!(step.processed_inputs);
                        body["findings"] = json!(step.findings);
                        match self
                            .research_request("research-progress", body, deadline)
                            .await
                        {
                            Ok(value) => {
                                processed += step.processed_inputs.len();
                                merge(&mut current, &value, state_version);
                                if step.action == research::Action::Done {
                                    completed_subjects += 1;
                                } else {
                                    yielded_subjects += 1;
                                }
                                break;
                            }
                            Err(error) => {
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
