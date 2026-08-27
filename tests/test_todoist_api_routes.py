import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]
API = ROOT / "apps/api/src/api.rs"


class TodoistApiRoutesContractTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.source = API.read_text(encoding="utf-8")

    def test_owner_routes_use_the_exact_methods_and_handlers(self) -> None:
        expected_routes = (
            (
                '"/workspace/integrations/todoist/status",',
                "get(task_service::todoist_status),",
            ),
            (
                '"/workspace/integrations/todoist/config",',
                "put(task_service::configure_todoist),",
            ),
            (
                '"/workspace/integrations/todoist/pull",',
                "post(task_service::pull_todoist),",
            ),
        )
        for path, handler in expected_routes:
            route = f".route(\n            {path}\n            {handler}\n        )"
            self.assertIn(route, self.source)

    def test_routes_are_inside_the_authenticated_workspace_router(self) -> None:
        workspace_start = self.source.index("let workspace_ordinary = Router::new()")
        workspace_end = self.source.index("let legacy_ordinary = Router::new()")
        workspace = self.source[workspace_start:workspace_end]
        for path in (
            "/workspace/integrations/todoist/status",
            "/workspace/integrations/todoist/config",
            "/workspace/integrations/todoist/pull",
        ):
            self.assertIn(path, workspace)

        protected_start = self.source.index("let protected = ordinary")
        protected_end = self.source.index("let web_auth_routes = Router::new()")
        protected = self.source[protected_start:protected_end]
        self.assertIn("auth::middleware", protected)


if __name__ == "__main__":
    unittest.main()
