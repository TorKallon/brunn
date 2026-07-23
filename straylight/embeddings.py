from __future__ import annotations

import hashlib
import json
import math
import os
import urllib.error
import urllib.request
from array import array
from dataclasses import dataclass
from typing import Protocol, Sequence

from straylight_eval import tokenize


class EmbeddingProvider(Protocol):
    provider: str
    model: str

    def embed(self, texts: Sequence[str]) -> list[list[float]]: ...


def pack_vector(values: Sequence[float]) -> bytes:
    return array("f", values).tobytes()


def unpack_vector(value: bytes) -> array:
    result = array("f")
    result.frombytes(value)
    return result


def vector_norm(values: Sequence[float]) -> float:
    return math.sqrt(sum(value * value for value in values))


def cosine_similarity(left: Sequence[float], right: Sequence[float], right_norm: float | None = None) -> float:
    if len(left) != len(right):
        return 0.0
    left_norm = vector_norm(left)
    right_norm = right_norm if right_norm is not None else vector_norm(right)
    if not left_norm or not right_norm:
        return 0.0
    return sum(a * b for a, b in zip(left, right, strict=True)) / (left_norm * right_norm)


@dataclass
class HashingEmbeddingProvider:
    """Dependency-free deterministic provider for tests and degraded local use."""

    dimensions: int = 256
    provider: str = "local"
    model: str = "hashing-v1"

    def embed(self, texts: Sequence[str]) -> list[list[float]]:
        vectors: list[list[float]] = []
        for text in texts:
            vector = [0.0] * self.dimensions
            for token in tokenize(text):
                digest = hashlib.sha256(token.encode()).digest()
                index = int.from_bytes(digest[:4], "big") % self.dimensions
                sign = 1.0 if digest[4] & 1 else -1.0
                vector[index] += sign
            norm = vector_norm(vector)
            vectors.append([value / norm for value in vector] if norm else vector)
        return vectors


@dataclass
class OpenAIEmbeddingProvider:
    model: str = "text-embedding-3-small"
    batch_size: int = 64
    timeout_seconds: int = 90
    provider: str = "openai"
    api_key: str | None = None
    base_url: str | None = None

    def __post_init__(self) -> None:
        self.api_key = self.api_key or os.environ.get("OPENAI_API_KEY")
        self.base_url = (self.base_url or os.environ.get("OPENAI_BASE_URL") or "https://api.openai.com/v1").rstrip("/")
        if not self.api_key:
            raise ValueError("OPENAI_API_KEY is required for OpenAI embeddings")

    def embed(self, texts: Sequence[str]) -> list[list[float]]:
        all_vectors: list[list[float]] = []
        for offset in range(0, len(texts), self.batch_size):
            batch = list(texts[offset:offset + self.batch_size])
            body = json.dumps({
                "model": self.model,
                "input": batch,
                "encoding_format": "float",
            }).encode()
            headers = {
                "Authorization": f"Bearer {self.api_key}",
                "Content-Type": "application/json",
            }
            if os.environ.get("OPENAI_ORGANIZATION"):
                headers["OpenAI-Organization"] = os.environ["OPENAI_ORGANIZATION"]
            if os.environ.get("OPENAI_PROJECT"):
                headers["OpenAI-Project"] = os.environ["OPENAI_PROJECT"]
            request = urllib.request.Request(
                f"{self.base_url}/embeddings",
                data=body,
                headers=headers,
                method="POST",
            )
            try:
                with urllib.request.urlopen(request, timeout=self.timeout_seconds) as response:
                    payload = json.load(response)
            except urllib.error.HTTPError as exc:
                detail = exc.read().decode(errors="replace")[:2000]
                raise RuntimeError(f"OpenAI embeddings request failed ({exc.code}): {detail}") from exc
            rows = sorted(payload.get("data", []), key=lambda item: item["index"])
            if len(rows) != len(batch):
                raise RuntimeError("OpenAI embeddings response did not match the request batch")
            all_vectors.extend(row["embedding"] for row in rows)
        return all_vectors
