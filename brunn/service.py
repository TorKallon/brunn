from __future__ import annotations

import json
from typing import Any, Sequence

from .embeddings import EmbeddingProvider
from .store import BrunnStore


class BrunnService:
    def __init__(self, store: BrunnStore, embedding_provider: EmbeddingProvider | None = None):
        self.store = store
        self.embedding_provider = embedding_provider

    def open(self, request: dict[str, Any]) -> dict[str, Any]:
        task = str(request.get("task") or "").strip()
        if not task:
            raise ValueError("task is required")
        scopes = request.get("scopes") or ([request["scope"]] if request.get("scope") else [])
        checkpoint_id = request.get("checkpoint_id")
        created = self.store.create_session(
            task=task,
            scope=scopes,
            revision=request.get("as_of", "latest"),
            checkpoint_id=checkpoint_id,
        )
        revision = created["revision"]
        checkpoint = created["checkpoint"]
        changes = []
        if checkpoint:
            prior = self.store.revision(checkpoint["corpus_revision"])
            changes = self.store.changes_since(prior["seq"], revision["seq"])
        maps = [self.store.corpus_map(revision["seq"], scope) for scope in scopes] if scopes else [self.store.corpus_map(revision["seq"])]
        result = {
            "session_id": created["session_id"],
            "corpus_revision": revision["revision_id"],
            "status": "complete",
            "retrieval": {
                "exact": "ready",
                "lexical": "ready",
                "semantic": "ready" if self.embedding_provider else "degraded",
            },
            "resumed_checkpoint": checkpoint,
            "changes_since_checkpoint": changes,
            "map": maps,
            "coverage": {
                "checkpoint": "complete" if checkpoint else "none",
                "revision_delta": "complete",
                "unstructured_research": "best_effort",
            },
        }
        self.store.record_operation(created["session_id"], "open", request, result)
        return result

    def query(self, request: dict[str, Any]) -> dict[str, Any]:
        session = self.store.session(request["session_id"])
        queries = request.get("queries")
        if not queries:
            queries = [{
                "query": request.get("query"),
                "scope": request.get("scope"),
                "limit": request.get("limit", 8),
            }]
        results = []
        for item in queries:
            query = str(item.get("query") or "").strip()
            if not query:
                raise ValueError("each query requires query text")
            scope = item.get("scope") or (session["scope"][0] if len(session["scope"]) == 1 else None)
            results.append(self.store.hybrid_search(
                session["revision_seq"],
                query,
                scope,
                min(int(item.get("limit", 8)), 20),
                self.embedding_provider,
            ))
        response = {
            "session_id": session["session_id"],
            "corpus_revision": self.store.revision(session["revision_seq"])["revision_id"],
            "results": results,
        }
        self.store.record_operation(session["session_id"], "query", request, response)
        return response

    def read(self, request: dict[str, Any]) -> dict[str, Any]:
        session = self.store.session(request["session_id"])
        requests = request.get("requests") or []
        if not requests:
            raise ValueError("requests must contain at least one read")
        response = {
            "session_id": session["session_id"],
            "corpus_revision": self.store.revision(session["revision_seq"])["revision_id"],
            "results": [self.store.read(session["revision_seq"], item) for item in requests],
        }
        self.store.record_operation(session["session_id"], "read", request, response)
        return response

    def checkpoint(self, request: dict[str, Any]) -> dict[str, Any]:
        session = self.store.session(request["session_id"])
        expected_parent = session.get("resumed_checkpoint_id")
        parent = request.get("parent_checkpoint_id")
        if expected_parent and parent != expected_parent:
            raise ValueError(f"child checkpoint must name resumed parent {expected_parent}")
        checkpoint = self.store.create_checkpoint(
            corpus_revision_seq=session["revision_seq"],
            state=request["state"],
            source_refs=request.get("source_refs") or [],
            parent_checkpoint_id=parent,
            session_id=session["session_id"],
        )
        response = {
            "status": "committed",
            "checkpoint_id": checkpoint["checkpoint_id"],
            "parent_checkpoint_id": checkpoint["parent_checkpoint_id"],
            "corpus_revision": checkpoint["corpus_revision"],
            "source_refs": checkpoint["source_refs"],
        }
        self.store.record_operation(session["session_id"], "checkpoint", request, response)
        return response

    def status(self, session_id: str) -> dict[str, Any]:
        session = self.store.session(session_id)
        return {
            "session": session,
            "corpus_revision": self.store.revision(session["revision_seq"])["revision_id"],
            "operations": self.store.operation_summary(session_id),
        }

    @staticmethod
    def schema() -> dict[str, Any]:
        return {
            "open": {
                "required": ["task"],
                "optional": ["scopes", "checkpoint_id", "as_of"],
            },
            "query": {
                "required": ["session_id", "queries"],
                "queries": [{"query": "string", "scope": "string|null", "limit": "1..20"}],
            },
            "read": {
                "required": ["session_id", "requests"],
                "requests": [{"ref": "chunk ref"}, {"path": "path", "start": 1, "end": 100}],
            },
            "checkpoint": {
                "required": ["session_id", "parent_checkpoint_id", "state", "source_refs"],
                "state_fields": [
                    "objective", "current_state", "decisions", "open_questions", "next_actions", "artifacts"
                ],
            },
        }
