from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]
MIGRATION = ROOT / "apps" / "api" / "migrations" / "0074_todoist_pull.sql"


class TodoistMigrationContractTests(unittest.TestCase):
    def source(self) -> str:
        self.assertTrue(MIGRATION.is_file())
        return MIGRATION.read_text(encoding="utf-8")

    def test_worker_secret_read_is_narrow_and_not_public(self) -> None:
        source = self.source()
        self.assertIn("task_todoist_secret_for_worker", source)
        self.assertIn("'todoist-api-token'", source)
        self.assertIn("secret_access_log", source)
        self.assertIn("'get'", source)
        self.assertIn(
            "REVOKE ALL ON FUNCTION straylight.task_todoist_secret_for_worker(uuid)",
            source,
        )
        self.assertNotIn(
            "GRANT EXECUTE ON FUNCTION straylight.task_todoist_secret_for_worker",
            source,
        )

    def test_internal_producer_is_hidden_and_narrow(self) -> None:
        source = self.source()
        self.assertIn("task_todoist_producers", source)
        self.assertIn("ARRAY['task.read','task.write']::text[]", source)
        self.assertIn("NOT EXISTS (\n      SELECT 1 FROM straylight.task_todoist_producers", source)

    def test_sync_state_has_a_durable_scheduler_lease(self) -> None:
        source = self.source()
        self.assertIn("lease_owner", source)
        self.assertIn("lease_expires_at", source)
        self.assertIn("task_todoist_sync_due_idx", source)
        self.assertIn("users_seed_todoist_sync_state", source)

    def test_recurrence_and_project_mapping_have_durable_identity(self) -> None:
        source = self.source()
        self.assertIn("task_todoist_projects", source)
        self.assertIn("task_todoist_occurrences", source)
        self.assertIn("PRIMARY KEY (user_id,series_id,occurrence_key)", source)
        self.assertIn("'todoist-inbox'", source)

    def test_configuration_mutations_require_an_owner_web_identity(self) -> None:
        source = self.source()
        self.assertIn("require_todoist_web_owner", source)
        self.assertIn("web_identities", source)
        self.assertIn("current_credential_id()", source)
        self.assertIn(
            "GRANT EXECUTE ON FUNCTION straylight.require_todoist_web_owner(uuid) TO app_rw",
            source,
        )


if __name__ == "__main__":
    unittest.main()
