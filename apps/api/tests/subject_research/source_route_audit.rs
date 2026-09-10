//! Ordinary audit snapshots must not expose server-owned route authority.
use super::*;

const PROTOCOL: &str = "dream.research.follow_up.v1";

struct RoutedAudit {
    origin: Value,
    routed: Value,
    state_version: i64,
}

async fn setup(f: &Fixture) -> RoutedAudit {
    control(f, "report-only", 0).await;
    let destination = write(
        f,
        "sources/Notes/Survey.md",
        "# Survey\n\nThe survey records an observation.\n",
        0,
    )
    .await;
    let selected_source = write(
        f,
        "sources/Notes/Witness.md",
        "# Witness\n\nThe witness retains an independent observation.\n",
        0,
    )
    .await;
    let origin = write(
        f,
        "sources/Notes/Instrument.md",
        "# Instrument\n\nThe equipment qualification is a separate primary observation.\n",
        0,
    )
    .await;
    let mut admitted = ok(post(
        f,
        &f.runner,
        "/v1/workspace/dreamer/admit",
        json!({
            "attempt_id":Uuid::now_v7(),"date":date(),"kind":"manual","lease_seconds":60,
            "requested_subject_refs":[destination["entry_ref"],selected_source["entry_ref"]]
        }),
    )
    .await);
    let (_, consumed) = submit(
        f,
        &admitted,
        admitted["state_version"].as_i64().unwrap(),
        vec![],
    )
    .await;
    admitted["state_version"] = consumed["state_version"].clone();
    let selected = next_subject(f, &admitted).await;
    assert_eq!(
        selected["research"]["subject_ref"],
        destination["entry_ref"]
    );
    let (_, selected) =
        discover_subject(f, &selected, vec![selected_source["entry_ref"].clone()]).await;
    let mut body = research_request(&selected);
    body["candidates"] = json!([{
        "kind":"summary","title":"Survey account","summary":"Two checked observations.",
        "reason":"Preserve an ordinary pending overview.","subject_ref":destination["entry_ref"],
        "path":selected["research"]["output_path"],"expected_version":0,
        "content":"# Survey account\n\nThe survey records an observation.[^s1]\nThe witness retains an independent observation.[^s2]\n",
        "sources":[reviewed(&destination),reviewed(&selected_source)]
    }]);
    body["processed_inputs"] = json!([]);
    body["findings"] = json!(["Both primary observations are included."]);
    let accepted = ok(post(f, &f.runner, "/v1/workspace/dreamer/candidates", body).await);
    let (_, run) = exact_run_record(f, &accepted).await;
    assert!(
        !run.to_string()
            .contains(origin["entry_ref"].as_str().unwrap()),
        "the routed origin is absent from every retained proposal citation and dependency"
    );
    let selected = next_subject(f, &accepted).await;
    assert_eq!(
        selected["research"]["subject_ref"],
        selected_source["entry_ref"]
    );
    let (_, selected) = discover_subject(f, &selected, vec![origin["entry_ref"].clone()]).await;
    let comparison = selected["comparison_proposals"][0]["pointer"].clone();
    assert_eq!(comparison["item_id"], accepted["accepted_candidate_ids"][0]);
    let mut body = progress_body(
        &selected,
        vec![reviewed(&selected_source), reviewed(&origin)],
        "waiting",
        "The separate equipment evidence needs assessment in the existing overview.",
    );
    body["processed_inputs"] = json!([]);
    body["findings"] = json!(["The equipment qualification is useful retained enrichment."]);
    body["follow_up_protocol"] = json!(PROTOCOL);
    body["follow_up"] = json!({"comparison":comparison,
        "origin_source":{"entry_ref":origin["entry_ref"],"version":origin["version"]},"targets":[]});
    let routed = ok(post(
        f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        body,
    )
    .await)["data"]
        .clone();
    assert_eq!(routed["inputs"], json!([]));
    let raw = current(f, "dreams/state.md").await.unwrap();
    assert_eq!(
        raw.2["dreamer_state"]["research"]["follow_ups"][0]["origin_source"]["entry_ref"],
        origin["entry_ref"]
    );
    RoutedAudit {
        origin,
        routed,
        state_version: raw.0,
    }
}

async fn read_audit(f: &Fixture, path: &str, version: Option<i64>) -> Value {
    let mut item = json!({"path":path,"view":"full","max_chars":256_000});
    if let Some(version) = version {
        item["version"] = json!(version);
    }
    let response = ok(post(
        f,
        &f.model,
        "/v1/workspace/read",
        json!({"requests":[item]}),
    )
    .await);
    response["data"]["items"][0].clone()
}

fn assert_scrubbed(item: &Value, container: &str) {
    assert_eq!(item["representation"], "audit_snapshot", "{item}");
    assert!(
        item["metadata"][container].is_object(),
        "a positive metadata assertion prevents a response-budget false pass: {item}"
    );
    assert!(
        item["metadata"][container]["research"]
            .get("follow_ups")
            .is_none()
    );
    assert!(
        item["metadata"][container]["source_dispositions"]
            .as_array()
            .is_none_or(|values| values.iter().all(|value| value.get("route").is_none()))
    );
}

#[tokio::test]
async fn source_routes_are_private_in_current_and_historical_state_after_origin_access_loss() {
    let Some(f) = fixture().await else { return };
    let s = setup(&f).await;
    let id = Uuid::parse_str(
        s.origin["entry_ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("entry:"),
    )
    .unwrap();
    // Only the route's uncited primary loses access. Existing proposal
    // manifests remain readable, so this exercises the ordinary audit render.
    sqlx::query("UPDATE brunn.entries SET path=$3 WHERE user_id=$1 AND id=$2")
        .bind(f.owner.user)
        .bind(id)
        .bind(format!(".brunn/tasks/{}.md", Uuid::now_v7()))
        .execute(&f.pool)
        .await
        .unwrap();
    let mut body = research_request(&s.routed);
    body["status"] = json!("waiting");
    body["processed_inputs"] = json!([]);
    ok(post(
        &f,
        &f.runner,
        "/v1/workspace/dreamer/research-progress",
        body,
    )
    .await);
    assert!(current(&f, "dreams/state.md").await.unwrap().0 > s.state_version);
    for version in [None, Some(s.state_version)] {
        let item = read_audit(&f, "dreams/state.md", version).await;
        assert_scrubbed(&item, "dreamer_state");
        assert!(item["text"].as_str().unwrap().contains("Dreamer progress"));
        assert!(
            !item
                .to_string()
                .contains(s.origin["entry_ref"].as_str().unwrap())
        );
        assert!(!item.to_string().contains("Instrument"));
    }
    assert_eq!(
        current(&f, "dreams/state.md").await.unwrap().2["dreamer_state"]["research"]["follow_ups"]
            [0]["origin_source"]["entry_ref"],
        s.origin["entry_ref"],
        "projection must not destroy retained route authority"
    );
}

#[tokio::test]
async fn source_route_dispositions_stay_private_in_state_and_run_audits() {
    let Some(f) = fixture().await else { return };
    let s = setup(&f).await;
    finish(
        &f,
        &s.routed,
        s.routed["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    ok(request(
        &f.app,
        &f.owner,
        Method::DELETE,
        &format!(
            "/v1/workspace/entries/{}?expected_version=1",
            s.origin["entry_ref"].as_str().unwrap()
        ),
        None,
    )
    .await);
    let admitted = admit(&f).await;
    let raw = current(&f, "dreams/state.md").await.unwrap();
    let dispositions = raw.2["dreamer_state"]["source_dispositions"]
        .as_array()
        .unwrap();
    let route_disposition = dispositions
        .iter()
        .find(|item| item["disposition"] == "excluded_by_source_policy")
        .unwrap();
    assert_eq!(
        route_disposition["route"]["origin_source"]["entry_ref"],
        s.origin["entry_ref"]
    );
    let ordinary: Vec<_> = dispositions
        .iter()
        .filter(|item| item.get("route").is_none())
        .cloned()
        .collect();
    assert!(
        ordinary
            .iter()
            .any(|item| item["disposition"] == "deleted_source"),
        "the unrelated deterministic intake disposition is retained"
    );
    let digest = hex::encode(Sha256::digest(
        serde_json::to_vec(route_disposition).unwrap(),
    ));
    let audit_path = format!("dreams/reviews/research-exclusion-{digest}.md");
    let audit = current(&f, &audit_path).await.unwrap();
    assert_eq!(audit.2["dreamer_review"]["disposition"], *route_disposition);
    for version in [None, Some(audit.0)] {
        let item = read_audit(&f, &audit_path, version).await;
        assert_eq!(
            item["representation"], "audit_withheld",
            "standalone route audits have no proven item manifest"
        );
        assert!(
            !item
                .to_string()
                .contains(s.origin["entry_ref"].as_str().unwrap())
        );
        assert!(item.get("metadata").is_none());
    }
    let (_, completed) = finish(
        &f,
        &admitted,
        admitted["state_version"].as_i64().unwrap(),
        "partial",
    )
    .await;
    let (_, run) = exact_run_record(&f, &completed).await;
    let run_path: String =
        sqlx::query_scalar("SELECT path FROM brunn.entries WHERE user_id=$1 AND id=$2")
            .bind(f.owner.user)
            .bind(
                Uuid::parse_str(
                    completed["run_entry_ref"]
                        .as_str()
                        .unwrap()
                        .trim_start_matches("entry:"),
                )
                .unwrap(),
            )
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(
        run["dreamer_run"]["source_dispositions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.get("route").is_some())
    );
    for (path, container, version) in [
        ("dreams/state.md", "dreamer_state", raw.0),
        (
            run_path.as_str(),
            "dreamer_run",
            completed["run_version"].as_i64().unwrap(),
        ),
    ] {
        for exact in [None, Some(version)] {
            let item = read_audit(&f, path, exact).await;
            assert_scrubbed(&item, container);
            assert_eq!(
                item["metadata"][container]["source_dispositions"],
                json!(ordinary)
            );
        }
    }
    assert_eq!(
        current(&f, &audit_path).await.unwrap(),
        audit,
        "ordinary reads preserve the complete immutable server disposition"
    );
}
