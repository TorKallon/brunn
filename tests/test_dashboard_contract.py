import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MIGRATION = ROOT / "apps/api/migrations/0064_workspace_dashboard_activity.sql"
API = ROOT / "apps/api/src/api.rs"
DASHBOARD = ROOT / "apps/api/src/dashboard_service.rs"
SIMPLE_CORE = ROOT / "apps/api/src/simple_core.rs"
USAGE = ROOT / "apps/api/src/usage.rs"
AUTH = ROOT / "apps/api/src/auth.rs"
BRIEFINGS = ROOT / "apps/api/src/briefing_service.rs"
OBJECT_STORE = ROOT / "apps/api/src/object_store.rs"


class DashboardContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.migration = MIGRATION.read_text(encoding="utf-8")
        cls.api = API.read_text(encoding="utf-8")
        cls.dashboard = DASHBOARD.read_text(encoding="utf-8")
        cls.simple_core = SIMPLE_CORE.read_text(encoding="utf-8")
        cls.usage = USAGE.read_text(encoding="utf-8")
        cls.auth = AUTH.read_text(encoding="utf-8")
        cls.briefings = BRIEFINGS.read_text(encoding="utf-8")
        cls.object_store = OBJECT_STORE.read_text(encoding="utf-8")

    def test_activity_rollup_is_bounded_content_free_and_user_scoped(self) -> None:
        for marker in (
            "CREATE TABLE straylight.product_activity_minutely",
            "PRIMARY KEY (user_id, credential_id, bucket_start, operation)",
            "CHECK (operation IN",
            "CHECK (operation_count >= 0)",
            "CHECK (byte_count >= 0)",
            "ENABLE ROW LEVEL SECURITY",
            "FORCE ROW LEVEL SECURITY",
            "straylight_auth.can_access_user(user_id)",
            "bucket_start + interval '1 minute'",
            "GRANT SELECT ON straylight.product_activity_minutely TO app_rw, app_ro",
            "product_activity_minutely_user_time_idx",
            "product_activity_minutely_credential_recent_idx",
            "CREATE TABLE straylight.credential_activity",
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
            "straylight.web_identities",
            "'web_ui'",
            "manageable",
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
            'const ACTIVITY_COVERAGE: &str = "tracked_operations_only"',
            "product_activity_minutely",
            "physical_usage",
            "product_activity_health",
            "period_start",
            "period_end",
            "read_operations_today",
            "write_operations_today",
        ):
            self.assertIn(marker, self.dashboard)

    def test_tracker_is_bounded_fail_open_and_covers_product_operations(self) -> None:
        for marker in (
            "const ENTRY_CHANNEL_CAPACITY: usize = 4_096",
            "const ACTIVITY_CHANNEL_CAPACITY: usize = 4_096",
            "const MAX_PENDING_KEYS: usize = 5_000",
            "try_send(event)",
            '"product activity batch dropped"',
            "run_entry_usage",
            "run_activity",
            "ProductActivityTrackerStatus::Disabled",
            "ProductActivityTrackerStatus::Degraded",
            "record_credential_activity",
        ):
            haystack = self.usage
            self.assertIn(marker, haystack)
        self.assertNotIn("ProductActivityOperation", self.dashboard)

    def test_briefings_and_successful_control_requests_are_instrumented(self) -> None:
        for marker in (
            "ProductActivityOperation::BriefingList",
            "ProductActivityOperation::BriefingRead",
            "ProductActivityOperation::BriefingTopics",
            "ProductActivityOperation::BriefingPublish",
            "ProductActivityOperation::BriefingAction",
            "if !result.no_op",
        ):
            self.assertIn(marker, self.briefings)
        self.assertIn("response.status().is_success()", self.auth)
        self.assertIn("record_credential_activity", self.auth)

    def test_physical_inventory_is_versioned_cached_and_never_false_zero(self) -> None:
        for marker in (
            "list_object_versions()",
            "physical_object_versions",
            "PhysicalUsageStatus::Stale",
            "PhysicalUsageStatus::Unavailable",
            "PHYSICAL_USAGE_CACHE_TTL",
            "physical_object_versions: None",
            "physical_size_bytes: None",
        ):
            self.assertIn(marker, self.object_store)


if __name__ == "__main__":
    unittest.main()
