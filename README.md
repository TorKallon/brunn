<p align="center">
  <img src="assets/brand/brunn-well-1024.png" width="72" height="72" alt="" />
</p>

<h1 align="center">brunn</h1>

<p align="center"><em>The well your agents draw from.</em></p>

Brunn is a durable workspace for people and AI agents. It keeps source
material, project state, tasks, files, and credentials in one shared place so
work can continue across conversations, clients, and machines.

The name is pronounced **brunn** — one syllable, /brʊn/. It comes from the Old
Norse *brunnr*, meaning “well.” The hosted service lives at
[brunn.ai](https://brunn.ai).

## What Brunn does

- **Keeps durable memory.** Markdown, source references, versions, and
  provenance remain available after an agent session ends.
- **Finds useful evidence.** Agents can use exact and keyword search, with
  optional semantic search, then read the original source before answering or
  acting.
- **Makes work resumable.** Checkpoints record the current state, decisions,
  open questions, and next actions. A change feed shows what happened later.
- **Supports real work.** Brunn includes projects, tasks, briefings, published
  documents, notifications, and optional agent messaging and location
  presence.
- **Handles files safely.** Binary assets keep exact hashes and versioned
  object storage. Import, export, backup, and restore paths keep the workspace
  portable.
- **Protects credentials.** An encrypted secret vault keeps values outside the
  searchable memory corpus, exports, logs, and object storage. Scoped
  credentials control who can read or change data.
- **Works across clients.** Agents connect through an OAuth-protected hosted
  MCP server, a local MCP adapter, or the HTTP API. People use the web control
  plane and native iPhone app.

## How it works

The core is a Rust API and background worker backed by PostgreSQL, pgvector,
and S3-compatible object storage. A TypeScript MCP gateway connects agent
clients, a React app provides the web interface, and a SwiftUI app provides the
iPhone interface. Docker Compose runs the local server stack.

Markdown and stored file bytes are the source of truth. Search indexes,
embeddings, dashboards, and other views can be rebuilt from that data.

## Run locally

You need Docker with Compose and BuildKit. An OpenAI API key is required for
semantic indexing and model-assisted features.

```bash
cp .env.example .env
# Replace every placeholder in .env with a local value.
chmod 600 .env
make config
make up
```

Open the web app at [http://localhost:13110](http://localhost:13110). The API
health endpoints are:

```bash
curl -fsS http://127.0.0.1:18110/health
curl -fsS http://127.0.0.1:18110/ready
```

PostgreSQL, MinIO, and the API bind to localhost. See
[`docs/Operations.md`](docs/Operations.md) for ports, migrations, backups,
production configuration, and troubleshooting.

## Test

The full test suite also needs Rust 1.96, Node.js 24, and Python 3 with Pillow.

```bash
cargo test --manifest-path apps/api/Cargo.toml --all-targets --all-features
python3 -m unittest discover -s tests -v
(cd apps/web && npm ci && npm run build && npm test)
(cd apps/mcp && npm ci && npm run build && npm test)
```

## Repository guide

- `apps/api` — Rust API, worker, migrations, and command-line tools
- `apps/mcp` — local and hosted MCP adapters
- `apps/web` — React web control plane
- `apps/ios` — native SwiftUI app
- `infra` and `deploy` — storage, database, observability, and hosting
- `eval` and `tests` — reasoning evaluations and deterministic contracts
- `assets/brand` — canonical Still Water artwork

## Documentation

- [Architecture](docs/Architecture.md)
- [Product specification](docs/Specification.md)
- [Local and production operations](docs/Operations.md)
- [Remote MCP setup](docs/Remote%20MCP.md)
- [Still Water design system](docs/Brand.md)
