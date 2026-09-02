import {
  defineRailway,
  group,
  preserve,
  project,
  service,
  volume,
} from "railway/iac";

const releaseRuntime = {
  BRUNN_ENV: "production",
  BRUNN_BIND: "[::]:8080",
  BRUNN_LEGACY_API_ENABLED: "false",
  BRUNN_EVALUATION_API_ENABLED: "false",
  BRUNN_EMBEDDING_PROVIDER: "openai",
  BRUNN_ALLOW_DEGRADED_EMBEDDINGS: "false",
  BRUNN_EMBEDDING_MODEL: "text-embedding-3-small",
  BRUNN_EMBEDDING_DIMENSIONS: "1536",
  BRUNN_CAPTURE_MODEL: "gpt-5.6",
  BRUNN_CAPTURE_MAX_OUTPUT_TOKENS: "8192",
  BRUNN_ASSET_DESCRIPTION_MODEL: "gpt-5.6",
  BRUNN_ASSET_DESCRIPTION_MAX_OUTPUT_TOKENS: "4096",
  BRUNN_ASSET_DESCRIPTION_IMAGE_DETAIL: "high",
  BRUNN_DREAM_MODEL: "gpt-5.6",
  BRUNN_DREAM_SCHEDULER_ENABLED: "false",
  BRUNN_DREAM_SCHEDULER_POLL_SECONDS: "15",
  BRUNN_DREAM_INACTIVITY_SECONDS: "60",
  BRUNN_DREAM_COOLDOWN_SECONDS: "900",
  BRUNN_DREAM_DIRTY_THRESHOLD: "10",
  BRUNN_BACKGROUND_JOB_LEASE_SECONDS: "300",
  BRUNN_MATERIALIZE_TOKEN_BUDGET: "24000",
  BRUNN_REQUEST_TIMEOUT_SECONDS: "30",
  BRUNN_TRANSFER_TIMEOUT_SECONDS: "3600",
  BRUNN_MAX_CONCURRENT_TRANSFERS: "8",
  BRUNN_READINESS_TIMEOUT_SECONDS: "5",
  BRUNN_REQUESTS_PER_MINUTE: "600",
  BRUNN_INTENTION_LEDGER: preserve(),
  BRUNN_LEXICAL_SINGLE_SCAN: preserve(),
  BRUNN_RESUME_DELTAS: preserve(),
  BRUNN_SEARCH_CHAR_CAP: preserve(),
  BRUNN_SEARCH_FAIR_SHARE: preserve(),
  BRUNN_SEARCH_TOP1_HYDRATION: preserve(),
  BRUNN_SEMANTIC_LANE: preserve(),
  BRUNN_TODOIST_SYNC_ENABLED: preserve(),
  BRUNN_MESSAGING_ENABLED: preserve(),
  BRUNN_LOCATION_PINGS_ENABLED: "true",
  BRUNN_LOCATION_PRESENCE_IN_OPEN: "true",
  BRUNN_SEMANTIC_DEADLINE_MS: "2500",
  BRUNN_SEMANTIC_QUERY_PROVIDER_TIMEOUT_MS: "5000",
  BRUNN_SEMANTIC_QUERY_CONCURRENCY: "8",
  // Requalified 2026-08-03 from the post-GIN-fix gate battery. Semantic work
  // is now opportunistic for hybrid retrieval, so it cannot inflate these
  // foreground measurements; the thresholds continue to pause backfill on
  // real exact+lexical degradation.
  // Pre-fix values 120/107 predated semantic-on and paused continuously.
  BRUNN_EMBEDDING_BACKFILL_OPEN_P95_LIMIT_MS: "500",
  BRUNN_EMBEDDING_BACKFILL_SEARCH_P95_LIMIT_MS: "350",
  BRUNN_SUPERSESSION_DEMOTION: preserve(),
  BRUNN_VERBATIM_SPANS: preserve(),
  BRUNN_ACCOUNT_EXPORT_TTL_HOURS: "24",
  BRUNN_ACCOUNT_EXPORT_TEMP_DIR: "/tmp/brunn-exports",
  BRUNN_ACCOUNT_DELETION_BACKUP_RETENTION_DAYS: "30",
  BRUNN_DATABASE_MAX_CONNECTIONS: "8",
  BRUNN_ALLOWED_ORIGINS: "https://brunn.ai",
  BRUNN_PUBLIC_URL: "https://brunn.ai",
  BRUNN_APNS_APP_ID: "com.rourkem.brunn",
  // Owner-alpha push delivery is enabled for the signed-device sandbox
  // rollout. API and worker must retain the same value.
  BRUNN_APNS_DELIVERY_ENABLED: "true",
  AUTH_EMAIL_FROM: "Brunn <login@solark.io>",
  AUTH_EMAIL_REPLY_TO: "rourkem@rourkem.com",
  BRUNN_S3_ENDPOINT: "",
  BRUNN_S3_REGION: "us-west-2",
  BRUNN_S3_BUCKET: "rourkem-brunn-alpha-us-west-2",
  BRUNN_S3_FORCE_PATH_STYLE: "false",
  BRUNN_S3_CREATE_BUCKET: "false",
  BRUNN_METRICS_ENABLED: "true",
  BRUNN_DOGSTATSD_ADDR: "datadog-agent.railway.internal:8125",
  BRUNN_METRICS_FLUSH_SECONDS: "3",
  OPENAI_BASE_URL: "https://api.openai.com/v1",
  DD_ENV: "alpha",
  DD_SERVICE: "brunn",
  DD_VERSION: preserve(),
  BRUNN_BUILD_REVISION: preserve(),
  RUST_LOG: "info,sqlx=warn",
};

const postgresData = volume("postgres-data", {
  region: "us-west2",
  sizeMB: 20000,
});

const db = service("db", {
  build: {
    builder: "DOCKERFILE",
    dockerfilePath: "infra/postgres/Dockerfile",
    watchPatterns: ["infra/postgres/**"],
  },
  replicas: { "us-west2": 1 },
  deploy: {
    drainingSeconds: 60,
    limitOverride: {
      containers: {
        cpu: 2,
        // 2026-08-02 owner-authorized memory experiment: the 1.25 GB HNSW
        // index plus the lexical working set could not stay cached in 2 GiB.
        memoryBytes: 4 * 1024 * 1024 * 1024,
      },
    },
  },
  env: {
    PORT: "5432",
    RAILWAY_RUN_UID: "0",
    BRUNN_BUILD_REVISION: preserve(),
    POSTGRES_DB: "brunn",
    POSTGRES_USER: "admin",
    PGDATA: "/var/lib/postgresql/data/pgdata",
    POSTGRES_PASSWORD: preserve(),
    APP_RW_PASSWORD: preserve(),
    APP_RO_PASSWORD: preserve(),
    POSTGRES_INITDB_ARGS:
      "--locale-provider=builtin --builtin-locale=C.UTF-8 --encoding=UTF8 --data-checksums",
    DATABASE_URL_ADMIN: preserve(),
    DATABASE_URL_RW: preserve(),
    DATABASE_URL_RO: preserve(),
  },
  volumeMounts: {
    "/var/lib/postgresql/data": postgresData,
  },
});

const dreamer = service("dreamer", {
  build: {
    builder: "DOCKERFILE",
    dockerfilePath: "apps/api/Dockerfile.dreamer",
    watchPatterns: ["apps/api/**", "apps/mcp/**"],
  },
  start: "/usr/local/bin/brunn dreamer serve",
  replicas: { "us-west2": 1 },
  deploy: {
    restartPolicyType: "ALWAYS",
    restartPolicyMaxRetries: null,
    drainingSeconds: 60,
    limitOverride: {
      containers: {
        cpu: 1,
        memoryBytes: 1024 * 1024 * 1024,
      },
    },
  },
  env: {
    BRUNN_API_URL: "http://api.railway.internal:8080",
    DREAMER_BIND: "[::]:8090",
    // Brunn credentials for the runner: `dreamer` (read_write; also
    // handed to codex through the MCP server) and `dreamer_runner` (vault
    // custody + run notifications; codex never holds it). Minted via
    // POST /credentials with an owner token; values managed outside IaC.
    DREAMER_WORKSPACE_TOKEN: preserve(),
    DREAMER_RUNNER_TOKEN: preserve(),
    // Shared secret for the api → dreamer private surface.
    DREAMER_INTERNAL_TOKEN: preserve(),
    DREAMER_CODEX_MODEL: preserve(),
  },
});

const api = service("api", {
  build: {
    builder: "DOCKERFILE",
    dockerfilePath: "apps/api/Dockerfile",
    watchPatterns: ["apps/api/**"],
  },
  start: "/usr/local/bin/brunn serve",
  healthcheck: "/ready",
  healthcheckTimeout: 300,
  replicas: { "us-west2": 2 },
  deploy: {
    restartPolicyType: "ALWAYS",
    restartPolicyMaxRetries: null,
    overlapSeconds: 30,
    drainingSeconds: 30,
    limitOverride: {
      containers: {
        cpu: 1,
        memoryBytes: 1024 * 1024 * 1024,
      },
    },
  },
  env: {
    ...releaseRuntime,
    PORT: "8080",
    DATABASE_URL_RW: db.env.DATABASE_URL_RW,
    DATABASE_URL_RO: db.env.DATABASE_URL_RO,
    BRUNN_CONTINUATION_SECRET: preserve(),
    BRUNN_NOTIFICATION_TOKEN_ENCRYPTION_KEY: preserve(),
    BRUNN_SECRET_ENCRYPTION_KEY: preserve(),
    BRUNN_S3_ACCESS_KEY: preserve(),
    BRUNN_S3_SECRET_KEY: preserve(),
    RESEND_API_KEY: preserve(),
    BRUNN_SYSLOG_ADDR: "datadog-agent.railway.internal:514",
    OPENAI_API_KEY: preserve(),
    DREAMER_INTERNAL_URL: "http://dreamer.railway.internal:8090",
    DREAMER_INTERNAL_TOKEN: dreamer.env.DREAMER_INTERNAL_TOKEN,
  },
});

const worker = service("worker", {
  build: {
    builder: "DOCKERFILE",
    dockerfilePath: "apps/api/Dockerfile",
    watchPatterns: ["apps/api/**"],
  },
  start: "/usr/local/bin/brunn worker",
  preDeploy: "/usr/local/bin/brunn migrate",
  replicas: { "us-west2": 1 },
  deploy: {
    restartPolicyType: "ALWAYS",
    restartPolicyMaxRetries: null,
    drainingSeconds: 60,
    limitOverride: {
      containers: {
        cpu: 2,
        memoryBytes: 2 * 1024 * 1024 * 1024,
      },
    },
  },
  env: {
    ...releaseRuntime,
    DD_VERSION: api.env.DD_VERSION,
    BRUNN_BUILD_REVISION: api.env.BRUNN_BUILD_REVISION,
    // The API owns activation. Reference that exact service variable so the
    // worker cannot drift to a separate preserved kill-switch value.
    BRUNN_TODOIST_SYNC_ENABLED:
      api.env.BRUNN_TODOIST_SYNC_ENABLED,
    BRUNN_MESSAGING_ENABLED:
      api.env.BRUNN_MESSAGING_ENABLED,
    DATABASE_URL_ADMIN: db.env.DATABASE_URL_ADMIN,
    DATABASE_URL_RW: db.env.DATABASE_URL_RW,
    DATABASE_URL_RO: db.env.DATABASE_URL_RO,
    BRUNN_CONTINUATION_SECRET: api.env.BRUNN_CONTINUATION_SECRET,
    BRUNN_NOTIFICATION_TOKEN_ENCRYPTION_KEY:
      api.env.BRUNN_NOTIFICATION_TOKEN_ENCRYPTION_KEY,
    BRUNN_SECRET_ENCRYPTION_KEY:
      api.env.BRUNN_SECRET_ENCRYPTION_KEY,
    BRUNN_APNS_TEAM_ID: preserve(),
    BRUNN_APNS_KEY_ID: preserve(),
    BRUNN_APNS_PRIVATE_KEY: preserve(),
    BRUNN_S3_ACCESS_KEY: api.env.BRUNN_S3_ACCESS_KEY,
    BRUNN_S3_SECRET_KEY: api.env.BRUNN_S3_SECRET_KEY,
    BRUNN_SYSLOG_ADDR: "datadog-agent.railway.internal:515",
    BRUNN_EMBEDDING_BACKFILL_FOREGROUND_STATUS_URL:
      "http://api.railway.internal:8080/health/foreground-latency",
    OPENAI_API_KEY: api.env.OPENAI_API_KEY,
  },
});

const mcp = service("mcp", {
  build: {
    builder: "DOCKERFILE",
    dockerfilePath: "apps/mcp/Dockerfile.remote",
    watchPatterns: ["apps/mcp/**"],
  },
  healthcheck: "/healthz",
  healthcheckTimeout: 300,
  replicas: { "us-west2": 1 },
  deploy: {
    // OAuth authorization and replay state are process-local, so this service
    // intentionally remains single-replica. Restart it without an exhaustion
    // limit if that sole process exits for any reason.
    restartPolicyType: "ALWAYS",
    restartPolicyMaxRetries: null,
    drainingSeconds: 30,
    limitOverride: {
      containers: {
        cpu: 0.5,
        memoryBytes: 256 * 1024 * 1024,
      },
    },
  },
  env: {
    PORT: "8080",
    BRUNN_API_URL: "http://api.railway.internal:8080",
    BRUNN_MCP_PUBLIC_URL: "https://brunn.ai",
    BRUNN_MCP_ALLOWED_ORIGINS: "https://chatgpt.com,https://brunn.ai",
    BRUNN_MESSAGING_ENABLED: api.env.BRUNN_MESSAGING_ENABLED,
    BRUNN_MCP_SEALING_KEY: preserve(),
    BRUNN_BUILD_REVISION: preserve(),
  },
});

const web = service("web", {
  build: {
    builder: "DOCKERFILE",
    dockerfilePath: "apps/web/Dockerfile.railway",
    watchPatterns: ["apps/web/**"],
  },
  healthcheck: "/api/ready",
  healthcheckTimeout: 300,
  // The Railway edge retries failed dials only when another replica exists.
  // Keep two independent ingress targets so one unreachable container cannot
  // take down static assets, OAuth discovery, MCP, and the workspace API.
  replicas: { "us-west2": 2 },
  deploy: {
    restartPolicyType: "ALWAYS",
    restartPolicyMaxRetries: null,
    overlapSeconds: 30,
    drainingSeconds: 30,
    limitOverride: {
      containers: {
        cpu: 1,
        memoryBytes: 256 * 1024 * 1024,
      },
    },
  },
  env: {
    PORT: "8080",
    BRUNN_API_HOST: api.env.RAILWAY_PRIVATE_DOMAIN,
    BRUNN_MCP_HOST: mcp.env.RAILWAY_PRIVATE_DOMAIN,
    BRUNN_DNS_RESOLVER: "[fd12::10]",
    BRUNN_BUILD_REVISION: preserve(),
  },
});

const datadog = service("datadog-agent", {
  build: {
    builder: "DOCKERFILE",
    dockerfilePath: "deploy/railway/datadog-agent/Dockerfile",
    watchPatterns: ["deploy/railway/datadog-agent/**"],
  },
  replicas: { "us-west2": 1 },
  deploy: {
    restartPolicyType: "ALWAYS",
    restartPolicyMaxRetries: null,
    limitOverride: {
      containers: {
        cpu: 1,
        memoryBytes: 512 * 1024 * 1024,
      },
    },
  },
  env: {
    DD_API_KEY: preserve(),
    DD_SITE: "datadoghq.com",
    DD_HOSTNAME: "brunn-railway-alpha",
    DD_ENV: "alpha",
    DD_VERSION: preserve(),
    DD_TAGS: "project:brunn platform:railway",
    DD_BIND_HOST: "0.0.0.0",
    DD_DOGSTATSD_NON_LOCAL_TRAFFIC: "true",
    DD_DOGSTATSD_TAG_CARDINALITY: "low",
    DD_LOGS_ENABLED: "true",
    DD_APM_ENABLED: "false",
    DD_PROCESS_CONFIG_PROCESS_COLLECTION_ENABLED: "false",
    DD_CONTAINER_EXCLUDE: "name:datadog-agent",
  },
});

export default defineRailway(() => {
  const application = group("Application", [web, api, worker, mcp, dreamer]);
  const storage = group("Storage", [db, postgresData]);
  const operations = group("Operations", [datadog]);

  return project("Brunn", {
    resources: [application, storage, operations],
  });
});
