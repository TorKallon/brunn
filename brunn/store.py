from __future__ import annotations

import hashlib
import json
import sqlite3
import threading
import uuid
from contextlib import contextmanager
from datetime import datetime
from pathlib import Path
from typing import Any, Iterable, Sequence

from brunn_eval import split_section_lines, split_sections, tokenize

from .embeddings import EmbeddingProvider, cosine_similarity, pack_vector, unpack_vector, vector_norm


TEXT_SUFFIXES = {".md", ".txt", ".csv", ".tsv", ".sql", ".json", ".jsonl"}
CHUNK_CHARS = 1600
CHUNK_OVERLAP_CHARS = 220


def now_iso() -> str:
    return datetime.now().astimezone().isoformat(timespec="seconds")


def content_hash(content: str) -> str:
    return hashlib.sha256(content.encode()).hexdigest()


def infer_project(relative: str) -> str:
    parts = Path(relative).parts
    if len(parts) >= 2 and parts[0] in {"Projects", "Topics", "Trips"}:
        return parts[1]
    if len(parts) >= 3 and parts[0] == "eval" and parts[1] == "transition-deltas":
        stem = Path(relative).stem.casefold()
        prefixes = {
            "warmind": "Warmind",
            "charlemagne": "Charlemagne",
            "star-rupture": "Star Rupture",
            "switzerland": "Switzerland",
            "brunn": "Brunn",
        }
        return next((project for prefix, project in prefixes.items() if stem.startswith(prefix)), "Transition Deltas")
    return parts[0] if parts else "Corpus"


def chunk_document(path: str, text: str) -> tuple[list[str], list[dict[str, Any]]]:
    suffix = Path(path).suffix.casefold()
    if suffix == ".md":
        sections = split_sections(path, text)
    else:
        sections = [(f"({suffix.lstrip('.')} artifact)", 1, text.splitlines())]
    headings = [heading for heading, _, _ in sections if heading != "(preamble)"]
    chunks: list[dict[str, Any]] = []
    for heading, section_start, section_lines in sections:
        for start, end, chunk_text in split_section_lines(
            section_lines,
            section_start,
            CHUNK_CHARS,
            CHUNK_OVERLAP_CHARS,
        ):
            if not chunk_text.strip():
                continue
            digest = hashlib.sha256(
                f"{path}\0{start}\0{end}\0{chunk_text}".encode()
            ).hexdigest()[:20]
            chunks.append({
                "ref": f"chunk:{digest}",
                "heading": heading,
                "start": start,
                "end": end,
                "content": chunk_text,
                "content_hash": content_hash(chunk_text),
            })
    return headings, chunks


class BrunnStore:
    def __init__(self, path: Path | str):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._write_lock = threading.RLock()
        self._initialize()

    def connect(self) -> sqlite3.Connection:
        connection = sqlite3.connect(self.path, timeout=30)
        connection.row_factory = sqlite3.Row
        connection.execute("PRAGMA foreign_keys = ON")
        connection.execute("PRAGMA busy_timeout = 30000")
        return connection

    @contextmanager
    def connection(self):
        connection = self.connect()
        try:
            yield connection
        except Exception:
            connection.rollback()
            raise
        else:
            connection.commit()
        finally:
            connection.close()

    def _initialize(self) -> None:
        with self.connection() as connection:
            connection.execute("PRAGMA journal_mode = WAL")
            connection.executescript(
                """
                CREATE TABLE IF NOT EXISTS revisions (
                    seq INTEGER PRIMARY KEY AUTOINCREMENT,
                    revision_id TEXT UNIQUE,
                    parent_seq INTEGER REFERENCES revisions(seq),
                    corpus_hash TEXT NOT NULL,
                    note TEXT,
                    created_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS document_versions (
                    path TEXT NOT NULL,
                    revision_seq INTEGER NOT NULL REFERENCES revisions(seq),
                    project TEXT NOT NULL,
                    content_hash TEXT NOT NULL,
                    content TEXT NOT NULL,
                    headings_json TEXT NOT NULL,
                    deleted INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (path, revision_seq)
                );

                CREATE TABLE IF NOT EXISTS chunks (
                    rowid INTEGER PRIMARY KEY AUTOINCREMENT,
                    ref TEXT NOT NULL,
                    path TEXT NOT NULL,
                    revision_seq INTEGER NOT NULL REFERENCES revisions(seq),
                    project TEXT NOT NULL,
                    heading TEXT NOT NULL,
                    start_line INTEGER NOT NULL,
                    end_line INTEGER NOT NULL,
                    content_hash TEXT NOT NULL,
                    content TEXT NOT NULL,
                    UNIQUE (ref, revision_seq)
                );

                CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
                    ref UNINDEXED,
                    path UNINDEXED,
                    project UNINDEXED,
                    revision_seq UNINDEXED,
                    heading,
                    content,
                    tokenize = 'unicode61'
                );

                CREATE TABLE IF NOT EXISTS embeddings (
                    chunk_rowid INTEGER NOT NULL REFERENCES chunks(rowid) ON DELETE CASCADE,
                    provider TEXT NOT NULL,
                    model TEXT NOT NULL,
                    dimensions INTEGER NOT NULL,
                    norm REAL NOT NULL,
                    vector BLOB NOT NULL,
                    created_at TEXT NOT NULL,
                    PRIMARY KEY (chunk_rowid, provider, model)
                );

                CREATE TABLE IF NOT EXISTS sessions (
                    session_id TEXT PRIMARY KEY,
                    revision_seq INTEGER NOT NULL REFERENCES revisions(seq),
                    task TEXT NOT NULL,
                    scope_json TEXT NOT NULL,
                    resumed_checkpoint_id TEXT,
                    created_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS checkpoints (
                    checkpoint_id TEXT PRIMARY KEY,
                    parent_checkpoint_id TEXT REFERENCES checkpoints(checkpoint_id),
                    corpus_revision_seq INTEGER NOT NULL REFERENCES revisions(seq),
                    session_id TEXT REFERENCES sessions(session_id),
                    state_json TEXT NOT NULL,
                    source_refs_json TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS operations (
                    operation_id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL REFERENCES sessions(session_id),
                    operation TEXT NOT NULL,
                    request_json TEXT NOT NULL,
                    result_chars INTEGER NOT NULL,
                    created_at TEXT NOT NULL
                );
                """
            )

    def latest_revision(self) -> dict[str, Any] | None:
        with self.connection() as connection:
            row = connection.execute("SELECT * FROM revisions ORDER BY seq DESC LIMIT 1").fetchone()
        return dict(row) if row else None

    def revision(self, revision: str | int | None = None) -> dict[str, Any]:
        with self.connection() as connection:
            if revision is None or revision == "latest":
                row = connection.execute("SELECT * FROM revisions ORDER BY seq DESC LIMIT 1").fetchone()
            elif isinstance(revision, int):
                row = connection.execute("SELECT * FROM revisions WHERE seq = ?", (revision,)).fetchone()
            else:
                row = connection.execute("SELECT * FROM revisions WHERE revision_id = ?", (revision,)).fetchone()
        if not row:
            raise KeyError(f"Unknown corpus revision: {revision}")
        return dict(row)

    def active_documents(self, revision_seq: int, project: str | None = None) -> list[dict[str, Any]]:
        parameters: list[Any] = [revision_seq]
        project_filter = ""
        if project:
            project_filter = " AND project = ?"
            parameters.append(project)
        query = f"""
            WITH ranked AS (
                SELECT *, ROW_NUMBER() OVER (PARTITION BY path ORDER BY revision_seq DESC) AS rank
                FROM document_versions
                WHERE revision_seq <= ?{project_filter}
            )
            SELECT path, revision_seq, project, content_hash, content, headings_json
            FROM ranked WHERE rank = 1 AND deleted = 0 ORDER BY path
        """
        with self.connection() as connection:
            rows = connection.execute(query, parameters).fetchall()
        return [dict(row) for row in rows]

    def _latest_hashes(self, connection: sqlite3.Connection, revision_seq: int | None) -> dict[str, str]:
        if revision_seq is None:
            return {}
        rows = connection.execute(
            """
            WITH ranked AS (
                SELECT path, content_hash, deleted,
                       ROW_NUMBER() OVER (PARTITION BY path ORDER BY revision_seq DESC) AS rank
                FROM document_versions WHERE revision_seq <= ?
            )
            SELECT path, content_hash FROM ranked WHERE rank = 1 AND deleted = 0
            """,
            (revision_seq,),
        ).fetchall()
        return {row["path"]: row["content_hash"] for row in rows}

    def ingest_directory(self, root: Path | str, note: str = "directory ingestion") -> dict[str, Any]:
        root = Path(root)
        documents = {
            path.relative_to(root).as_posix(): path.read_text(encoding="utf-8", errors="replace")
            for path in sorted(root.rglob("*"))
            if path.is_file() and path.suffix.casefold() in TEXT_SUFFIXES
        }
        return self.ingest_documents(documents, note=note)

    def ingest_documents(self, documents: dict[str, str | None], note: str) -> dict[str, Any]:
        with self._write_lock, self.connection() as connection:
            connection.execute("BEGIN IMMEDIATE")
            latest = connection.execute("SELECT * FROM revisions ORDER BY seq DESC LIMIT 1").fetchone()
            parent_seq = latest["seq"] if latest else None
            previous = self._latest_hashes(connection, parent_seq)
            changed: dict[str, tuple[str | None, str]] = {}
            for path, text in documents.items():
                normalized = Path(path).as_posix().lstrip("./")
                digest = content_hash(text) if text is not None else "deleted"
                if previous.get(normalized) != digest:
                    changed[normalized] = (text, digest)
            if not changed:
                connection.rollback()
                if not latest:
                    raise ValueError("Cannot create an empty initial revision")
                return {**dict(latest), "changed_paths": []}

            digest = hashlib.sha256()
            digest.update((latest["revision_id"] if latest else "root").encode())
            for path, (_, value_hash) in sorted(changed.items()):
                digest.update(path.encode())
                digest.update(b"\0")
                digest.update(value_hash.encode())
            corpus_hash = digest.hexdigest()
            created_at = now_iso()
            cursor = connection.execute(
                "INSERT INTO revisions(revision_id, parent_seq, corpus_hash, note, created_at) VALUES(NULL, ?, ?, ?, ?)",
                (parent_seq, corpus_hash, note, created_at),
            )
            revision_seq = cursor.lastrowid
            revision_id = f"rev_{revision_seq:06d}_{corpus_hash[:12]}"
            connection.execute("UPDATE revisions SET revision_id = ? WHERE seq = ?", (revision_id, revision_seq))

            for path, (text, digest_value) in sorted(changed.items()):
                if text is None:
                    connection.execute(
                        "INSERT INTO document_versions VALUES (?, ?, ?, ?, '', '[]', 1)",
                        (path, revision_seq, infer_project(path), digest_value),
                    )
                    continue
                project = infer_project(path)
                headings, chunks = chunk_document(path, text)
                connection.execute(
                    "INSERT INTO document_versions VALUES (?, ?, ?, ?, ?, ?, 0)",
                    (path, revision_seq, project, digest_value, text, json.dumps(headings)),
                )
                for chunk in chunks:
                    chunk_cursor = connection.execute(
                        """
                        INSERT INTO chunks(ref, path, revision_seq, project, heading, start_line, end_line, content_hash, content)
                        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                        """,
                        (
                            chunk["ref"], path, revision_seq, project, chunk["heading"],
                            chunk["start"], chunk["end"], chunk["content_hash"], chunk["content"],
                        ),
                    )
                    connection.execute(
                        "INSERT INTO chunks_fts(rowid, ref, path, project, revision_seq, heading, content) VALUES (?, ?, ?, ?, ?, ?, ?)",
                        (
                            chunk_cursor.lastrowid, chunk["ref"], path, project,
                            revision_seq, chunk["heading"], chunk["content"],
                        ),
                    )
            connection.commit()
        return {
            "seq": revision_seq,
            "revision_id": revision_id,
            "parent_seq": parent_seq,
            "corpus_hash": corpus_hash,
            "note": note,
            "created_at": created_at,
            "changed_paths": sorted(changed),
        }

    def corpus_map(self, revision_seq: int, scope: str | None = None) -> dict[str, Any]:
        documents = self.active_documents(revision_seq)
        projects: dict[str, list[dict[str, Any]]] = {}
        for document in documents:
            projects.setdefault(document["project"], []).append(document)
        result: dict[str, Any] = {
            "projects": [
                {
                    "project": project,
                    "documents": len(items),
                    "characters": sum(len(item["content"]) for item in items),
                }
                for project, items in sorted(projects.items())
            ]
        }
        if scope:
            selected = next((name for name in projects if name.casefold() == scope.casefold()), None)
            if selected:
                result["scope"] = selected
                result["files"] = [
                    {
                        "path": item["path"],
                        "characters": len(item["content"]),
                        "headings": json.loads(item["headings_json"])[:16],
                    }
                    for item in projects[selected]
                ]
            else:
                result["scope_error"] = f"Unknown scope: {scope}"
        return result

    def changes_since(self, from_revision_seq: int, to_revision_seq: int) -> list[dict[str, Any]]:
        with self.connection() as connection:
            rows = connection.execute(
                """
                SELECT path, revision_seq, project, content_hash, content, deleted
                FROM document_versions
                WHERE revision_seq > ? AND revision_seq <= ?
                ORDER BY revision_seq, path
                """,
                (from_revision_seq, to_revision_seq),
            ).fetchall()
        return [
            {
                "path": row["path"],
                "revision_seq": row["revision_seq"],
                "project": row["project"],
                "content_hash": row["content_hash"],
                "change": "deleted" if row["deleted"] else "upserted",
                "content": row["content"] if not row["deleted"] and len(row["content"]) <= 6000 else None,
            }
            for row in rows
        ]

    def _active_chunk_rows(self, connection: sqlite3.Connection, revision_seq: int, project: str | None = None) -> list[sqlite3.Row]:
        parameters: list[Any] = [revision_seq]
        project_filter = ""
        if project:
            project_filter = " AND project = ?"
            parameters.append(project)
        return connection.execute(
            f"""
            WITH latest AS (
                SELECT path, MAX(revision_seq) AS revision_seq
                FROM document_versions
                WHERE revision_seq <= ?{project_filter}
                GROUP BY path
            )
            SELECT chunks.* FROM chunks
            JOIN latest USING(path, revision_seq)
            ORDER BY chunks.path, chunks.start_line
            """,
            parameters,
        ).fetchall()

    def lexical_search(self, revision_seq: int, query: str, project: str | None, limit: int) -> list[dict[str, Any]]:
        terms = list(dict.fromkeys(tokenize(query)))[:32]
        if not terms:
            return []
        fts_query = " OR ".join(f'"{term.replace(chr(34), "")}"' for term in terms)
        parameters: list[Any] = [revision_seq, fts_query]
        project_filter = ""
        if project:
            project_filter = " AND chunks.project = ?"
            parameters.append(project)
        parameters.append(max(limit * 4, 40))
        with self.connection() as connection:
            rows = connection.execute(
                f"""
                WITH latest AS (
                    SELECT path, MAX(revision_seq) AS revision_seq
                    FROM document_versions WHERE revision_seq <= ? GROUP BY path
                )
                SELECT chunks.*, bm25(chunks_fts, 0.0, 0.0, 0.0, 0.0, 2.0, 1.0) AS lexical_score
                FROM chunks_fts
                JOIN chunks ON chunks.rowid = chunks_fts.rowid
                JOIN latest USING(path, revision_seq)
                WHERE chunks_fts MATCH ?{project_filter}
                ORDER BY lexical_score ASC, chunks.path, chunks.start_line
                LIMIT ?
                """,
                parameters,
            ).fetchall()
        return [self._chunk_result(row, ["lexical_recall"], -float(row["lexical_score"])) for row in rows[:limit]]

    def index_embeddings(self, provider: EmbeddingProvider, batch_size: int = 64) -> dict[str, Any]:
        latest = self.revision()
        with self.connection() as connection:
            rows = connection.execute(
                """
                WITH latest_docs AS (
                    SELECT path, MAX(revision_seq) AS revision_seq
                    FROM document_versions WHERE revision_seq <= ? GROUP BY path
                )
                SELECT chunks.* FROM chunks
                JOIN latest_docs USING(path, revision_seq)
                LEFT JOIN embeddings ON embeddings.chunk_rowid = chunks.rowid
                    AND embeddings.provider = ? AND embeddings.model = ?
                WHERE embeddings.chunk_rowid IS NULL
                ORDER BY chunks.rowid
                """,
                (latest["seq"], provider.provider, provider.model),
            ).fetchall()
        indexed = 0
        dimensions = 0
        for offset in range(0, len(rows), batch_size):
            batch = rows[offset:offset + batch_size]
            vectors = provider.embed([row["content"] for row in batch])
            if len(vectors) != len(batch):
                raise RuntimeError("Embedding provider returned an unexpected vector count")
            with self._write_lock, self.connection() as connection:
                connection.execute("BEGIN IMMEDIATE")
                for row, vector in zip(batch, vectors, strict=True):
                    dimensions = len(vector)
                    connection.execute(
                        """
                        INSERT OR REPLACE INTO embeddings
                        (chunk_rowid, provider, model, dimensions, norm, vector, created_at)
                        VALUES (?, ?, ?, ?, ?, ?, ?)
                        """,
                        (
                            row["rowid"], provider.provider, provider.model, dimensions,
                            vector_norm(vector), pack_vector(vector), now_iso(),
                        ),
                    )
                connection.commit()
            indexed += len(batch)
        return {
            "provider": provider.provider,
            "model": provider.model,
            "indexed": indexed,
            "dimensions": dimensions,
            "revision_id": latest["revision_id"],
        }

    def semantic_search(
        self,
        revision_seq: int,
        query: str,
        project: str | None,
        limit: int,
        provider: EmbeddingProvider,
    ) -> list[dict[str, Any]]:
        query_vector = provider.embed([query])[0]
        with self.connection() as connection:
            rows = connection.execute(
                """
                WITH latest AS (
                    SELECT path, MAX(revision_seq) AS revision_seq
                    FROM document_versions WHERE revision_seq <= ? GROUP BY path
                )
                SELECT chunks.*, embeddings.vector, embeddings.norm
                FROM chunks
                JOIN latest USING(path, revision_seq)
                JOIN embeddings ON embeddings.chunk_rowid = chunks.rowid
                WHERE embeddings.provider = ? AND embeddings.model = ?
                  AND (? IS NULL OR chunks.project = ?)
                """,
                (revision_seq, provider.provider, provider.model, project, project),
            ).fetchall()
        scored = [
            (cosine_similarity(query_vector, unpack_vector(row["vector"]), row["norm"]), row)
            for row in rows
        ]
        scored.sort(key=lambda item: (-item[0], item[1]["path"], item[1]["start_line"]))
        return [self._chunk_result(row, ["semantic_recall"], score) for score, row in scored[:limit]]

    def hybrid_search(
        self,
        revision_seq: int,
        query: str,
        project: str | None,
        limit: int,
        provider: EmbeddingProvider | None = None,
    ) -> dict[str, Any]:
        lexical = self.lexical_search(revision_seq, query, project, max(limit * 2, 12))
        semantic = self.semantic_search(revision_seq, query, project, max(limit * 2, 12), provider) if provider else []
        fused: dict[str, dict[str, Any]] = {}
        scores: dict[str, float] = {}
        reasons: dict[str, set[str]] = {}
        for results in [lexical, semantic]:
            for rank, item in enumerate(results, start=1):
                ref = item["ref"]
                fused.setdefault(ref, item)
                scores[ref] = scores.get(ref, 0.0) + 1.0 / (60 + rank)
                reasons.setdefault(ref, set()).update(item["why_selected"])
        ranked = sorted(fused.values(), key=lambda item: (-scores[item["ref"]], item["path"], item["lines"][0]))
        selected: list[dict[str, Any]] = []
        per_path: dict[str, int] = {}
        for item in ranked:
            if per_path.get(item["path"], 0) >= 3:
                continue
            item = {**item, "score": round(scores[item["ref"]], 8), "why_selected": sorted(reasons[item["ref"]])}
            selected.append(item)
            per_path[item["path"]] = per_path.get(item["path"], 0) + 1
            if len(selected) >= limit:
                break
        return {
            "query": query,
            "scope": project,
            "modes": ["lexical", *( ["semantic"] if provider else [])],
            "status": "complete" if provider else "degraded",
            "degraded_reason": None if provider else "semantic index unavailable; lexical retrieval used",
            "results": selected,
        }

    @staticmethod
    def _chunk_result(row: sqlite3.Row, why: list[str], score: float) -> dict[str, Any]:
        return {
            "ref": row["ref"],
            "path": row["path"],
            "project": row["project"],
            "heading": row["heading"],
            "lines": [row["start_line"], row["end_line"]],
            "content_hash": row["content_hash"],
            "content": row["content"],
            "score": round(score, 8),
            "why_selected": why,
        }

    def read(self, revision_seq: int, request: dict[str, Any]) -> dict[str, Any]:
        ref = request.get("ref")
        path = request.get("path")
        with self.connection() as connection:
            if ref:
                rows = self._active_chunk_rows(connection, revision_seq)
                row = next((item for item in rows if item["ref"] == ref), None)
                if not row:
                    raise KeyError(f"Unknown active chunk ref: {ref}")
                return self._chunk_result(row, ["exact_ref"], 1.0)
        if not path:
            raise ValueError("read requires ref or path")
        document = next((item for item in self.active_documents(revision_seq) if item["path"] == path), None)
        if not document:
            raise KeyError(f"Unknown active document: {path}")
        lines = document["content"].splitlines()
        start = max(1, int(request.get("start") or 1))
        end = min(len(lines), int(request.get("end") or len(lines)))
        max_chars = min(int(request.get("max_chars") or 20000), 50000)
        content = "\n".join(lines[start - 1:end])
        truncated = len(content) > max_chars
        return {
            "path": path,
            "content_hash": document["content_hash"],
            "lines": [start, end],
            "content": content[:max_chars],
            "truncated": truncated,
            "continuation_start": end + 1 if truncated else None,
        }

    def create_session(
        self,
        task: str,
        scope: Sequence[str],
        revision: str | int | None = None,
        checkpoint_id: str | None = None,
    ) -> dict[str, Any]:
        revision_row = self.revision(revision)
        checkpoint = self.get_checkpoint(checkpoint_id) if checkpoint_id else None
        session_id = f"ms_{uuid.uuid4().hex[:20]}"
        with self.connection() as connection:
            connection.execute(
                "INSERT INTO sessions VALUES (?, ?, ?, ?, ?, ?)",
                (session_id, revision_row["seq"], task, json.dumps(list(scope)), checkpoint_id, now_iso()),
            )
        return {"session_id": session_id, "revision": revision_row, "checkpoint": checkpoint}

    def session(self, session_id: str) -> dict[str, Any]:
        with self.connection() as connection:
            row = connection.execute("SELECT * FROM sessions WHERE session_id = ?", (session_id,)).fetchone()
        if not row:
            raise KeyError(f"Unknown session: {session_id}")
        result = dict(row)
        result["scope"] = json.loads(result.pop("scope_json"))
        return result

    def get_checkpoint(self, checkpoint_id: str | None) -> dict[str, Any] | None:
        if not checkpoint_id:
            return None
        with self.connection() as connection:
            row = connection.execute("SELECT * FROM checkpoints WHERE checkpoint_id = ?", (checkpoint_id,)).fetchone()
        if not row:
            raise KeyError(f"Unknown checkpoint: {checkpoint_id}")
        result = dict(row)
        result["state"] = json.loads(result.pop("state_json"))
        result["source_refs"] = json.loads(result.pop("source_refs_json"))
        result["corpus_revision"] = self.revision(result.pop("corpus_revision_seq"))["revision_id"]
        return result

    def create_checkpoint(
        self,
        corpus_revision_seq: int,
        state: dict[str, Any],
        source_refs: Sequence[str],
        parent_checkpoint_id: str | None = None,
        session_id: str | None = None,
    ) -> dict[str, Any]:
        if parent_checkpoint_id:
            self.get_checkpoint(parent_checkpoint_id)
        active_paths = {item["path"] for item in self.active_documents(corpus_revision_seq)}
        with self.connection() as connection:
            active_chunk_refs = {
                row["ref"] for row in self._active_chunk_rows(connection, corpus_revision_seq)
            }
        unknown_refs = sorted(set(source_refs) - active_paths - active_chunk_refs)
        if unknown_refs:
            raise ValueError(f"checkpoint contains unknown source refs: {unknown_refs}")
        canonical_state = json.dumps(state, sort_keys=True, separators=(",", ":"))
        canonical_refs = json.dumps(sorted(set(source_refs)), separators=(",", ":"))
        digest = hashlib.sha256(
            f"{parent_checkpoint_id or ''}\0{corpus_revision_seq}\0{canonical_state}\0{canonical_refs}".encode()
        ).hexdigest()
        checkpoint_id = f"cp_{digest[:20]}"
        with self._write_lock, self.connection() as connection:
            connection.execute("BEGIN IMMEDIATE")
            connection.execute(
                """
                INSERT OR IGNORE INTO checkpoints
                (checkpoint_id, parent_checkpoint_id, corpus_revision_seq, session_id, state_json, source_refs_json, created_at)
                VALUES (?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    checkpoint_id, parent_checkpoint_id, corpus_revision_seq, session_id,
                    canonical_state, canonical_refs, now_iso(),
                ),
            )
            connection.commit()
        return self.get_checkpoint(checkpoint_id) or {}

    def record_operation(self, session_id: str, operation: str, request: dict[str, Any], result: dict[str, Any]) -> None:
        rendered = json.dumps(result, separators=(",", ":"))
        with self.connection() as connection:
            connection.execute(
                "INSERT INTO operations(session_id, operation, request_json, result_chars, created_at) VALUES (?, ?, ?, ?, ?)",
                (session_id, operation, json.dumps(request, sort_keys=True), len(rendered), now_iso()),
            )

    def operation_summary(self, session_id: str) -> dict[str, Any]:
        with self.connection() as connection:
            rows = connection.execute(
                "SELECT operation, result_chars, created_at FROM operations WHERE session_id = ? ORDER BY operation_id",
                (session_id,),
            ).fetchall()
        return {
            "completed_calls": len(rows),
            "result_chars": sum(row["result_chars"] for row in rows),
            "operations": [dict(row) for row in rows],
        }
