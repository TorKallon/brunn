from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[1]
CLIENT = ROOT / "apps" / "api" / "src" / "todoist_sync.rs"
WORKER = ROOT / "apps" / "api" / "src" / "worker.rs"


class TodoistReadOnlyContractTests(unittest.TestCase):
    def source(self) -> str:
        self.assertTrue(CLIENT.is_file(), "the Todoist sync client must be isolated")
        return CLIENT.read_text(encoding="utf-8")

    def test_client_is_pinned_to_the_unified_v1_sync_endpoint(self) -> None:
        source = self.source()
        self.assertIn("https://api.todoist.com/api/v1/sync", source)
        self.assertIn(
            "https://api.todoist.com/api/v1/tasks/completed/by_completion_date",
            source,
        )
        self.assertIn('resource_types', source)
        self.assertIn('sync_token', source)

    def test_client_has_no_todoist_mutation_surface(self) -> None:
        source = self.source()
        forbidden = (
            '"commands"',
            "/projects/",
            "close_task",
            "reopen_task",
            "delete_task",
            "create_task",
            "update_task",
        )
        for fragment in forbidden:
            self.assertNotIn(fragment, source)
        self.assertNotIn(".put(", source)
        self.assertNotIn(".patch(", source)
        self.assertNotIn(".delete(", source)
        self.assertEqual(source.count(".post(self.sync_url.clone())"), 1)
        self.assertEqual(source.count(".get(url)"), 1)
        self.assertIn('Ok("production")', source)
        self.assertIn("uncredentialed loopback HTTP origin", source)
        client_impl = source.split("impl TodoistClient {", 1)[1].split("\n}", 1)[0]
        public_async_methods = re.findall(
            r"pub(?:\(crate\))?\s+async\s+fn\s+([a-zA-Z0-9_]+)", client_impl
        )
        self.assertEqual(
            public_async_methods, ["sync", "completed_by_completion_date"]
        )

    def test_token_cannot_be_formatted_or_serialized(self) -> None:
        source = self.source()
        self.assertNotRegex(source, r"impl\s+(?:std::fmt::)?Display\s+for\s+TodoistToken")
        self.assertNotRegex(source, r"(?:derive\([^)]*Serialize[^)]*\)|impl\s+Serialize)\s+for?\s*TodoistToken")
        self.assertIn('write!(formatter, "[REDACTED]")', source)

    def test_content_bearing_apply_errors_cannot_reach_worker_logs(self) -> None:
        source = self.source()
        apply_failure = source.split(
            "if task_service::apply_todoist_sync_in_tx(", 1
        )[1].split("if let Err(error) = finish_sync_success_in_tx(", 1)[0]
        self.assertIn('TodoistClientError::new("todoist_apply_rejected")', apply_failure)
        self.assertIn("return Ok(true);", apply_failure)
        self.assertNotIn("return Err(error);", apply_failure)

        worker = WORKER.read_text(encoding="utf-8")
        boundary = worker.split("async fn run_todoist_sync(", 1)[1].split(
            "async fn run_task_guard(", 1
        )[0]
        self.assertIn('tracing::warn!("Todoist sync cycle failed")', boundary)
        self.assertNotIn("?error", boundary)


if __name__ == "__main__":
    unittest.main()
