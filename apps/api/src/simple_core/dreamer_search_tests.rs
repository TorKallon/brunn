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
    }
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
