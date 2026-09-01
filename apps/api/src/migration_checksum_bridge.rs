use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::{
    error::{ApiError, ApiResult},
    migration_checksum_map::HISTORICAL_MIGRATION_CHECKSUMS,
};

const CANONICAL_MAIN_SCHEMA: &str = "brunn";
const CANONICAL_AUTH_SCHEMA: &str = "brunn_auth";
const RETIRED_MAIN_SCHEMA_SEGMENTS: [&str; 2] = ["stray", "light"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChecksumBridgeOutcome {
    LedgerAbsent,
    AlreadyCurrent { checked: usize },
    Reconciled { checked: usize, updated: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AppliedMigration {
    version: i64,
    checksum_sha384: String,
    success: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReconciliationPlan {
    checked: usize,
    update_indexes: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchemaIdentity {
    Retired,
    Canonical,
}

pub(crate) async fn reconcile(pool: &PgPool) -> ApiResult<ChecksumBridgeOutcome> {
    let mut transaction = pool.begin().await?;
    let ledger_exists =
        sqlx::query_scalar::<_, bool>("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
            .fetch_one(&mut *transaction)
            .await?;
    if !ledger_exists {
        transaction.commit().await?;
        return Ok(ChecksumBridgeOutcome::LedgerAbsent);
    }

    sqlx::query("LOCK TABLE public._sqlx_migrations IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *transaction)
        .await?;
    let identity = read_schema_identity(&mut transaction).await?;
    let rows = sqlx::query(
        "SELECT version, encode(checksum, 'hex') AS checksum_sha384, success \
         FROM public._sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut *transaction)
    .await?;
    let applied = rows
        .iter()
        .map(|row| {
            Ok(AppliedMigration {
                version: row.try_get("version")?,
                checksum_sha384: row.try_get("checksum_sha384")?,
                success: row.try_get("success")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let plan = plan_reconciliation(&applied).map_err(ApiError::configuration)?;

    if !plan.update_indexes.is_empty() && identity != SchemaIdentity::Retired {
        return Err(ApiError::configuration(
            "historical migration checksums require reconciliation, but the database schemas are not in the retired identity",
        ));
    }

    for index in &plan.update_indexes {
        let checksum = &HISTORICAL_MIGRATION_CHECKSUMS[*index];
        let result = sqlx::query(
            "UPDATE public._sqlx_migrations \
             SET checksum=decode($2, 'hex') \
             WHERE version=$1 AND checksum=decode($3, 'hex')",
        )
        .bind(checksum.version)
        .bind(checksum.current_sha384)
        .bind(checksum.previous_sha384)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            return Err(ApiError::configuration(format!(
                "migration checksum reconciliation lost its exact match for version {}",
                checksum.version
            )));
        }
    }

    let outcome = if plan.update_indexes.is_empty() {
        ChecksumBridgeOutcome::AlreadyCurrent {
            checked: plan.checked,
        }
    } else {
        ChecksumBridgeOutcome::Reconciled {
            checked: plan.checked,
            updated: plan.update_indexes.len(),
        }
    };
    transaction.commit().await?;
    Ok(outcome)
}

async fn read_schema_identity(
    transaction: &mut Transaction<'_, Postgres>,
) -> ApiResult<SchemaIdentity> {
    let retired_main = RETIRED_MAIN_SCHEMA_SEGMENTS.concat();
    let retired_auth = format!("{retired_main}_auth");
    let row = sqlx::query(
        "SELECT to_regnamespace($1) IS NOT NULL AS retired_main, \
                to_regnamespace($2) IS NOT NULL AS retired_auth, \
                to_regnamespace($3) IS NOT NULL AS canonical_main, \
                to_regnamespace($4) IS NOT NULL AS canonical_auth",
    )
    .bind(retired_main)
    .bind(retired_auth)
    .bind(CANONICAL_MAIN_SCHEMA)
    .bind(CANONICAL_AUTH_SCHEMA)
    .fetch_one(&mut **transaction)
    .await?;
    classify_schema_identity(
        row.try_get("retired_main")?,
        row.try_get("retired_auth")?,
        row.try_get("canonical_main")?,
        row.try_get("canonical_auth")?,
    )
    .map_err(ApiError::configuration)
}

fn classify_schema_identity(
    retired_main: bool,
    retired_auth: bool,
    canonical_main: bool,
    canonical_auth: bool,
) -> Result<SchemaIdentity, String> {
    match (retired_main, retired_auth, canonical_main, canonical_auth) {
        (true, true, false, false) => Ok(SchemaIdentity::Retired),
        (false, false, true, true) => Ok(SchemaIdentity::Canonical),
        _ => Err(
            "database schemas are missing, duplicated, or in an unexpected partial rename state"
                .to_owned(),
        ),
    }
}

fn plan_reconciliation(applied: &[AppliedMigration]) -> Result<ReconciliationPlan, String> {
    if let Some(failed) = applied.iter().find(|migration| !migration.success) {
        return Err(format!(
            "migration ledger contains an unsuccessful row at version {}",
            failed.version
        ));
    }

    let historical = applied
        .iter()
        .take_while(|migration| {
            migration.version
                <= HISTORICAL_MIGRATION_CHECKSUMS
                    .last()
                    .expect("checksum map is non-empty")
                    .version
        })
        .collect::<Vec<_>>();
    for (index, migration) in historical.iter().enumerate() {
        let expected = &HISTORICAL_MIGRATION_CHECKSUMS[index];
        if migration.version != expected.version {
            return Err(format!(
                "migration ledger is not an exact historical prefix at version {}",
                migration.version
            ));
        }
    }
    if applied.get(historical.len()).is_some()
        && historical.len() != HISTORICAL_MIGRATION_CHECKSUMS.len()
    {
        return Err(
            "migration ledger contains a later version before the historical prefix is complete"
                .to_owned(),
        );
    }

    let mut previous_seen = false;
    let mut current_seen = false;
    let mut update_indexes = Vec::new();
    for (index, migration) in historical.iter().enumerate() {
        let expected = &HISTORICAL_MIGRATION_CHECKSUMS[index];
        if expected.previous_sha384 == expected.current_sha384 {
            if migration.checksum_sha384 != expected.current_sha384 {
                return Err(format!(
                    "migration checksum mismatch at unchanged version {}",
                    migration.version
                ));
            }
            continue;
        }
        if migration.checksum_sha384 == expected.previous_sha384 {
            previous_seen = true;
            update_indexes.push(index);
        } else if migration.checksum_sha384 == expected.current_sha384 {
            current_seen = true;
        } else {
            return Err(format!(
                "migration checksum mismatch at version {}",
                migration.version
            ));
        }
    }
    if previous_seen && current_seen {
        return Err(
            "migration ledger contains an unexpected partial checksum reconciliation state"
                .to_owned(),
        );
    }
    Ok(ReconciliationPlan {
        checked: historical.len(),
        update_indexes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn applied(version: i64, checksum_sha384: &str) -> AppliedMigration {
        AppliedMigration {
            version,
            checksum_sha384: checksum_sha384.to_owned(),
            success: true,
        }
    }

    fn previous_prefix(count: usize) -> Vec<AppliedMigration> {
        HISTORICAL_MIGRATION_CHECKSUMS[..count]
            .iter()
            .map(|migration| applied(migration.version, migration.previous_sha384))
            .collect()
    }

    fn current_prefix(count: usize) -> Vec<AppliedMigration> {
        HISTORICAL_MIGRATION_CHECKSUMS[..count]
            .iter()
            .map(|migration| applied(migration.version, migration.current_sha384))
            .collect()
    }

    #[test]
    fn checksum_map_is_complete_ordered_and_exactly_eighty_one_changed_files() {
        assert_eq!(HISTORICAL_MIGRATION_CHECKSUMS.len(), 82);
        assert_eq!(
            HISTORICAL_MIGRATION_CHECKSUMS
                .iter()
                .filter(|migration| migration.previous_sha384 != migration.current_sha384)
                .count(),
            81
        );
        for (index, migration) in HISTORICAL_MIGRATION_CHECKSUMS.iter().enumerate() {
            assert_eq!(migration.version, i64::try_from(index + 1).unwrap());
            for checksum in [migration.previous_sha384, migration.current_sha384] {
                assert_eq!(checksum.len(), 96);
                assert!(checksum.bytes().all(|byte| byte.is_ascii_hexdigit()));
            }
        }
    }

    #[test]
    fn fresh_current_and_replayed_ledgers_need_no_updates() {
        assert_eq!(
            plan_reconciliation(&[]).unwrap(),
            ReconciliationPlan {
                checked: 0,
                update_indexes: vec![],
            }
        );
        let current = current_prefix(HISTORICAL_MIGRATION_CHECKSUMS.len());
        let plan = plan_reconciliation(&current).unwrap();
        assert_eq!(plan.checked, 82);
        assert!(plan.update_indexes.is_empty());

        let mut replay = current;
        replay.push(applied(83, &"a".repeat(96)));
        let plan = plan_reconciliation(&replay).unwrap();
        assert_eq!(plan.checked, 82);
        assert!(plan.update_indexes.is_empty());
    }

    #[test]
    fn full_and_partial_retired_ledgers_update_only_exact_applied_rows() {
        let full = plan_reconciliation(&previous_prefix(82)).unwrap();
        assert_eq!(full.checked, 82);
        assert_eq!(full.update_indexes.len(), 81);

        let partial = plan_reconciliation(&previous_prefix(40)).unwrap();
        assert_eq!(partial.checked, 40);
        assert_eq!(partial.update_indexes.len(), 39);
        assert!(partial.update_indexes.iter().all(|index| *index < 40));
    }

    #[test]
    fn mismatch_mixture_failure_and_gaps_abort_without_a_plan() {
        let mut mismatch = previous_prefix(82);
        mismatch[4].checksum_sha384 = "0".repeat(96);
        assert!(plan_reconciliation(&mismatch).is_err());

        let mut mixture = previous_prefix(82);
        mixture[0].checksum_sha384 = HISTORICAL_MIGRATION_CHECKSUMS[0].current_sha384.to_owned();
        assert!(plan_reconciliation(&mixture).is_err());

        let mut failed = previous_prefix(82);
        failed[0].success = false;
        assert!(plan_reconciliation(&failed).is_err());

        let mut gap = previous_prefix(82);
        gap.remove(5);
        assert!(plan_reconciliation(&gap).is_err());

        let mut short_with_later = previous_prefix(20);
        short_with_later.push(applied(83, &"b".repeat(96)));
        assert!(plan_reconciliation(&short_with_later).is_err());
    }

    #[test]
    fn schema_identity_accepts_only_complete_retired_or_canonical_pairs() {
        assert_eq!(
            classify_schema_identity(true, true, false, false).unwrap(),
            SchemaIdentity::Retired
        );
        assert_eq!(
            classify_schema_identity(false, false, true, true).unwrap(),
            SchemaIdentity::Canonical
        );
        for invalid in [
            (false, false, false, false),
            (true, false, false, false),
            (true, true, true, true),
            (false, false, true, false),
        ] {
            assert!(classify_schema_identity(invalid.0, invalid.1, invalid.2, invalid.3).is_err());
        }
    }
}
