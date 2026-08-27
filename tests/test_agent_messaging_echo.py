from __future__ import annotations

import importlib.util
import io
import json
import stat
import sys
import tempfile
import threading
import unittest
from collections import deque
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Iterator


SCRIPT_PATH = Path(__file__).resolve().parents[1] / "scripts/agent_messaging_echo.py"
SPEC = importlib.util.spec_from_file_location("agent_messaging_echo", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("could not load agent messaging echo resident")
echo = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = echo
SPEC.loader.exec_module(echo)


TOKEN = "sl_echo_test_token_that_must_never_be_logged"
CONVERSATION_ID = "019f8800-0000-7000-8000-000000000001"
CLIENT_KEY = "01ARZ3NDEKTSV4RRFFQ69G5FAV"


class FakeMessagingServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self) -> None:
        super().__init__(("127.0.0.1", 0), FakeHandler)
        self.get_responses: deque[tuple[int, dict[str, Any]]] = deque()
        self.post_responses: deque[tuple[int, dict[str, Any]]] = deque()
        self.calls: list[dict[str, Any]] = []
        self.lock = threading.Lock()

    @property
    def base_url(self) -> str:
        host, port = self.server_address
        return f"http://{host}:{port}"


class FakeHandler(BaseHTTPRequestHandler):
    server: FakeMessagingServer

    def log_message(self, _format: str, *_args: object) -> None:
        return

    def do_GET(self) -> None:  # noqa: N802 - stdlib handler API
        with self.server.lock:
            self.server.calls.append(
                {
                    "method": "GET",
                    "path": self.path,
                    "authorization": self.headers.get("authorization"),
                    "body": None,
                }
            )
            response = self.server.get_responses.popleft()
        self._respond(*response)

    def do_POST(self) -> None:  # noqa: N802 - stdlib handler API
        length = int(self.headers.get("content-length", "0"))
        body = json.loads(self.rfile.read(length))
        with self.server.lock:
            self.server.calls.append(
                {
                    "method": "POST",
                    "path": self.path,
                    "authorization": self.headers.get("authorization"),
                    "body": body,
                }
            )
            response = self.server.post_responses.popleft()
        self._respond(*response)

    def _respond(self, status: int, body: dict[str, Any]) -> None:
        encoded = json.dumps(body, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)


@contextmanager
def fake_server() -> Iterator[FakeMessagingServer]:
    server = FakeMessagingServer()
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield server
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


def sync_response(cursor: int, messages: list[dict[str, Any]]) -> dict[str, Any]:
    return {
        "status": "complete" if messages else "timeout",
        "data": {
            ("cursor" if messages else "resume_cursor"): cursor,
            "messages": messages,
        },
    }


def resident(
    server: FakeMessagingServer,
    state_file: Path,
    *,
    retry_backoff: tuple[float, ...] = (),
    sleep=lambda _seconds: None,
    logger=lambda _message: None,
    key_factory=lambda: CLIENT_KEY,
    slow_seconds: float = 0.0,
):
    return echo.EchoResident(
        echo.JsonHttpClient(server.base_url, TOKEN, timeout=2),
        echo.StateStore(state_file),
        retry_backoff_seconds=retry_backoff,
        sleep=sleep,
        logger=logger,
        ulid_factory=key_factory,
        slow_seconds=slow_seconds,
    )


class AgentMessagingEchoTests(unittest.TestCase):
    def test_cursor_state_resumes_and_is_atomically_replaced_private(self) -> None:
        with tempfile.TemporaryDirectory() as directory, fake_server() as server:
            state_file = Path(directory) / "resident" / "cursor.json"
            store = echo.StateStore(state_file)
            store.save(echo.EchoState(cursor=41, pending=()))
            server.get_responses.append((200, sync_response(44, [])))

            self.assertTrue(resident(server, state_file).run_cycle())

            self.assertEqual(44, store.load().cursor)
            self.assertEqual((), store.load().pending)
            self.assertEqual(
                "/v1/workspace/messaging/sync?cursor=41&wait=25",
                server.calls[0]["path"],
            )
            self.assertEqual(0o600, stat.S_IMODE(state_file.stat().st_mode))
            self.assertEqual([], list(state_file.parent.glob(f".{state_file.name}.*")))

    def test_question_reply_stays_queued_across_restart_then_sends(self) -> None:
        private_question = "private deadline question that must not become an acknowledgement body"
        with tempfile.TemporaryDirectory() as directory, fake_server() as server:
            state_file = Path(directory) / "cursor.json"
            server.get_responses.extend(
                [
                    (
                        200,
                        sync_response(
                            9,
                            [
                                {
                                    "conversation_id": CONVERSATION_ID,
                                    "seq": 7,
                                    "from_agent_id": "owner",
                                    "kind": "question",
                                    "body_md": private_question,
                                }
                            ],
                        ),
                    ),
                    (200, sync_response(9, [])),
                ]
            )
            server.post_responses.extend(
                [
                    (503, {"error": {"code": "temporarily_unavailable"}}),
                    (
                        200,
                        {
                            "status": "committed",
                            "data": {"message": {"from_agent_id": "echo"}, "duplicate": False},
                        },
                    ),
                ]
            )

            first = resident(server, state_file)
            self.assertFalse(first.run_cycle())
            queued = echo.StateStore(state_file).load()
            self.assertEqual(9, queued.cursor)
            self.assertEqual(1, len(queued.pending))
            self.assertEqual("Acknowledged.", queued.pending[0].body_md)
            self.assertEqual(7, queued.pending[0].in_reply_to)
            self.assertEqual(CLIENT_KEY, queued.pending[0].client_key)

            def unexpected_key() -> str:
                raise AssertionError("a queued logical reply must not mint a new key after restart")

            second = resident(server, state_file, key_factory=unexpected_key)
            self.assertTrue(second.run_cycle())
            final_state = echo.StateStore(state_file).load()
            self.assertEqual((), final_state.pending)
            self.assertEqual("echo", final_state.principal_id)
            posts = [call for call in server.calls if call["method"] == "POST"]
            self.assertEqual(2, len(posts))
            self.assertEqual(posts[0]["body"], posts[1]["body"])
            self.assertEqual("Acknowledged.", posts[1]["body"]["body_md"])
            self.assertEqual(7, posts[1]["body"]["in_reply_to"])

    def test_ambiguous_retry_reuses_same_key_and_body(self) -> None:
        private_text = "echo this exact private text"
        sleeps: list[float] = []
        with tempfile.TemporaryDirectory() as directory, fake_server() as server:
            state_file = Path(directory) / "cursor.json"
            server.get_responses.append(
                (
                    200,
                    sync_response(
                        1,
                        [
                            {
                                "conversation_id": CONVERSATION_ID,
                                "seq": 1,
                                "from_agent_id": "owner",
                                "kind": "text",
                                "body_md": private_text,
                            }
                        ],
                    ),
                )
            )
            server.post_responses.extend(
                [
                    (503, {"error": {"code": "temporarily_unavailable"}}),
                    (
                        200,
                        {
                            "status": "committed",
                            "data": {"message": {"from_agent_id": "echo"}, "duplicate": True},
                        },
                    ),
                ]
            )

            instance = resident(
                server,
                state_file,
                retry_backoff=(0.125,),
                sleep=sleeps.append,
                slow_seconds=0.5,
            )
            self.assertTrue(instance.run_cycle())

            posts = [call for call in server.calls if call["method"] == "POST"]
            self.assertEqual(2, len(posts))
            self.assertEqual(posts[0]["body"], posts[1]["body"])
            self.assertEqual(CLIENT_KEY, posts[0]["body"]["client_key"])
            self.assertEqual(private_text, posts[0]["body"]["body_md"])
            self.assertEqual(1, posts[0]["body"]["in_reply_to"])
            self.assertEqual([0.5, 0.125], sleeps)
            final_state = echo.StateStore(state_file).load()
            self.assertEqual((), final_state.pending)
            self.assertEqual("echo", final_state.principal_id)

            server.get_responses.append(
                (
                    200,
                    sync_response(
                        2,
                        [
                            {
                                "conversation_id": CONVERSATION_ID,
                                "seq": 2,
                                "from_agent_id": "echo",
                                "kind": "text",
                                "body_md": private_text,
                            }
                        ],
                    ),
                )
            )
            self.assertTrue(instance.run_cycle())
            self.assertEqual(2, len([call for call in server.calls if call["method"] == "POST"]))

    def test_default_diagnostics_never_include_bearer_or_message_body(self) -> None:
        private_text = "body phrase 4f4a55f7 that must never appear in output"
        output = io.StringIO()
        with tempfile.TemporaryDirectory() as directory, fake_server() as server:
            state_file = Path(directory) / "cursor.json"
            server.get_responses.append(
                (
                    200,
                    sync_response(
                        3,
                        [
                            {
                                "conversation_id": CONVERSATION_ID,
                                "seq": 2,
                                "from_agent_id": "owner",
                                "kind": "text",
                                "body_md": private_text,
                            }
                        ],
                    ),
                )
            )
            server.post_responses.append(
                (503, {"error": {"message": f"{TOKEN} {private_text}"}})
            )

            instance = resident(
                server,
                state_file,
                logger=lambda message: print(message, file=output),
            )
            self.assertFalse(instance.run_cycle())

            rendered = output.getvalue()
            self.assertIn("reply outcome remains ambiguous", rendered)
            self.assertNotIn(TOKEN, rendered)
            self.assertNotIn(private_text, rendered)
            self.assertNotIn("Bearer", rendered)


if __name__ == "__main__":
    unittest.main()
