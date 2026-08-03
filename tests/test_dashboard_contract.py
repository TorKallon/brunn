import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MIGRATION = ROOT / "apps/api/migrations/0064_workspace_dashboard_activity.sql"
API = ROOT / "apps/api/src/api.rs"
DASHBOARD = ROOT / "apps/api/src/dashboard_service.rs"
SIMPLE_CORE = ROOT / "apps/api/src/simple_core.rs"
USAGE = ROOT / "apps/api/src/usage.rs"


class DashboardContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.migration = MIGRATION.read_text(encoding="utf-8")
        cls.api = API.read_text(encoding="utf-8")
        cls.dashboard = DASHBOARD.read_text(encoding="utf-8")
        cls.simple_core = SIMPLE_CORE.read_text(encoding="utf-8")
        cls.usage = USAGE.read_text(encoding="utf-8")

    def test_activity_rollup_is_bounded_content_free_and_user_scoped(self) -> None:
        for marker in (
            "CREATE TABLE straylight.product_activity_hourly",
            "PRIMARY KEY (user_id, credential_id, bucket_start, operation)",
            "CHECK (operation IN",
            "CHECK (operation_count >= 0)",
            "CHECK (byte_count >= 0)",
            "ENABLE ROW LEVEL SECURITY",
            "FORCE ROW LEVEL SECURITY",
            "straylight_auth.can_access_user(user_id)",
            "GRANT SELECT ON straylight.product_activity_hourly TO app_rw, app_ro",
            "product_activity_hourly_user_time_idx",
            "product_activity_hourly_credential_recent_idx",
        ):
            self.assertIn(marker, self.migration)
        self.assertNotIn("GRANT INSERT", self.migration)
        self.assertNotIn("GRANT UPDATE", self.migration)
        self.assertNotIn("GRANT DELETE", self.migration)

    def test_credential_projection_is_security_definer_and_secret_free(self) -> None:
        start = self.migration.index(
            "CREATE FUNCTION straylight_auth.dashboard_credentials"
        )
        end = self.migration.index(
            "REVOKE ALL ON FUNCTION straylight_auth.dashboard_credentials", start
        )
        function = self.migration[start:end]
        for marker in (
            "SECURITY DEFINER",
            "SET row_security = off",
            "straylight_auth.context_is_valid()",
            "straylight_auth.has_capability('read')",
            "straylight_auth.has_capability('status')",
            "NOT EXISTS",
            "straylight.web_identities",
        ):
            self.assertIn(marker, function)
        self.assertNotIn("token_hash", function)
        self.assertNotIn("token", function)

    def test_dashboard_route_and_contract_are_explicit(self) -> None:
        self.assertIn(
            '.route("/workspace/dashboard", get(dashboard_service::dashboard))',
            self.api,
        )
        for marker in (
            "auth.require(Capability::Read)?",
            "auth.require(Capability::Status)?",
            'const ACTIVITY_DAYS: i64 = 7',
            'const BINARY_STORAGE_SEMANTICS: &str = "current_referenced_objects"',
            'const ACTIVITY_COVERAGE: &str = "tracked_operations_only"',
            "period_start",
            "period_end",
            "read_operations_today",
            "write_operations_today",
        ):
            self.assertIn(marker, self.dashboard)

    def test_tracker_is_bounded_fail_open_and_covers_product_operations(self) -> None:
        for marker in (
            "const CHANNEL_CAPACITY: usize = 4_096",
            "const MAX_PENDING_KEYS: usize = 5_000",
            "try_send(event)",
            '"product activity batch dropped"',
            "ProductActivityOperation::Open",
            "ProductActivityOperation::Search",
            "ProductActivityOperation::Read",
            "ProductActivityOperation::BinaryFetch",
            "ProductActivityOperation::Write",
            "ProductActivityOperation::Capture",
            "ProductActivityOperation::Checkpoint",
            "ProductActivityOperation::BinaryUpload",
            "ProductActivityOperation::Delete",
        ):
            haystack = self.usage if marker.startswith("const ") or "try_send" in marker or "batch dropped" in marker else self.simple_core
            self.assertIn(marker, haystack)
        self.assertNotIn("ProductActivityOperation", self.dashboard)


if __name__ == "__main__":
    unittest.main()
