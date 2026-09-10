//! Real lexical search contracts; only disposable fixture users and indexed text.
use super::*;
use crate::Config;
use sqlx::{PgPool, postgres::PgPoolOptions};

async fn actor(pool: &PgPool, user: Uuid, capabilities: &[&str]) -> AuthContext {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO brunn.api_credentials(id,user_id,label,token_hash,capabilities) VALUES($1,$2,'Dreamer search fixture',$3,$4)")
        .bind(id).bind(user).bind(hash_token(&id.to_string())).bind(capabilities).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO brunn.credential_scope_grants(credential_id,user_id,scope_id) SELECT $1,$2,id FROM brunn.scopes WHERE user_id=$2 AND scope_ref='scope:root'")
        .bind(id).bind(user).execute(pool).await.unwrap();
    AuthContext {
        user_id: UserId(user),
        credential_id: CredentialId(id),
        capabilities: capabilities
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        scope_refs: vec!["scope:root".into()],
        read_only: false,
    }
}

async fn indexed_entry(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    path: &str,
    content: &str,
    metadata: Value,
    chunks: usize,
) -> Uuid {
    let id = Uuid::now_v7();
    let version_id = Uuid::now_v7();
    let hash = hash_token(content);
    sqlx::query("INSERT INTO brunn.entries(id,user_id,path,title,kind,media_type,current_version) VALUES($1,$2,$3,'Search fixture','markdown','text/markdown',1)")
        .bind(id).bind(user).bind(path).execute(&mut **tx).await.unwrap();
    sqlx::query("INSERT INTO brunn.entry_versions(id,entry_id,user_id,version,content_sha256,content,size_bytes,metadata) VALUES($1,$2,$3,1,$4,$5,$6,$7)")
        .bind(version_id).bind(id).bind(user).bind(&hash).bind(content).bind(content.len() as i64).bind(metadata).execute(&mut **tx).await.unwrap();
    let ids = (0..chunks).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
    sqlx::query("INSERT INTO brunn.search_chunks(id,user_id,entry_id,entry_version_id,chunk_index,path,heading,content,token_estimate) SELECT chunk_id,$1,$2,$3,(ordinality-1)::integer,$4,'',$5,8 FROM unnest($6::uuid[]) WITH ORDINALITY AS chunk(chunk_id,ordinality)")
        .bind(user).bind(id).bind(version_id).bind(path).bind(content).bind(ids).execute(&mut **tx).await.unwrap();
    sqlx::query("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) VALUES($1,$2,1,'create',$3,$4)")
        .bind(user).bind(id).bind(path).bind(hash).execute(&mut **tx).await.unwrap();
    id
}

async fn replace_metadata(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    id: Uuid,
    metadata: Value,
) {
    // Current metadata controls admission even while old indexed text and an
    // immutable older version remain available to ordinary workspace search.
    sqlx::query("INSERT INTO brunn.entry_versions(entry_id,user_id,version,content_sha256,content,size_bytes,metadata) SELECT entry_id,user_id,2,content_sha256,content,size_bytes,$3 FROM brunn.entry_versions WHERE user_id=$1 AND entry_id=$2 AND version=1")
        .bind(user).bind(id).bind(metadata).execute(&mut **tx).await.unwrap();
    sqlx::query("UPDATE brunn.entries SET current_version=2 WHERE user_id=$1 AND id=$2")
        .bind(user)
        .bind(id)
        .execute(&mut **tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) SELECT e.user_id,e.id,2,'update',e.path,v.content_sha256 FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=2 WHERE e.user_id=$1 AND e.id=$2")
        .bind(user).bind(id).execute(&mut **tx).await.unwrap();
}

fn headers(mut candidates: Vec<Candidate>, sort: SearchSort) -> Vec<Value> {
    sort_candidates(&mut candidates, sort);
    let mut seen = HashSet::new();
    candidates.into_iter().filter(|candidate| seen.insert(candidate.entry_id)).take(8)
        .map(|candidate| json!({"reference":format!("entry:{}",candidate.entry_id),"path":candidate.path,"version":candidate.version})).collect()
}

#[tokio::test]
async fn dreamer_search_filters_generated_editions_before_all_candidate_caps() {
    let Some(url) = std::env::var("BRUNN_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.is_empty())
    else {
        eprintln!("BRUNN_TEST_DATABASE_URL unset; skipping Dreamer search DB contract");
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let user = Uuid::now_v7();
    let foreign = Uuid::now_v7();
    for id in [user, foreign] {
        sqlx::query("INSERT INTO brunn.users(id,external_ref,display_name) VALUES($1,$2,'Dreamer search fixture')")
            .bind(id).bind(format!("dreamer-search:{id}")).execute(&pool).await.unwrap();
    }
    let runner = actor(&pool, user, &["dreamer:run"]).await;
    let reader = actor(&pool, user, &["query", "read"]).await;
    let mut config = Config::from_env().unwrap();
    let mut role_url = url::Url::parse(&url).unwrap();
    role_url
        .query_pairs_mut()
        .append_pair("options", "-c role=app_ro");
    config.database_url_ro = role_url.to_string();
    role_url.set_query(None);
    role_url
        .query_pairs_mut()
        .append_pair("options", "-c role=app_rw");
    config.database_url_rw = role_url.to_string();
    config.database_url_admin = None;
    config.database_max_connections = 4;
    config.apns_delivery_enabled = false;
    let state = AppState::connect(config).await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    let mut generated = Vec::new();
    for ordinal in 0..140 {
        let metadata = match ordinal % 3 {
            0 => json!({"kind":"briefing_edition"}),
            1 => json!({"kind":"briefing_edition","briefing":{"schema":"briefing.v1"}}),
            _ => json!({}),
        };
        let id = indexed_entry(
            &mut tx,
            user,
            &format!("Briefings/edition-{ordinal}.md"),
            "needle needle needle generated edition",
            metadata,
            32,
        )
        .await;
        if ordinal % 3 == 2 {
            replace_metadata(&mut tx, user, id, json!({"kind":"briefing_edition"})).await;
        }
        generated.push(id);
    }
    assert_eq!(generated.len() * 32, 4480);
    let mut primary = HashSet::new();
    let mut primary_order = Vec::new();
    for ordinal in 0..9 {
        let metadata = match ordinal % 3 {
            0 => json!({}),
            1 => json!({"briefing":{"schema":"briefing.v1"}}),
            _ => json!({"kind":"briefing_edition"}),
        };
        let id = indexed_entry(
            &mut tx,
            user,
            &format!("Briefings/primary-{ordinal}.md"),
            "needle primaryonly",
            metadata,
            1,
        )
        .await;
        if ordinal % 3 == 2 {
            replace_metadata(&mut tx, user, id, json!({"kind":"ordinary_note"})).await;
        }
        primary.insert(id);
        primary_order.push(id);
    }
    // Creation uses wall-clock timestamps, so the primary entries above would
    // otherwise be newer. The ordinary index pool is intentionally unordered:
    // a primary chunk entering that pool must not win the last-modified lane
    // merely because of fixture insertion order. Make the crowding explicit
    // while retaining distinct primary dates for the normal ordering check.
    sqlx::query("UPDATE brunn.entries SET updated_at=CASE WHEN id=ANY($2) THEN '2026-02-01T00:00:00Z'::timestamptz ELSE '2026-01-01T00:00:00Z'::timestamptz + array_position($3::uuid[],id) * interval '1 second' END WHERE user_id=$1")
        .bind(user)
        .bind(&generated)
        .bind(&primary_order)
        .execute(&mut *tx)
        .await
        .unwrap();
    // Exceed the recent-change lane even after generated changes are removed;
    // finding the older primary sources then requires the filtered index pool.
    for ordinal in 0..300 {
        indexed_entry(
            &mut tx,
            user,
            &format!("Other/unrelated-{ordinal}.md"),
            "unrelated background",
            json!({}),
            1,
        )
        .await;
    }
    // More than 128 generated matching identities make the ordinary best-match
    // recent lane dense; more than 256 changes would exhaust an unfiltered lane.
    sqlx::query("INSERT INTO brunn.workspace_changes(user_id,entry_id,entry_version,operation,path,content_sha256) SELECT e.user_id,e.id,e.current_version,'update',e.path,v.content_sha256 FROM generate_series(1,3) repetition CROSS JOIN brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version WHERE e.user_id=$1 AND e.id=ANY($2)")
        .bind(user).bind(&generated).execute(&mut *tx).await.unwrap();
    let foreign_id = indexed_entry(
        &mut tx,
        foreign,
        "Other/foreign.md",
        "needle primaryonly",
        json!({}),
        1,
    )
    .await;
    let deleted = indexed_entry(
        &mut tx,
        user,
        "Other/deleted.md",
        "needle primaryonly",
        json!({}),
        1,
    )
    .await;
    sqlx::query("UPDATE brunn.entries SET deleted_at=clock_timestamp() WHERE id=$1")
        .bind(deleted)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let result = search_headers_for_dreamer(&state, &runner, &["needle".into()])
        .await
        .unwrap();
    assert_eq!(result.len(), 2);
    assert!(
        !runner.can(Capability::Read),
        "internal read authority must stay local"
    );
    for (index, sort) in [SearchSort::BestMatch, SearchSort::LastModified]
        .into_iter()
        .enumerate()
    {
        let mut tx = state.begin_read(&reader).await.unwrap();
        let (control, _) = fetch_lexical_candidates(
            &mut tx,
            "primaryonly",
            "primaryonly",
            sort,
            None,
            false,
            0.0,
            false,
            user,
        )
        .await
        .unwrap();
        let (ordinary, _) = fetch_lexical_candidates(
            &mut tx, "needle", "needle", sort, None, false, 0.0, false, user,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let expected = headers(control, sort);
        assert_eq!(expected.len(), 8);
        assert_eq!(
            result[index]["candidates"],
            json!(expected),
            "eligible primary ordering must retain normal lexical scoring and date semantics"
        );
        for hit in result[index]["candidates"].as_array().unwrap() {
            let id = Uuid::parse_str(
                hit["reference"]
                    .as_str()
                    .unwrap()
                    .strip_prefix("entry:")
                    .unwrap(),
            )
            .unwrap();
            assert!(primary.contains(&id));
            assert_ne!(id, foreign_id);
            assert_ne!(id, deleted);
        }
        assert!(
            ordinary
                .iter()
                .any(|candidate| generated.contains(&candidate.entry_id)),
            "ordinary workspace search must keep generated editions visible"
        );
        let ordinary_ids = ordinary
            .iter()
            .map(|candidate| candidate.entry_id)
            .collect::<HashSet<_>>();
        assert_eq!(
            ordinary_ids.len(),
            64,
            "fixture must saturate the SQL entry cap"
        );
        assert!(
            ordinary_ids.iter().all(|id| generated.contains(id)),
            "ordinary search demonstrates generated candidates crowding out primary sources"
        );
    }
    assert!(
        search_headers_for_dreamer(&state, &reader, &["needle".into()])
            .await
            .is_err()
    );
    sqlx::query("UPDATE brunn.api_credentials SET disabled_at=clock_timestamp() WHERE id=$1")
        .bind(runner.credential_id.0)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        search_headers_for_dreamer(&state, &runner, &["needle".into()])
            .await
            .is_err()
    );
    pool.close().await;
}

struct SearchFixture {
    pool: PgPool,
    state: AppState,
    user: Uuid,
    runner: AuthContext,
    reader: AuthContext,
}

async fn search_fixture() -> Option<SearchFixture> {
    let Some(url) = std::env::var("BRUNN_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.is_empty())
    else {
        eprintln!("BRUNN_TEST_DATABASE_URL unset; skipping Dreamer retrieval parity DB contract");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let user = Uuid::now_v7();
    sqlx::query("INSERT INTO brunn.users(id,external_ref,display_name) VALUES($1,$2,'Dreamer retrieval parity fixture')")
        .bind(user).bind(format!("dreamer-parity:{user}")).execute(&pool).await.unwrap();
    let runner = actor(&pool, user, &["dreamer:run"]).await;
    let reader = actor(&pool, user, &["query", "read"]).await;
    let mut config = Config::from_env().unwrap();
    let mut role_url = url::Url::parse(&url).unwrap();
    role_url
        .query_pairs_mut()
        .append_pair("options", "-c role=app_ro");
    config.database_url_ro = role_url.to_string();
    role_url.set_query(None);
    role_url
        .query_pairs_mut()
        .append_pair("options", "-c role=app_rw");
    config.database_url_rw = role_url.to_string();
    config.database_url_admin = None;
    config.database_max_connections = 4;
    config.apns_delivery_enabled = false;
    config.supersession_demotion = false;
    let state = AppState::connect(config).await.unwrap();
    Some(SearchFixture {
        pool,
        state,
        user,
        runner,
        reader,
    })
}

async fn titled_entry(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    path: &str,
    title: &str,
    content: &str,
    chunks: usize,
) -> Uuid {
    let id = indexed_entry(tx, user, path, content, json!({}), chunks).await;
    // The current title participates in normal lexical scoring. Keeping it
    // outside search_chunks makes the matching lane independent of that bonus.
    sqlx::query("UPDATE brunn.entries SET title=$3 WHERE user_id=$1 AND id=$2")
        .bind(user)
        .bind(id)
        .bind(title)
        .execute(&mut **tx)
        .await
        .unwrap();
    id
}

fn header_ids(result: &Value) -> Vec<Uuid> {
    result["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            Uuid::parse_str(
                item["reference"]
                    .as_str()
                    .unwrap()
                    .strip_prefix("entry:")
                    .unwrap(),
            )
            .unwrap()
        })
        .collect()
}

#[tokio::test]
async fn dreamer_search_natural_language_unions_late_fallbacks_and_keeps_original_query_order() {
    let Some(f) = search_fixture().await else {
        return;
    };
    let query = "Please inspect photometry calibration observatory detector schedule and explain what the current request should report.";
    assert!(search_anchors(query).is_empty());
    let fallbacks = bounded_lexical_fallback_queries(query);
    assert_eq!(
        fallbacks,
        [
            "calibration observatory",
            "photometry calibration",
            "observatory detector",
            "detector schedule"
        ]
    );
    let mut tx = f.pool.begin().await.unwrap();
    let incidental = titled_entry(
        &mut tx,
        f.user,
        "sources/Optics/Incidental.md",
        "Calibration observatory",
        "calibration observatory administrative boilerplate",
        1,
    )
    .await;
    let primary = titled_entry(
        &mut tx,
        f.user,
        "sources/Optics/Primary.md",
        "Photometry calibration observatory detector schedule",
        "detector schedule records the exposure interval",
        1,
    )
    .await;
    let shared = titled_entry(
        &mut tx,
        f.user,
        "sources/Optics/Shared.md",
        "Photometry calibration",
        "photometry calibration observatory detector schedule",
        3,
    )
    .await;
    let boilerplate = titled_entry(
        &mut tx,
        f.user,
        "sources/Optics/Boilerplate.md",
        "Generic instruction template",
        query,
        1,
    )
    .await;
    sqlx::query("UPDATE brunn.entries AS entry SET updated_at=TIMESTAMPTZ '2020-01-01 00:00:00+00'+make_interval(hours=>ordered.ordinality::integer) FROM unnest($2::uuid[]) WITH ORDINALITY AS ordered(id,ordinality) WHERE entry.user_id=$1 AND entry.id=ordered.id")
        .bind(f.user).bind(vec![primary, incidental, shared, boilerplate]).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = f.state.begin_read(&f.reader).await.unwrap();
    let (strict, _) = fetch_lexical_candidates(
        &mut tx,
        query,
        query,
        SearchSort::BestMatch,
        None,
        false,
        0.0,
        false,
        f.user,
    )
    .await
    .unwrap();
    assert!(
        strict.iter().any(|hit| hit.entry_id == boilerplate),
        "the original full query really has an unrelated hit"
    );
    assert!(
        strict.iter().all(|hit| hit.entry_id != primary),
        "strict AND on wrapper filler misses the actual primary record"
    );
    for (index, focused) in fallbacks.iter().enumerate() {
        let (hits, _) = fetch_lexical_candidates(
            &mut tx,
            focused,
            query,
            SearchSort::BestMatch,
            None,
            false,
            0.0,
            false,
            f.user,
        )
        .await
        .unwrap();
        assert!(
            !hits.is_empty(),
            "each selective pair must exercise a real index fetch: {focused}"
        );
        assert_eq!(
            hits.iter().any(|hit| hit.entry_id == primary),
            index == 3,
            "the primary body is reachable only through the fourth pair"
        );
        if index == 0 {
            assert!(hits.iter().any(|hit| hit.entry_id == incidental));
        }
    }
    tx.commit().await.unwrap();

    let result = search_headers_for_dreamer(&f.state, &f.runner, &[query.into()])
        .await
        .unwrap();
    assert_eq!(result.len(), 2);
    for (index, sort) in [SearchSort::BestMatch, SearchSort::LastModified]
        .into_iter()
        .enumerate()
    {
        let (normal, _) = lexical_candidates(&f.state, &f.reader, query, sort, None)
            .await
            .unwrap();
        assert_eq!(
            result[index]["candidates"],
            json!(headers(normal, sort)),
            "Dreamer must use ordinary lexical scoring and merge order for {sort:?}"
        );
        let ids = header_ids(&result[index]);
        assert_eq!(ids.len(), 4);
        assert_eq!(
            ids.iter().filter(|id| **id == shared).count(),
            1,
            "repeated sections and overlapping pairs merge by entry identity"
        );
        assert!(
            ids.contains(&primary),
            "an early incidental hit cannot suppress later primary recall"
        );
        assert_eq!(ids.iter().copied().collect::<HashSet<_>>().len(), ids.len());
    }
    assert_eq!(
        header_ids(&result[0])[0],
        primary,
        "the complete original query title bonus outranks first-pair boilerplate"
    );
    assert_eq!(
        header_ids(&result[1]),
        vec![boilerplate, shared, incidental, primary],
        "explicit fixture timestamps make date ordering independent of score ties"
    );
    assert_eq!(
        search_headers_for_dreamer(&f.state, &f.runner, &[query.into()])
            .await
            .unwrap(),
        result,
        "merging repeated entry IDs retains deterministic header order"
    );
    assert!(!f.runner.can(Capability::Read));
    f.pool.close().await;
}

#[tokio::test]
async fn dreamer_search_excluded_anchors_do_not_block_fallback_or_consume_header_slots() {
    let Some(f) = search_fixture().await else {
        return;
    };
    let query = "Please inspect \"polar refractor\" sample_case_27 photometry calibration observatory detector schedule and report what is current.";
    assert_eq!(search_anchors(query), ["polar refractor", "sample_case_27"]);
    let fallbacks = bounded_lexical_fallback_queries(query);
    assert!(
        fallbacks.contains(&"calibration observatory".to_owned()),
        "{fallbacks:?}"
    );
    let mut tx = f.pool.begin().await.unwrap();
    let primary = indexed_entry(
        &mut tx,
        f.user,
        "sources/Optics/Fallback.md",
        "calibration observatory primary evidence",
        json!({}),
        1,
    )
    .await;
    let mut excluded = HashSet::new();
    for path in [
        "sources/Credentials/Anchor.md",
        "sources/Passwords/Anchor.md",
        "dreams/research/anchor.md",
        "derived/entities/anchor.md",
        "Location/Places.md",
        "Location/Visits/anchor.md",
        "memory/evidence/anchor.md",
        "artifacts/anchor.md",
        "Evidence/Location/anchor.md",
        "private/dreamer.md",
        "agent-memory/anchor.md",
    ] {
        assert!(crate::dreamer_review::research_source_excluded(
            path,
            &Value::Null
        ));
        excluded.insert(
            indexed_entry(
                &mut tx,
                f.user,
                path,
                "polar refractor sample_case_27 calibration observatory",
                json!({}),
                2,
            )
            .await,
        );
    }
    excluded.insert(
        indexed_entry(
            &mut tx,
            f.user,
            "Briefings/AnchorEdition.md",
            "polar refractor sample_case_27 calibration observatory",
            json!({"kind":"briefing_edition"}),
            4,
        )
        .await,
    );
    tx.commit().await.unwrap();

    for single_scan in [false, true] {
        let mut state = f.state.clone();
        state.config.lexical_single_scan = single_scan;
        let result = search_headers_for_dreamer(&state, &f.runner, &[query.into()])
            .await
            .unwrap();
        for group in &result {
            assert_eq!(
                header_ids(group),
                vec![primary],
                "excluded anchor hits must not count as successful discovery: lexical_single_scan={single_scan}"
            );
        }
    }

    let mut tx = f.pool.begin().await.unwrap();
    let quoted = indexed_entry(
        &mut tx,
        f.user,
        "sources/Optics/Quoted.md",
        "polar refractor is the recorded instrument",
        json!({}),
        2,
    )
    .await;
    let identifier = indexed_entry(
        &mut tx,
        f.user,
        "sources/Optics/Identifier.md",
        "sample_case_27 records the second instrument",
        json!({}),
        2,
    )
    .await;
    tx.commit().await.unwrap();
    for single_scan in [false, true] {
        let mut state = f.state.clone();
        state.config.lexical_single_scan = single_scan;
        let result = search_headers_for_dreamer(&state, &f.runner, &[query.into()])
            .await
            .unwrap();
        for group in &result {
            let ids = header_ids(group);
            assert_eq!(ids.len(), 2);
            assert_eq!(
                ids.iter().copied().collect::<HashSet<_>>(),
                HashSet::from([quoted, identifier]),
                "both explicit anchors are retained before fallback: lexical_single_scan={single_scan}"
            );
            assert!(ids.iter().all(|id| !excluded.contains(id)));
            assert!(
                !ids.contains(&primary),
                "eligible anchors retain ordinary lexical stop behavior"
            );
        }
    }
    f.pool.close().await;
}

#[tokio::test]
async fn dreamer_search_six_queries_exercise_every_fallback_within_client_deadline() {
    let Some(mut f) = search_fixture().await else {
        return;
    };
    // Two missing wrapper-only anchors exercise the maximum six bounded
    // fetches per query/sort; their words are absent from the fallback terms.
    f.state.config.lexical_single_scan = false;
    let subjects = [
        "photometry calibration observatory detector schedule",
        "spectroscopy diffraction laboratory crystal rotation",
        "cartography projection coastline compass bearing",
        "hydrology reservoir catchment sediment sampling",
        "acoustics resonance auditorium membrane vibration",
        "botanical germination greenhouse substrate moisture",
    ];
    let queries = subjects
        .iter()
        .map(|subject| {
            format!("\"please inspect\" \"explain request\" {subject} and report what is current.")
        })
        .collect::<Vec<_>>();
    let mut expected = Vec::new();
    let mut tx = f.pool.begin().await.unwrap();
    for (index, query) in queries.iter().enumerate() {
        assert_eq!(search_anchors(query), ["please inspect", "explain request"]);
        let fallbacks = bounded_lexical_fallback_queries(query);
        assert_eq!(fallbacks.len(), 4, "{query}: {fallbacks:?}");
        let mut ids = HashSet::new();
        for (pair, focused) in fallbacks.iter().enumerate() {
            ids.insert(
                indexed_entry(
                    &mut tx,
                    f.user,
                    &format!("sources/Batch/Subject{index}/Pair{pair}.md"),
                    focused,
                    json!({}),
                    3,
                )
                .await,
            );
        }
        // Generated anchor hits are filtered by every SQL fetch before they
        // can suppress the four independent primary fallback lanes.
        indexed_entry(
            &mut tx,
            f.user,
            &format!("Briefings/BatchEdition{index}.md"),
            query,
            json!({"kind":"briefing_edition"}),
            16,
        )
        .await;
        expected.push(ids);
    }
    tx.commit().await.unwrap();

    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        search_headers_for_dreamer(&f.state, &f.runner, &queries),
    )
    .await
    .expect("six-query Dreamer search must fit the 30-second client deadline")
    .unwrap();
    let elapsed = started.elapsed();
    eprintln!(
        "Dreamer six-query retrieval: {:?}; 12 query/sort groups, 72 bounded fetches",
        elapsed
    );
    assert!(elapsed < std::time::Duration::from_secs(30));
    assert_eq!(result.len(), 12);
    for (index, expected_ids) in expected.iter().enumerate() {
        for sort in 0..2 {
            let ids = header_ids(&result[index * 2 + sort]);
            assert_eq!(
                ids.len(),
                4,
                "all four independently indexed primary pairs survive the union"
            );
            assert_eq!(ids.into_iter().collect::<HashSet<_>>(), *expected_ids);
        }
    }
    // Inspect the individual lanes after timing, so these oracle queries do
    // not warm every search before the measured helper invocation.
    let mut tx = f.state.begin_read(&f.reader).await.unwrap();
    for query in &queries {
        for sort in [SearchSort::BestMatch, SearchSort::LastModified] {
            for focused in bounded_lexical_fallback_queries(query) {
                let rows = sqlx::query(DREAMER_LEXICAL_CANDIDATES_SQL)
                    .bind(&focused)
                    .bind(sort.as_str())
                    .bind(f.reader.user_id.0)
                    .fetch_all(&mut *tx)
                    .await
                    .unwrap();
                assert!(
                    !rows.is_empty(),
                    "fixture must exercise this fallback pair in both sorts: {focused}"
                );
            }
        }
    }
    tx.commit().await.unwrap();
    let mut excessive = queries;
    excessive.push("an extra synthetic query".into());
    assert!(
        search_headers_for_dreamer(&f.state, &f.runner, &excessive)
            .await
            .is_err(),
        "the public helper must reject more than six queries"
    );
    f.pool.close().await;
}
