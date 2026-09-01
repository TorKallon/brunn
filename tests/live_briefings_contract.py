#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path
from typing import Any


class ContractFailure(RuntimeError):
    pass


def load_env(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        name, value = line.split("=", 1)
        values[name] = value
    return values


class Client:
    def __init__(self, base_url: str, token: str) -> None:
        self.base_url = base_url.rstrip("/")
        self.token = token

    def request(
        self,
        method: str,
        path: str,
        *,
        body: Any | None = None,
        expected: int = 200,
    ) -> dict[str, Any]:
        payload = json.dumps(body).encode("utf-8") if body is not None else None
        request = urllib.request.Request(
            f"{self.base_url}{path}",
            data=payload,
            method=method,
            headers={
                "Accept": "application/json",
                "Authorization": f"Bearer {self.token}",
                **({"Content-Type": "application/json"} if payload is not None else {}),
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                status = response.status
                raw = response.read()
        except urllib.error.HTTPError as error:
            status = error.code
            raw = error.read()
        parsed = parse_json(raw)
        if status != expected:
            raise ContractFailure(
                f"{method} {path} returned HTTP {status}, expected {expected}: "
                f"{json.dumps(parsed, sort_keys=True)[:1000]}"
            )
        return parsed


def parse_json(raw: bytes) -> dict[str, Any]:
    if not raw:
        return {}
    value = json.loads(raw)
    if not isinstance(value, dict):
        raise ContractFailure("API response was not a JSON object")
    return value


def data(body: dict[str, Any]) -> dict[str, Any]:
    value = body.get("data", body)
    if not isinstance(value, dict):
        raise ContractFailure("API response data was not an object")
    return value


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ContractFailure(message)


def provision_user(admin: Client, run_id: str, label: str) -> Client:
    response = admin.request(
        "POST",
        "/v1/admin/users",
        body={
            "external_ref": f"briefings-contract:{run_id}:{label}",
            "display_name": f"Briefings contract {label}",
            "credential_name": f"Briefings contract {label} owner",
        },
    )
    token = (
        response.get("credential", {}).get("token")
        if isinstance(response.get("credential"), dict)
        else None
    )
    require(isinstance(token, str) and bool(token), "provisioning omitted owner token")
    return Client(admin.base_url, token)


def write(
    client: Client,
    path: str,
    content: str,
    *,
    expected_version: int | None = None,
    expected: int = 200,
) -> dict[str, Any]:
    body: dict[str, Any] = {
        "path": path,
        "content": content,
        "media_type": "text/markdown",
        "metadata": {},
    }
    if expected_version is not None:
        body["expected_version"] = expected_version
    return client.request("POST", "/v1/workspace/write", body=body, expected=expected)


def publish(client: Client, body: dict[str, Any], *, expected: int = 200) -> dict[str, Any]:
    return client.request(
        "POST",
        "/v1/workspace/briefings/publish",
        body=body,
        expected=expected,
    )


def item_action(client: Client, body: dict[str, Any], *, expected: int = 200) -> dict[str, Any]:
    return client.request(
        "POST",
        "/v1/workspace/briefings/items/action",
        body=body,
        expected=expected,
    )


def topics_snapshot(client: Client) -> dict[str, Any]:
    return data(client.request("GET", "/v1/workspace/briefings/topics"))


def publish_payload(
    run_id: str,
    date: str,
    *,
    news_body: str,
    idempotency_key: str,
    expected_version: int,
) -> dict[str, Any]:
    marker = run_id[:8]
    return {
        "date": date,
        "edition": "morning",
        "timezone": "UTC",
        "generated_at": f"{date}T06:30:00Z",
        "summary_md": [
            f"Live contract incident {marker} was disclosed overnight.",
            "AMZN opened up $32.85.",
        ],
        "sections": [
            {
                "topic": "ai",
                "title": "AI",
                "items": [
                    {
                        "id": "openai-incident",
                        "kind": "news",
                        "headline_md": f"**Live contract incident {marker} disclosed.**",
                        "body_md": news_body,
                        "why_it_matters": "Agent sandboxing is now a procurement question.",
                        "detail_md": "Fuller context with measurements and links.",
                        "what_changed": "The postmortem added a timeline.",
                        "delta": "new",
                        "story": {
                            "key": f"story-{run_id[:12]}",
                            "urls": [f"https://example.com/stories/{run_id}"],
                            "title": f"Live contract incident {marker}",
                            "entities": ["OpenAI"],
                            "event_at": date,
                        },
                        "times": {
                            "published_at": f"{date}T05:45:00Z",
                            "event_at": date,
                            "first_seen_at": f"{date}T06:12:00Z",
                        },
                    }
                ],
            },
            {
                "topic": "markets",
                "title": "Markets",
                "items": [
                    {
                        "id": "amzn-open",
                        "kind": "metric",
                        "headline_md": "**AMZN opened up $32.85.**",
                        "delta": "new",
                    }
                ],
            },
        ],
        "omitted": [
            {
                "story_key": f"omitted-{run_id[:12]}",
                "urls": [f"https://example.com/omitted/{run_id}"],
                "reason": "already delivered; no material delta",
            }
        ],
        "idempotency_key": idempotency_key,
        "expected_version": expected_version,
    }


def run(base_url: str, env: dict[str, str]) -> dict[str, Any]:
    run_id = uuid.uuid4().hex
    admin = Client(base_url, env["BRUNN_DEV_READ_WRITE_TOKEN"])
    briefer = provision_user(admin, run_id, "briefings")
    started = time.monotonic()

    date = time.strftime("%Y-%m-%d", time.gmtime())
    edition_path = f"Briefings/{date[:4]}/Morning briefing - {date}.md"
    news_id = "openai-incident"
    metric_id = "amzn-open"
    headline = f"**Live contract incident {run_id[:8]} disclosed.**"
    story_key = f"story-{run_id[:12]}"
    omitted_key = f"omitted-{run_id[:12]}"
    delivered_url = f"https://example.com/stories/{run_id}"
    omitted_url = f"https://example.com/omitted/{run_id}"
    unseen_url = f"https://example.com/unseen/{run_id}"

    first_payload = publish_payload(
        run_id,
        date,
        news_body="The provider published a postmortem overnight.",
        idempotency_key=f"live-briefings:{run_id}:publish-1",
        expected_version=0,
    )
    first = publish(briefer, first_payload)
    require(first.get("status") == "committed", "first publish did not commit")
    first_data = data(first)
    entry_ref = str(first_data.get("entry_ref", ""))
    require(entry_ref.startswith("entry:"), "publish receipt lacks an entry ref")
    require(first_data.get("path") == edition_path, "publish receipt path mismatch")
    require(first_data.get("version") == 1, "first publish did not create version 1")
    require(
        str(first_data.get("content_hash", "")).startswith("sha256:"),
        "publish receipt lacks a content hash",
    )
    require(
        first_data.get("delta")
        == {"added": sorted([news_id, metric_id]), "changed": [], "removed": []},
        "first publish delta did not report both items as added",
    )
    require(
        "skipped_invalid_urls" not in first_data,
        "publish reported skipped URLs for valid story URLs",
    )

    replay = publish(briefer, first_payload)
    require(replay.get("status") == "no_op", "identical publish replay was not a no-op")
    replay_data = data(replay)
    require(replay_data.get("entry_ref") == entry_ref, "publish replay changed the entry ref")
    require(replay_data.get("version") == 1, "publish replay advanced the version")
    require(
        replay_data.get("delta") == {"added": [], "changed": [], "removed": []},
        "publish replay reported a non-empty delta",
    )

    listing = data(briefer.request("GET", "/v1/workspace/briefings"))
    editions = listing.get("editions")
    require(isinstance(editions, list), "briefing list omitted editions")
    listed = next(
        (
            edition
            for edition in editions
            if isinstance(edition, dict) and edition.get("path") == edition_path
        ),
        None,
    )
    require(listed is not None, "briefing list did not contain the published edition")
    require(
        listed.get("date") == date
        and listed.get("edition") == "morning"
        and listed.get("entry_ref") == entry_ref
        and listed.get("version") == 1
        and listed.get("item_count") == 2
        and listed.get("section_titles") == ["AI", "Markets"],
        "briefing list row did not match the published edition",
    )

    fetched = data(briefer.request("GET", f"/v1/workspace/briefings/{date}/morning"))
    briefing = fetched.get("briefing")
    require(isinstance(briefing, dict), "briefing fetch omitted metadata")
    require(briefing.get("schema") == "briefing.v1", "briefing metadata schema mismatch")
    require(
        briefing.get("date") == date and briefing.get("edition") == "morning",
        "briefing metadata identity mismatch",
    )
    require(fetched.get("entry_ref") == entry_ref, "briefing fetch entry ref mismatch")
    require(fetched.get("version") == 1, "briefing fetch version mismatch")
    markdown = fetched.get("markdown")
    require(
        isinstance(markdown, str) and headline in markdown,
        "briefing markdown did not contain the published headline",
    )
    versions = fetched.get("versions")
    require(
        isinstance(versions, list) and len(versions) == 1,
        "briefing fetch did not report exactly one version",
    )

    dedupe = data(
        briefer.request(
            "POST",
            "/v1/workspace/briefings/dedupe-check",
            body={
                "candidates": [
                    {"urls": [delivered_url]},
                    {"urls": [unseen_url]},
                    {"urls": [omitted_url]},
                ]
            },
        )
    )
    candidates = dedupe.get("candidates")
    require(
        isinstance(candidates, list) and len(candidates) == 3,
        "dedupe-check did not report all candidates",
    )
    duplicate, unseen, omitted = candidates
    require(
        duplicate.get("verdict_hint") == "duplicate",
        "delivered URL was not classified as a duplicate",
    )
    duplicate_hits = duplicate.get("exact")
    require(
        isinstance(duplicate_hits, list) and len(duplicate_hits) == 1,
        "duplicate verdict lacked exactly one exact hit",
    )
    hit = duplicate_hits[0]
    require(
        hit.get("story_key") == story_key
        and "url" in hit.get("matched_by", [])
        and hit.get("delivery_count") == 1
        and hit.get("last_delivered_date") == date
        and hit.get("last_delivered_edition_ref") == entry_ref
        and hit.get("last_delivered_headline") == headline,
        "duplicate hit lost its delivery history",
    )
    require(
        unseen.get("verdict_hint") == "unseen" and unseen.get("exact") == [],
        "unseen URL was not classified as unseen",
    )
    require(
        omitted.get("verdict_hint") == "possible_update",
        "omitted URL was not classified as a possible update",
    )
    omitted_hits = omitted.get("exact")
    require(
        isinstance(omitted_hits, list)
        and len(omitted_hits) == 1
        and omitted_hits[0].get("story_key") == omitted_key
        and omitted_hits[0].get("delivery_count") == 0
        and omitted_hits[0].get("suppression_count") == 1,
        "omitted story did not retain its suppression history",
    )

    expand_note = f"Trace the {run_id[:8]} incident timeline."
    expanded = item_action(
        briefer,
        {
            "action": "expand",
            "edition_ref": entry_ref,
            "item_id": news_id,
            "topic_slug": "ai",
            "note": expand_note,
        },
    )
    require(expanded.get("status") == "committed", "expand action did not commit")
    expanded_data = data(expanded)
    request_path = f"Briefings/Requests/{date} - {news_id}.md"
    require(
        expanded_data.get("path") == request_path
        and expanded_data.get("status") == "pending"
        and expanded_data.get("date") == date
        and expanded_data.get("item_id") == news_id,
        "expand action did not create the pending request",
    )
    duplicate_expand = item_action(
        briefer,
        {"action": "expand", "edition_ref": entry_ref, "item_id": news_id},
        expected=409,
    )
    require(
        isinstance(duplicate_expand.get("error"), dict)
        and duplicate_expand["error"].get("code") == "request_exists",
        "repeated expand did not fail closed on the pending request",
    )

    pending = topics_snapshot(briefer).get("pending_requests")
    require(isinstance(pending, list), "topics snapshot omitted pending requests")
    pending_request = next(
        (
            item
            for item in pending
            if isinstance(item, dict) and item.get("item_id") == news_id
        ),
        None,
    )
    require(pending_request is not None, "pending expansion request was not listed")
    require(
        pending_request.get("path") == request_path
        and pending_request.get("edition_ref") == entry_ref
        and pending_request.get("date") == date
        and pending_request.get("topic") == "ai"
        and pending_request.get("note") == expand_note,
        "pending request lost its recorded fields",
    )

    republish = publish(
        briefer,
        publish_payload(
            run_id,
            date,
            news_body="The provider published a postmortem and a second advisory.",
            idempotency_key=f"live-briefings:{run_id}:publish-2",
            expected_version=1,
        ),
    )
    require(republish.get("status") == "committed", "changed republish did not commit")
    republish_data = data(republish)
    require(republish_data.get("entry_ref") == entry_ref, "republish changed the entry ref")
    require(republish_data.get("version") == 2, "republish did not create version 2")
    require(
        republish_data.get("delta") == {"added": [], "changed": [news_id], "removed": []},
        "republish delta did not isolate the changed item",
    )

    feedback = item_action(
        briefer,
        {
            "action": "feedback",
            "edition_ref": entry_ref,
            "item_id": news_id,
            "verdict": "useful",
        },
    )
    require(feedback.get("status") == "committed", "feedback action did not commit")
    feedback_line = str(data(feedback).get("line", ""))
    require(
        entry_ref in feedback_line and news_id in feedback_line and "useful" in feedback_line,
        "feedback line lost its identifying fields",
    )
    read_receipt = item_action(
        briefer,
        {"action": "read", "edition_ref": entry_ref, "item_id": metric_id},
    )
    require(read_receipt.get("status") == "committed", "read action did not commit")
    read_line = str(data(read_receipt).get("line", ""))

    topic_document = (
        "---\n"
        "name: Smoke topic\n"
        "mode: every_briefing\n"
        "section_order: 5\n"
        "---\n\n"
        "Track the live smoke topic.\n"
    )
    created_topic = write(
        briefer,
        "Briefings/Topics/smoke.md",
        topic_document,
        expected_version=0,
    )
    require(created_topic.get("status") == "committed", "topic creation did not commit")
    muted = item_action(briefer, {"action": "mute_topic", "topic_slug": "smoke"})
    require(muted.get("status") == "committed", "mute_topic did not commit")
    muted_data = data(muted)
    require(
        muted_data.get("slug") == "smoke" and muted_data.get("mode") == "muted",
        "mute_topic receipt did not report the muted mode",
    )

    snapshot = topics_snapshot(briefer)
    topics = snapshot.get("topics")
    require(isinstance(topics, list), "topics snapshot omitted topics")
    smoke_topic = next(
        (
            topic
            for topic in topics
            if isinstance(topic, dict) and topic.get("slug") == "smoke"
        ),
        None,
    )
    require(smoke_topic is not None, "created topic was not listed")
    require(
        smoke_topic.get("mode") == "muted"
        and smoke_topic.get("name") == "Smoke topic"
        and smoke_topic.get("section_order") == 5
        and "parse_error" not in smoke_topic,
        "muted topic lost its remaining frontmatter",
    )
    feedback_tail = snapshot.get("feedback_tail")
    require(
        isinstance(feedback_tail, list)
        and feedback_line in feedback_tail
        and read_line in feedback_tail
        and feedback_tail.index(feedback_line) < feedback_tail.index(read_line),
        "feedback log did not append both action lines in order",
    )

    return {
        "schema": "brunn-briefings-live-contract@v1",
        "status": "pass",
        "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
        "checks": {
            "publish_receipt": True,
            "idempotent_replay": True,
            "edition_list": True,
            "edition_fetch_markdown": True,
            "dedupe_duplicate_history": True,
            "dedupe_unseen": True,
            "dedupe_omitted_suppression": True,
            "expand_pending_request": True,
            "expand_replay_conflict": True,
            "republish_changed_delta": True,
            "feedback_append": True,
            "topic_mute": True,
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", default="http://127.0.0.1:18110")
    parser.add_argument("--env-file", type=Path, required=True)
    args = parser.parse_args()
    try:
        result = run(args.base_url, load_env(args.env_file))
    except (ContractFailure, KeyError, OSError, ValueError) as error:
        result = {
            "schema": "brunn-briefings-live-contract@v1",
            "status": "fail",
            "error": str(error),
        }
        print(json.dumps(result, indent=2, sort_keys=True))
        return 1
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
