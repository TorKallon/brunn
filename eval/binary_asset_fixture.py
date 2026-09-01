#!/usr/bin/env python3
"""Deterministic mixed Markdown/binary fixtures for Brunn State evaluation.

The fixture is intentionally synthetic. It is safe to publish, has no external
dependencies, and gives both the service and filesystem baselines identical
source bytes. The reference ledger is not a substitute for live integration;
it makes lifecycle expectations executable before a live stack is available.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import mimetypes
import os
import re
import shutil
import sqlite3
import stat
import struct
import tempfile
import unicodedata
import uuid
import zipfile
import zlib
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath
from typing import Any, Iterable, Mapping, Sequence

from PIL import Image, ImageDraw, ImageFont


FORMAT_VERSION = "brunn-state-binary-fixture-v1"
FIXTURE_EPOCH_NS = 1_783_468_800_000_000_000
ASSET_NAMESPACE = uuid.UUID("f30b647c-e502-4fc5-a0e7-359762a33e21")
GENERATED_DESCRIPTION_ROOT = ".brunn-state/generated/descriptions"
READ_ACTIONS = frozenset({"asset:list", "asset:metadata", "asset:download", "export:read"})
WRITE_ACTIONS = frozenset({"asset:import", "asset:delete"})


class PathContractError(ValueError):
    """A path cannot be represented safely and losslessly."""


class AuthorizationError(PermissionError):
    """A credential cannot perform the requested operation."""


class RangeContractError(ValueError):
    """A byte range is malformed or unsatisfiable."""


@dataclass(frozen=True)
class FileSpec:
    path: str
    content: bytes
    media_type: str
    role: str
    describes_path: str | None = None


@dataclass(frozen=True)
class Credential:
    name: str
    scopes: frozenset[str]
    actions: frozenset[str]

    def require(self, action: str, scope: str) -> None:
        if action not in self.actions:
            raise AuthorizationError(f"{self.name} cannot perform {action}")
        if scope not in self.scopes:
            raise AuthorizationError(f"{self.name} cannot access scope {scope}")


@dataclass(frozen=True)
class AssetVersion:
    version: int
    content_hash: str
    size_bytes: int
    media_type: str
    imported_revision: str


@dataclass
class LogicalAsset:
    asset_ref: str
    user_id: str
    scope: str
    path: str
    versions: list[AssetVersion] = field(default_factory=list)
    active: bool = True
    deleted_revision: str | None = None

    @property
    def current(self) -> AssetVersion:
        if not self.versions:
            raise AssertionError("logical asset has no versions")
        return self.versions[-1]


@dataclass(frozen=True)
class DownloadResult:
    status: int
    content: bytes
    content_hash: str
    content_range: str | None
    accept_ranges: str = "bytes"


def sha256_bytes(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def generated_description_path(original_path: str) -> str:
    digest = sha256_bytes(original_path.encode("utf-8"))
    return f"{GENERATED_DESCRIPTION_ROOT}/{digest}.md"


def _collision_key(path: str) -> str:
    return "/".join(
        unicodedata.normalize("NFC", part).casefold() for part in path.split("/")
    )


def validate_vault_paths(
    paths: Iterable[str],
    *,
    allow_generated_descriptions: bool = False,
) -> tuple[str, ...]:
    """Validate paths and reject aliases that cannot round-trip everywhere."""

    accepted: list[str] = []
    aliases: dict[str, str] = {}
    exact: set[str] = set()
    for raw in paths:
        if not isinstance(raw, str) or not raw:
            raise PathContractError("vault paths must be non-empty text")
        if "\x00" in raw:
            raise PathContractError(f"path contains NUL: {raw!r}")
        if "\\" in raw:
            raise PathContractError(f"path contains a backslash: {raw!r}")
        if raw.startswith("/") or re.match(r"^[A-Za-z]:", raw):
            raise PathContractError(f"path is absolute: {raw!r}")
        if any(ord(character) < 32 or ord(character) == 127 for character in raw):
            raise PathContractError(f"path contains a control character: {raw!r}")
        parts = raw.split("/")
        if any(part in {"", ".", ".."} for part in parts):
            raise PathContractError(f"path contains an unsafe component: {raw!r}")
        if any(part.endswith((" ", ".")) for part in parts):
            raise PathContractError(f"path has a non-portable trailing character: {raw!r}")
        if any(
            part.split(".", 1)[0].casefold()
            in {
                "con",
                "prn",
                "aux",
                "nul",
                "com1",
                "com2",
                "com3",
                "com4",
                "com5",
                "com6",
                "com7",
                "com8",
                "com9",
                "lpt1",
                "lpt2",
                "lpt3",
                "lpt4",
                "lpt5",
                "lpt6",
                "lpt7",
                "lpt8",
                "lpt9",
            }
            for part in parts
        ):
            raise PathContractError(f"path contains a reserved device name: {raw!r}")
        if (
            raw == GENERATED_DESCRIPTION_ROOT
            or raw.startswith(GENERATED_DESCRIPTION_ROOT + "/")
        ) and not allow_generated_descriptions:
            raise PathContractError(f"path uses the system description namespace: {raw!r}")
        if raw in exact:
            raise PathContractError(f"duplicate path: {raw!r}")
        alias = _collision_key(raw)
        prior = aliases.get(alias)
        if prior is not None:
            raise PathContractError(
                f"path collides after Unicode normalization and case folding: "
                f"{prior!r} and {raw!r}"
            )
        exact.add(raw)
        aliases[alias] = raw
        accepted.append(raw)
    return tuple(accepted)


def safe_destination(root: Path, relative_path: str) -> Path:
    validate_vault_paths(
        [relative_path],
        allow_generated_descriptions=True,
    )
    root = root.resolve()
    destination = root.joinpath(*PurePosixPath(relative_path).parts)
    resolved_parent = destination.parent.resolve()
    if resolved_parent != root and root not in resolved_parent.parents:
        raise PathContractError(f"path escapes export root: {relative_path!r}")
    return destination


def _write_file(root: Path, spec: FileSpec, *, mtime_ns: int) -> None:
    destination = safe_destination(root, spec.path)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(spec.content)
    destination.chmod(0o644)
    os.utime(destination, ns=(mtime_ns, mtime_ns))


def infer_media_type(path: str) -> str:
    lower = path.casefold()
    if lower.endswith((".sqlite", ".sqlite3", ".db")):
        return "application/vnd.sqlite3"
    if lower.endswith(".md"):
        return "text/markdown"
    if lower.endswith(".sql"):
        return "application/sql"
    if lower.endswith(".ics"):
        return "text/calendar"
    guessed, _ = mimetypes.guess_type(path)
    return guessed or "application/octet-stream"


def make_png(width: int, height: int, *, palette: Sequence[tuple[int, int, int]]) -> bytes:
    """Create a small deterministic RGB PNG using only the standard library."""

    if width <= 0 or height <= 0 or not palette:
        raise ValueError("PNG dimensions and palette must be non-empty")
    rows = bytearray()
    stripe = max(1, width // len(palette))
    for y in range(height):
        rows.append(0)
        for x in range(width):
            index = min(len(palette) - 1, (x + y // 3) // stripe)
            red, green, blue = palette[index]
            rows.extend((red, green, blue))

    def chunk(kind: bytes, payload: bytes) -> bytes:
        body = kind + payload
        return struct.pack(">I", len(payload)) + body + struct.pack(">I", zlib.crc32(body))

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(rows), level=9))
        + chunk(b"IEND", b"")
    )


def _fixture_font(size: int, *, bold: bool = False) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    name = "DejaVuSans-Bold.ttf" if bold else "DejaVuSans.ttf"
    try:
        return ImageFont.truetype(name, size)
    except OSError:
        return ImageFont.load_default(size=size)


def _render_image(image: Image.Image) -> bytes:
    output = io.BytesIO()
    image.save(output, format="PNG", optimize=False, compress_level=9)
    return output.getvalue()


def make_receipt_png() -> bytes:
    image = Image.new("RGB", (1000, 1320), "#f6f5ef")
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle((90, 45, 910, 1275), radius=8, fill="white", outline="#a9a9a3", width=3)
    title = _fixture_font(46, bold=True)
    heading = _fixture_font(28, bold=True)
    body = _fixture_font(27)
    small = _fixture_font(22)
    draw.text((165, 105), "ALPINE LODGE SAAS-FEE", font=title, fill="#202020")
    draw.text((335, 170), "LODGING RECEIPT", font=heading, fill="#444444")
    draw.line((145, 225, 855, 225), fill="#555555", width=2)
    rows = [
        ("Receipt no.", "AL-0718-442"),
        ("Date", "2026-07-18"),
        ("Room", "204 / 1 night"),
        ("Guest", "Family lodging"),
    ]
    y = 270
    for label, value in rows:
        draw.text((150, y), label, font=body, fill="#333333")
        draw.text((510, y), value, font=body, fill="#111111")
        y += 56
    draw.line((145, y + 10, 855, y + 10), fill="#b0b0b0", width=2)
    y += 58
    charges = [
        ("Lodging", "CHF 164.00"),
        ("City tax", "CHF 13.87"),
        ("VAT", "CHF 6.83"),
    ]
    for label, value in charges:
        draw.text((150, y), label, font=body, fill="#222222")
        draw.text((645, y), value, font=body, fill="#222222")
        y += 62
    draw.line((145, y + 4, 855, y + 4), fill="#333333", width=3)
    y += 42
    draw.text((150, y), "TOTAL", font=title, fill="#111111")
    draw.text((570, y), "CHF 184.70", font=title, fill="#111111")
    y += 105
    draw.text((150, y), "PAID  VISA *4812", font=heading, fill="#245b3f")
    y += 75
    draw.text((150, y), "Thank you for staying with us.", font=small, fill="#555555")
    draw.text((150, y + 42), "Retain this receipt for expense and tax records.", font=small, fill="#555555")
    return _render_image(image)


def _dashed_line(
    draw: ImageDraw.ImageDraw,
    start: tuple[int, int],
    end: tuple[int, int],
    *,
    fill: str,
    width: int,
    dash: int = 18,
) -> None:
    x1, y1 = start
    x2, y2 = end
    distance = ((x2 - x1) ** 2 + (y2 - y1) ** 2) ** 0.5
    steps = max(1, int(distance // dash))
    for index in range(0, steps, 2):
        left = index / steps
        right = min(1.0, (index + 1) / steps)
        draw.line(
            (
                x1 + (x2 - x1) * left,
                y1 + (y2 - y1) * left,
                x1 + (x2 - x1) * right,
                y1 + (y2 - y1) * right,
            ),
            fill=fill,
            width=width,
        )


def make_world_map_png() -> bytes:
    image = Image.new("RGB", (1400, 900), "#e7efe9")
    draw = ImageDraw.Draw(image)
    title = _fixture_font(42, bold=True)
    label = _fixture_font(28, bold=True)
    small = _fixture_font(23)
    draw.text((55, 35), "FACTORY FREIGHT MAP - REVISION 2", font=title, fill="#17252d")
    draw.rectangle((55, 105, 1345, 835), outline="#385460", width=4)
    points = {
        "Faraday Works": (250, 500),
        "Landerworks": (650, 500),
        "Grand Basin": (650, 230),
        "Oil Mesa": (1080, 675),
    }
    draw.line((250, 500, 650, 500), fill="#4d6a76", width=10)
    draw.line((650, 500, 650, 230), fill="#4d6a76", width=10)
    _dashed_line(draw, (650, 230), (1080, 675), fill="#b3262e", width=10)
    draw.line((650, 500, 760, 720, 1080, 675), fill="#23834f", width=14, joint="curve")
    draw.ellipse((735, 695, 785, 745), fill="#ffffff", outline="#17693e", width=5)
    draw.text((790, 630), "S-3 southern freight checkpoint", font=small, fill="#17693e")
    blocked_x, blocked_y = 865, 452
    draw.line((blocked_x - 25, blocked_y - 25, blocked_x + 25, blocked_y + 25), fill="#b3262e", width=10)
    draw.line((blocked_x - 25, blocked_y + 25, blocked_x + 25, blocked_y - 25), fill="#b3262e", width=10)
    draw.text((895, 420), "DIRECT RAIL BLOCKED", font=small, fill="#9b1720")
    for name, (x, y) in points.items():
        draw.ellipse((x - 28, y - 28, x + 28, y + 28), fill="#f2b84b", outline="#263f4a", width=5)
        draw.text((x - 105, y + 42), name, font=label, fill="#17252d")
    draw.text((80, 790), "Green: approved freight route", font=small, fill="#17693e")
    draw.text((455, 790), "Red dashed: unavailable rail", font=small, fill="#9b1720")
    return _render_image(image)


def make_office_layout_png() -> bytes:
    image = Image.new("RGB", (1200, 900), "#f4f1e8")
    draw = ImageDraw.Draw(image)
    title = _fixture_font(40, bold=True)
    label = _fixture_font(28, bold=True)
    small = _fixture_font(22)
    draw.text((60, 30), "OFFICE LAYOUT - OBSERVED SURFACES", font=title, fill="#26343b")
    draw.rectangle((110, 120, 1090, 810), fill="#ffffff", outline="#303c42", width=7)
    draw.text((525, 82), "NORTH", font=small, fill="#303c42")
    draw.text((1110, 450), "EAST", font=small, fill="#303c42")
    draw.text((30, 450), "WEST", font=small, fill="#303c42")
    draw.rectangle((335, 145, 820, 285), fill="#8ca3ad", outline="#3a515b", width=4)
    draw.text((500, 190), "DESK", font=label, fill="#17252d")
    draw.rectangle((1038, 330, 1092, 615), fill="#9fd1ef", outline="#30739b", width=4)
    draw.text((855, 365), "EAST WINDOW", font=small, fill="#245d7d")
    draw.rectangle((108, 555, 155, 610), fill="#f4cb53", outline="#765d12", width=4)
    draw.text((170, 562), "NETWORK OUTLET", font=small, fill="#5b4910")
    draw.rectangle((845, 145, 1030, 755), fill="#f9e5bf", outline="#d99f3f", width=3)
    for y in range(165, 750, 35):
        draw.line((855, y, 1015, y + 25), fill="#d99f3f", width=2)
    draw.text((857, 690), "KEEP CLEAR", font=small, fill="#9b5c0d")
    draw.line((130, 582, 230, 300, 335, 250), fill="#2d7f4f", width=12, joint="curve")
    draw.ellipse((210, 277, 250, 317), fill="#ffffff", outline="#2d7f4f", width=4)
    draw.text((255, 280), "Cable anchor A3", font=small, fill="#24683f")
    draw.text((170, 735), "Recommended cable path: west wall to north desk", font=small, fill="#24683f")
    return _render_image(image)


def make_pdf(lines: Sequence[str]) -> bytes:
    """Create a deterministic, valid one-page PDF with machine-readable text."""

    escaped = [
        line.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")
        for line in lines
    ]
    operations = ["BT", "/F1 11 Tf", "72 740 Td"]
    for index, line in enumerate(escaped):
        if index:
            operations.append("0 -18 Td")
        operations.append(f"({line}) Tj")
    operations.append("ET")
    stream = ("\n".join(operations) + "\n").encode("ascii")
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
            b"/Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
        ),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        b"<< /Length "
        + str(len(stream)).encode("ascii")
        + b" >>\nstream\n"
        + stream
        + b"endstream",
    ]
    output = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    offsets = [0]
    for number, body in enumerate(objects, start=1):
        offsets.append(len(output))
        output.extend(f"{number} 0 obj\n".encode("ascii"))
        output.extend(body)
        output.extend(b"\nendobj\n")
    xref_offset = len(output)
    output.extend(f"xref\n0 {len(objects) + 1}\n".encode("ascii"))
    output.extend(b"0000000000 65535 f \n")
    for offset in offsets[1:]:
        output.extend(f"{offset:010d} 00000 n \n".encode("ascii"))
    output.extend(
        (
            f"trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\n"
            f"startxref\n{xref_offset}\n%%EOF\n"
        ).encode("ascii")
    )
    return bytes(output)


def make_sqlite(revision: int) -> bytes:
    """Build a deterministic SQLite factory snapshot."""

    if revision not in {1, 2}:
        raise ValueError("fixture SQLite revision must be 1 or 2")
    rows = [
        ("Faraday Works", "west", 120),
        ("Landerworks", "central", 75),
        ("Grand Basin", "north", 30 if revision == 1 else 45),
    ]
    if revision == 2:
        rows.append(("Oil Mesa", "southeast", 18))
    routes = [
        ("Faraday Works", "Landerworks", "rail", "active", 60),
        ("Landerworks", "Grand Basin", "truck", "planned", 20),
    ]
    if revision == 2:
        routes.append(("Grand Basin", "Oil Mesa", "rail", "blocked", 0))
    with tempfile.TemporaryDirectory(prefix="brunn-state-sqlite-") as temporary:
        path = Path(temporary) / "factory.sqlite"
        database = sqlite3.connect(path)
        try:
            database.execute("PRAGMA page_size=4096")
            database.execute("PRAGMA journal_mode=OFF")
            database.execute("PRAGMA synchronous=OFF")
            database.execute("PRAGMA auto_vacuum=NONE")
            database.execute("PRAGMA encoding='UTF-8'")
            database.execute(f"PRAGMA user_version={revision}")
            database.execute(
                "CREATE TABLE sites(name TEXT PRIMARY KEY, region TEXT NOT NULL, ingots INTEGER NOT NULL)"
            )
            database.execute(
                "CREATE TABLE routes(origin TEXT NOT NULL, destination TEXT NOT NULL, "
                "mode TEXT NOT NULL, status TEXT NOT NULL, throughput INTEGER NOT NULL, "
                "PRIMARY KEY(origin, destination))"
            )
            database.executemany("INSERT INTO sites VALUES (?, ?, ?)", rows)
            database.executemany("INSERT INTO routes VALUES (?, ?, ?, ?, ?)", routes)
            database.commit()
            database.execute("VACUUM")
            database.commit()
        finally:
            database.close()
        return path.read_bytes()


def make_zip(entries: Mapping[str, bytes]) -> bytes:
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for path, content in sorted(entries.items()):
            validate_vault_paths([path])
            info = zipfile.ZipInfo(path, date_time=(1980, 1, 1, 0, 0, 0))
            info.create_system = 3
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = (stat.S_IFREG | 0o644) << 16
            archive.writestr(info, content)
    return output.getvalue()


def render_generated_description(
    *,
    original_path: str,
    content: bytes,
    media_type: str,
    asset_version: int,
    title: str,
    summary: str,
    facts: Sequence[str],
    limitations: Sequence[str],
) -> bytes:
    metadata = {
        "authoritative": False,
        "describes_asset_version": asset_version,
        "describes_path": original_path,
        "describes_sha256": f"sha256:{sha256_bytes(content)}",
        "evidence_kind": "derived_non_authoritative",
        "generated": True,
        "media_type": media_type,
        "size_bytes": len(content),
    }
    lines = [
        "# " + title,
        "",
        "> Generated, non-authoritative description of a losslessly retained native file.",
        "> Treat the native bytes and source-backed Markdown as authority.",
        "",
        "```json brunn-state-asset-description",
        json.dumps(metadata, indent=2, sort_keys=True, ensure_ascii=True),
        "```",
        "",
        "## Summary",
        "",
        summary,
        "",
        "## Extracted Facts",
        "",
    ]
    lines.extend(f"- {fact}" for fact in facts)
    lines.extend(["", "## Limitations", ""])
    lines.extend(f"- {limitation}" for limitation in limitations)
    lines.append("")
    return "\n".join(lines).encode("utf-8")


def parse_generated_description(content: bytes) -> dict[str, Any]:
    text = content.decode("utf-8")
    marker = "```json brunn-state-asset-description\n"
    start = text.find(marker)
    if start < 0:
        raise ValueError("generated description lacks metadata block")
    start += len(marker)
    end = text.find("\n```", start)
    if end < 0:
        raise ValueError("generated description metadata block is unterminated")
    parsed = json.loads(text[start:end])
    if not isinstance(parsed, dict):
        raise ValueError("generated description metadata must be an object")
    return parsed


def _text(path: str, content: str, media_type: str, role: str) -> FileSpec:
    return FileSpec(path, content.encode("utf-8"), media_type, role)


def _binary_with_description(
    *,
    path: str,
    content: bytes,
    media_type: str,
    asset_version: int,
    title: str,
    summary: str,
    facts: Sequence[str],
    limitations: Sequence[str] = ("Verify the native file before consequential action.",),
) -> tuple[FileSpec, FileSpec]:
    description_path = generated_description_path(path)
    binary = FileSpec(path, content, media_type, "native_binary")
    description = FileSpec(
        description_path,
        render_generated_description(
            original_path=path,
            content=content,
            media_type=media_type,
            asset_version=asset_version,
            title=title,
            summary=summary,
            facts=facts,
            limitations=limitations,
        ),
        "text/markdown",
        "generated_description",
        describes_path=path,
    )
    return binary, description


def fixture_specs() -> tuple[
    dict[str, dict[str, tuple[FileSpec, ...]]],
    dict[str, dict[str, tuple[str, ...]]],
]:
    receipt = make_receipt_png()
    world_map = make_world_map_png()
    office_image = make_office_layout_png()
    shared_diagram = make_png(
        64,
        64,
        palette=((240, 244, 247), (41, 98, 125), (202, 113, 74), (59, 130, 96)),
    )
    proposal = make_pdf(
        [
            "UNBOOKED PROPOSAL",
            "Lake transfer: 2026-08-12 10:00",
            "No confirmation number",
        ]
    )
    confirmation = make_pdf(
        [
            "BOOKING CONFIRMATION",
            "Lake transfer: 2026-08-13 09:20",
            "Confirmation LK-2048",
        ]
    )
    factory_v1 = make_sqlite(1)
    factory_v2 = make_sqlite(2)
    release_zip = make_zip(
        {
            "manifest.json": json.dumps(
                {
                    "build": "cs-alpha-17",
                    "commit": "0123456789abcdef",
                    "restore_gate": "pending",
                },
                sort_keys=True,
            ).encode("ascii"),
            "release-notes.md": b"# Alpha 17\n\nAsset retrieval is included.\n",
            "checksums.txt": b"fixture-only deterministic archive\n",
        }
    )

    def personal(revision: int) -> list[FileSpec]:
        current_db = factory_v1 if revision == 1 else factory_v2
        db_version = revision
        schedule_date = "2026-12-12T09:00:00-08:00" if revision == 1 else "2026-12-13T08:30:00-08:00"
        schedule_status = (
            "Crystal Slalom is scheduled for December 12 at 09:00."
            if revision == 1
            else "Crystal Slalom moved to December 13 at 08:30 because of weather."
        )
        trip_booking = (
            "The lake transfer on August 12 is proposed and not booked."
            if revision == 1
            else "The lake transfer is booked for August 13 at 09:20 under LK-2048."
        )
        files = [
            _text(
                "Trips/Switzerland/Switzerland Trip.md",
                "# Switzerland Trip\n\n"
                "Athlete camp: August 3-15, 2026.\n\n"
                "Family travel window: August 8-16, 2026.\n\n"
                f"{trip_booking}\n",
                "text/markdown",
                "source_markdown",
            ),
            _text(
                "Trips/Switzerland/Expenses/Expense Ledger.md",
                "# Expense Ledger\n\n"
                "The Alpine Lodge Saas-Fee receipt dated 2026-07-18 was manually "
                "verified on 2026-07-19. Total CHF 184.70; VAT CHF 6.83.\n",
                "text/markdown",
                "source_markdown",
            ),
            _text(
                "Areas/Skiing/Kids Team/Schedule.md",
                "# Kids Ski Team Schedule\n\n"
                f"{schedule_status}\n\n"
                "Registration is confirmed. RSVP deadline is December 5. "
                "Wax pickup is December 11 at 18:00.\n",
                "text/markdown",
                "source_markdown",
            ),
            _text(
                "Areas/Skiing/Kids Team/team-schedule.ics",
                "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n"
                "UID:crystal-slalom-2026@example.test\r\n"
                f"DTSTART:{schedule_date}\r\n"
                "SUMMARY:Crystal Slalom\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
                "text/calendar",
                "text_asset",
            ),
            _text(
                "Topics/StarRupture/Factory Notes.md",
                "# Factory Notes\n\n"
                "Faraday Works is west of Landerworks. Grand Basin is north. "
                "Oil Mesa is southeast and its direct rail is blocked. "
                "Use the southern freight route for the next delivery.\n",
                "text/markdown",
                "source_markdown",
            ),
            _text(
                "Data/StarRupture/factory-baseline.sql",
                "CREATE TABLE sites(name TEXT, ingots INTEGER);\n"
                "INSERT INTO sites VALUES ('Faraday Works',120),"
                "('Landerworks',75),('Grand Basin',30);\n"
                "-- Baseline revision 1; superseded by factory.sqlite revision 2.\n",
                "application/sql",
                "text_asset",
            ),
            _text(
                "Rooms/Office/Layout Constraints.md",
                "# Office Layout Constraints\n\n"
                "The desk is on the north wall, the window is east, and the usable "
                "network outlet is west. Keep the east walkway clear and route the "
                "network cable along the west wall.\n",
                "text/markdown",
                "source_markdown",
            ),
        ]
        files.extend(
            _binary_with_description(
                path="Trips/Switzerland/Expenses/alpine-lodge-receipt.png",
                content=receipt,
                media_type="image/png",
                asset_version=1,
                title="Alpine Lodge Receipt",
                summary="Scanned lodging receipt retained as expense and tax evidence.",
                facts=(
                    "Merchant: Alpine Lodge Saas-Fee.",
                    "Date: 2026-07-18.",
                    "Total: CHF 184.70.",
                    "VAT: CHF 6.83.",
                ),
            )
        )
        files.extend(
            _binary_with_description(
                path="Records/Taxes/2026/Receipts/alpine-lodge-receipt-copy.png",
                content=receipt,
                media_type="image/png",
                asset_version=1,
                title="Tax Receipt Copy",
                summary="Byte-identical retained copy of the Switzerland lodging receipt.",
                facts=("This copy has the same native SHA-256 as the trip expense receipt.",),
            )
        )
        files.extend(
            _binary_with_description(
                path="Data/StarRupture/factory.sqlite",
                content=current_db,
                media_type="application/vnd.sqlite3",
                asset_version=db_version,
                title="Current Factory Database",
                summary="Queryable factory sites and routes snapshot.",
                facts=(
                    f"This is native revision {revision}; exact row values are intentionally omitted.",
                    "The Grand Basin to Oil Mesa rail is blocked in revision 2."
                    if revision == 2
                    else "The Landerworks to Grand Basin truck route is planned.",
                ),
                limitations=("Run SQLite queries against the native database for exact answers.",),
            )
        )
        files.extend(
            _binary_with_description(
                path="Topics/StarRupture/Maps/world-map.png",
                content=world_map,
                media_type="image/png",
                asset_version=1,
                title="StarRupture World Map",
                summary="Visual reference for named factory regions and freight routing.",
                facts=(
                    "Faraday Works is west of Landerworks.",
                    "Grand Basin is north of Landerworks.",
                    "Oil Mesa is southeast; the direct rail is blocked.",
                ),
                limitations=("Use Factory Notes for canonical route status.",),
            )
        )
        files.extend(
            _binary_with_description(
                path="Rooms/Office/office-layout.png",
                content=office_image,
                media_type="image/png",
                asset_version=1,
                title="Office Layout Image",
                summary="Room image used to validate desk, window, outlet, and walkway constraints.",
                facts=(
                    "Desk: north wall.",
                    "Window: east wall.",
                    "Network outlet: west wall.",
                ),
                limitations=("The image does not establish hidden wiring behind the walls.",),
            )
        )
        files.extend(
            _binary_with_description(
                path="Reference/Shared Architecture.png",
                content=shared_diagram,
                media_type="image/png",
                asset_version=1,
                title="Shared Architecture Diagram",
                summary="A shared reference copied into personal and work scopes.",
                facts=("Native bytes intentionally duplicate the work-scope diagram.",),
            )
        )
        if revision == 1:
            files.extend(
                _binary_with_description(
                    path="Trips/Switzerland/Proposals/Lake Transfer Proposal.pdf",
                    content=proposal,
                    media_type="application/pdf",
                    asset_version=1,
                    title="Lake Transfer Proposal",
                    summary="An unbooked proposal retained in revision history.",
                    facts=(
                        "Proposed time: 2026-08-12 10:00.",
                        "No confirmation number was issued.",
                    ),
                )
            )
        else:
            files.extend(
                _binary_with_description(
                    path="Trips/Switzerland/Confirmations/Lake Transfer LK-2048.pdf",
                    content=confirmation,
                    media_type="application/pdf",
                    asset_version=1,
                    title="Lake Transfer Confirmation",
                    summary="The current booked lake transfer confirmation.",
                    facts=(
                        "Booked time: 2026-08-13 09:20.",
                        "Confirmation: LK-2048.",
                    ),
                )
            )
        return files

    def work(_: int) -> list[FileSpec]:
        files = [
            _text(
                "Projects/Brunn State/Alpha Checklist.md",
                "# Brunn State Alpha Checklist\n\n"
                "Candidate build: cs-alpha-17 at commit 0123456789abcdef.\n\n"
                "Asset retrieval passed. The off-host restore gate is still pending, "
                "so the candidate is not launch-ready.\n",
                "text/markdown",
                "source_markdown",
            )
        ]
        files.extend(
            _binary_with_description(
                path="Projects/Brunn State/Artifacts/cs-alpha-17.zip",
                content=release_zip,
                media_type="application/zip",
                asset_version=1,
                title="Brunn State Alpha 17 Artifact",
                summary="Deterministic release archive for the owner-alpha candidate.",
                facts=(
                    "Build: cs-alpha-17.",
                    "Commit: 0123456789abcdef.",
                    "Restore gate: pending.",
                ),
                limitations=("Inspect and verify the ZIP manifest before deployment.",),
            )
        )
        files.extend(
            _binary_with_description(
                path="Reference/Shared Architecture.png",
                content=shared_diagram,
                media_type="image/png",
                asset_version=1,
                title="Shared Architecture Diagram",
                summary="The work-scope copy of a shared architecture reference.",
                facts=("Native bytes intentionally duplicate the personal-scope diagram.",),
            )
        )
        return files

    snapshots: dict[str, dict[str, tuple[FileSpec, ...]]] = {}
    for revision in (1, 2):
        snapshots[f"r{revision}"] = {
            "personal": tuple(sorted(personal(revision), key=lambda item: item.path)),
            "work": tuple(sorted(work(revision), key=lambda item: item.path)),
        }
    proposal_path = "Trips/Switzerland/Proposals/Lake Transfer Proposal.pdf"
    deletions = {
        "r1": {"personal": (), "work": ()},
        "r2": {
            "personal": (
                proposal_path,
                generated_description_path(proposal_path),
            ),
            "work": (),
        },
    }
    return snapshots, deletions


def directory_manifest(root: Path) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    for path in sorted(item for item in root.rglob("*") if item.is_file()):
        relative = path.relative_to(root).as_posix()
        metadata = path.stat()
        records.append(
            {
                "path": relative,
                "sha256": sha256_file(path),
                "size_bytes": metadata.st_size,
                "mode": stat.S_IMODE(metadata.st_mode),
            }
        )
    return records


def generate_fixture(destination: Path) -> dict[str, Any]:
    destination = destination.expanduser().resolve()
    if destination.exists() and any(destination.iterdir()):
        raise FileExistsError(f"fixture destination is not empty: {destination}")
    destination.mkdir(parents=True, exist_ok=True)
    snapshots, deletions = fixture_specs()
    manifest: dict[str, Any] = {
        "format": FORMAT_VERSION,
        "logical_identity_contract": {
            "key": [
                "user_id",
                "scope",
                "path",
            ],
            "same_path_changed_bytes": "new_version_same_asset",
            "same_bytes_distinct_path_or_scope": "distinct_asset_shared_physical_blob",
            "missing_from_later_snapshot": "retain_until_explicit_deletion",
        },
        "export_expectations": {
            "current": {
                "paths": "original logical paths",
                "versions": "active head only",
                "deleted": "excluded",
                "bytes": "exact SHA-256 match",
            },
            "history": {
                "paths": "manifest maps every version to its original logical path",
                "versions": "all retained versions",
                "deleted": "included with tombstone revision",
                "bytes": "content-addressed blobs with exact SHA-256 match",
            },
        },
        "snapshots": {},
        "unsafe_path_cases": {
            "individual": [
                "../escape.md",
                "/absolute/path.md",
                "C:\\vault\\file.md",
                "safe/../escape.md",
                "bad\u0000name.md",
                "folder/CON.txt",
                "folder/trailing-dot.",
                f"{GENERATED_DESCRIPTION_ROOT}/user-owned.md",
            ],
            "collision_sets": [
                ["Notes/Readme.md", "Notes/README.md"],
                ["People/Caf\u00e9.md", "People/Cafe\u0301.md"],
            ],
        },
    }
    for revision, scopes in snapshots.items():
        revision_record: dict[str, Any] = {"scopes": {}, "explicit_deletions": deletions[revision]}
        for scope, specs in scopes.items():
            paths = [spec.path for spec in specs]
            validate_vault_paths(paths, allow_generated_descriptions=True)
            root = destination / "snapshots" / revision / scope
            for spec in specs:
                _write_file(
                    root,
                    spec,
                    mtime_ns=FIXTURE_EPOCH_NS + (1 if revision == "r2" else 0),
                )
                if spec.role != "generated_description":
                    _write_file(
                        destination / "evaluation" / revision / scope,
                        spec,
                        mtime_ns=FIXTURE_EPOCH_NS + (1 if revision == "r2" else 0),
                    )
            revision_record["scopes"][scope] = {
                "root": root.relative_to(destination).as_posix(),
                "evaluation_root": (
                    destination / "evaluation" / revision / scope
                ).relative_to(destination).as_posix(),
                "files": [
                    {
                        "path": spec.path,
                        "sha256": sha256_bytes(spec.content),
                        "size_bytes": len(spec.content),
                        "media_type": spec.media_type,
                        "role": spec.role,
                        "describes_path": spec.describes_path,
                    }
                    for spec in specs
                ],
            }
        manifest["snapshots"][revision] = revision_record
    manifest_path = destination / "fixture-manifest.json"
    manifest_path.write_text(
        json.dumps(manifest, indent=2, sort_keys=True, ensure_ascii=True) + "\n",
        encoding="utf-8",
    )
    manifest_path.chmod(0o644)
    os.utime(manifest_path, ns=(FIXTURE_EPOCH_NS, FIXTURE_EPOCH_NS))
    return manifest


class ReferenceAssetLedger:
    """Small executable oracle for binary lifecycle and export semantics."""

    def __init__(self, *, user_id: str = "user:fixture") -> None:
        self.user_id = user_id
        self.physical_blobs: dict[str, bytes] = {}
        self.assets: dict[tuple[str, str], LogicalAsset] = {}
        self.revisions: list[str] = []

    def import_snapshot(
        self,
        *,
        credential: Credential,
        scope: str,
        snapshot_root: Path,
        revision: str,
        explicit_deletions: Iterable[str] = (),
    ) -> None:
        credential.require("asset:import", scope)
        files = sorted(path for path in snapshot_root.rglob("*") if path.is_file())
        paths = [path.relative_to(snapshot_root).as_posix() for path in files]
        validate_vault_paths(paths, allow_generated_descriptions=True)
        deletion_paths = validate_vault_paths(
            explicit_deletions,
            allow_generated_descriptions=True,
        )
        for path, relative in zip(files, paths, strict=True):
            content = path.read_bytes()
            digest = sha256_bytes(content)
            self.physical_blobs.setdefault(digest, content)
            key = (scope, relative)
            logical = self.assets.get(key)
            if logical is None:
                asset_id = uuid.uuid5(
                    ASSET_NAMESPACE,
                    f"{self.user_id}\0{scope}\0{relative}",
                )
                logical = LogicalAsset(
                    asset_ref=f"asset:{asset_id}",
                    user_id=self.user_id,
                    scope=scope,
                    path=relative,
                )
                self.assets[key] = logical
            if not logical.versions or logical.current.content_hash != digest:
                logical.versions.append(
                    AssetVersion(
                        version=len(logical.versions) + 1,
                        content_hash=digest,
                        size_bytes=len(content),
                        media_type=infer_media_type(relative),
                        imported_revision=revision,
                    )
                )
            logical.active = True
            logical.deleted_revision = None
        for relative in deletion_paths:
            logical = self.assets.get((scope, relative))
            if logical is None:
                raise KeyError(f"cannot delete unknown logical path: {scope}/{relative}")
            logical.active = False
            logical.deleted_revision = revision
        if revision not in self.revisions:
            self.revisions.append(revision)

    def delete(
        self,
        *,
        credential: Credential,
        scope: str,
        path: str,
        revision: str,
    ) -> None:
        credential.require("asset:delete", scope)
        validate_vault_paths([path], allow_generated_descriptions=True)
        logical = self.assets[(scope, path)]
        logical.active = False
        logical.deleted_revision = revision

    def list_assets(self, *, credential: Credential, scope: str) -> list[dict[str, Any]]:
        credential.require("asset:list", scope)
        return [
            self._metadata(logical)
            for logical in sorted(self.assets.values(), key=lambda item: item.path)
            if logical.scope == scope and logical.active
        ]

    def metadata(
        self,
        *,
        credential: Credential,
        scope: str,
        asset_ref: str,
    ) -> dict[str, Any]:
        credential.require("asset:metadata", scope)
        logical = self._asset(asset_ref)
        if logical.scope != scope or not logical.active:
            raise KeyError(asset_ref)
        return self._metadata(logical)

    def download(
        self,
        *,
        credential: Credential,
        scope: str,
        asset_ref: str,
        version: int | None = None,
        range_header: str | None = None,
    ) -> DownloadResult:
        credential.require("asset:download", scope)
        logical = self._asset(asset_ref)
        if logical.scope != scope:
            raise KeyError(asset_ref)
        selected_version = version or logical.current.version
        try:
            asset_version = logical.versions[selected_version - 1]
        except (IndexError, TypeError):
            raise KeyError(f"{asset_ref}/versions/{selected_version}") from None
        content = self.physical_blobs[asset_version.content_hash]
        if range_header is None:
            return DownloadResult(
                status=200,
                content=content,
                content_hash=asset_version.content_hash,
                content_range=None,
            )
        start, end = parse_byte_range(range_header, len(content))
        return DownloadResult(
            status=206,
            content=content[start : end + 1],
            content_hash=asset_version.content_hash,
            content_range=f"bytes {start}-{end}/{len(content)}",
        )

    def searchable_markdown(self, *, credential: Credential, scope: str) -> list[str]:
        credential.require("asset:list", scope)
        return [
            logical.path
            for logical in sorted(self.assets.values(), key=lambda item: item.path)
            if logical.scope == scope
            and logical.active
            and logical.path.casefold().endswith(".md")
        ]

    def authoritative_markdown(self, *, credential: Credential, scope: str) -> list[str]:
        return [
            path
            for path in self.searchable_markdown(credential=credential, scope=scope)
            if not path.startswith(GENERATED_DESCRIPTION_ROOT + "/")
        ]

    def export_current(self, *, credential: Credential, destination: Path) -> dict[str, Any]:
        destination = destination.resolve()
        current_root = destination / "current"
        records: list[dict[str, Any]] = []
        for logical in sorted(self.assets.values(), key=lambda item: (item.scope, item.path)):
            if not logical.active:
                continue
            credential.require("export:read", logical.scope)
            version = logical.current
            output = safe_destination(current_root / logical.scope, logical.path)
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(self.physical_blobs[version.content_hash])
            output.chmod(0o644)
            records.append(
                {
                    "asset_ref": logical.asset_ref,
                    "scope": logical.scope,
                    "path": logical.path,
                    "version": version.version,
                    "sha256": version.content_hash,
                    "size_bytes": version.size_bytes,
                    "state": "active",
                }
            )
        manifest = {
            "format": FORMAT_VERSION,
            "mode": "current",
            "records": records,
        }
        destination.mkdir(parents=True, exist_ok=True)
        (destination / "export-manifest.json").write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        return manifest

    def export_history(self, *, credential: Credential, destination: Path) -> dict[str, Any]:
        destination = destination.resolve()
        blob_root = destination / "blobs"
        records: list[dict[str, Any]] = []
        for logical in sorted(self.assets.values(), key=lambda item: (item.scope, item.path)):
            credential.require("export:read", logical.scope)
            versions = []
            for version in logical.versions:
                blob_path = blob_root / version.content_hash
                blob_path.parent.mkdir(parents=True, exist_ok=True)
                if not blob_path.exists():
                    blob_path.write_bytes(self.physical_blobs[version.content_hash])
                    blob_path.chmod(0o644)
                versions.append(
                    {
                        "version": version.version,
                        "sha256": version.content_hash,
                        "size_bytes": version.size_bytes,
                        "media_type": version.media_type,
                        "imported_revision": version.imported_revision,
                        "blob_path": blob_path.relative_to(destination).as_posix(),
                    }
                )
            records.append(
                {
                    "asset_ref": logical.asset_ref,
                    "scope": logical.scope,
                    "path": logical.path,
                    "state": "active" if logical.active else "deleted",
                    "deleted_revision": logical.deleted_revision,
                    "versions": versions,
                }
            )
        manifest = {
            "format": FORMAT_VERSION,
            "mode": "history",
            "records": records,
        }
        destination.mkdir(parents=True, exist_ok=True)
        (destination / "history-manifest.json").write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        return manifest

    def _asset(self, asset_ref: str) -> LogicalAsset:
        for logical in self.assets.values():
            if logical.asset_ref == asset_ref:
                return logical
        raise KeyError(asset_ref)

    @staticmethod
    def _metadata(logical: LogicalAsset) -> dict[str, Any]:
        version = logical.current
        return {
            "asset_ref": logical.asset_ref,
            "scope": logical.scope,
            "path": logical.path,
            "version": version.version,
            "content_hash": f"sha256:{version.content_hash}",
            "size_bytes": version.size_bytes,
            "media_type": version.media_type,
        }


def parse_byte_range(header: str, size_bytes: int) -> tuple[int, int]:
    if size_bytes <= 0 or not header.startswith("bytes="):
        raise RangeContractError("only satisfiable byte ranges are supported")
    raw = header[6:]
    if not raw or "," in raw or "-" not in raw:
        raise RangeContractError("exactly one byte range is required")
    start_text, end_text = raw.split("-", 1)
    if not start_text:
        try:
            suffix = int(end_text)
        except ValueError:
            raise RangeContractError("suffix range is invalid") from None
        if suffix <= 0:
            raise RangeContractError("suffix range must be positive")
        return max(0, size_bytes - suffix), size_bytes - 1
    try:
        start = int(start_text)
        end = size_bytes - 1 if not end_text else int(end_text)
    except ValueError:
        raise RangeContractError("byte range is invalid") from None
    if start < 0 or start >= size_bytes or end < start:
        raise RangeContractError("byte range is unsatisfiable")
    return start, min(end, size_bytes - 1)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path, help="empty output directory")
    parser.add_argument(
        "--replace",
        action="store_true",
        help="remove an existing fixture directory before generation",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    destination = args.destination.expanduser().resolve()
    if args.replace and destination.exists():
        shutil.rmtree(destination)
    manifest = generate_fixture(destination)
    summary = {
        "destination": str(destination),
        "format": manifest["format"],
        "snapshot_count": len(manifest["snapshots"]),
        "file_count": sum(
            len(scope["files"])
            for revision in manifest["snapshots"].values()
            for scope in revision["scopes"].values()
        ),
    }
    print(json.dumps(summary, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
