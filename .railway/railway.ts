import {
  defineRailway,
  group,
  preserve,
  project,
  service,
  volume,
} from "railway/iac";

const releaseRuntime = {
  STRAYLIGHT_ENV: "production",
  STRAYLIGHT_BIND: "[::]:8080",
  STRAYLIGHT_LEGACY_API_ENABLED: "false",
  STRAYLIGHT_EVALUATION_API_ENABLED: "false",
  STRAYLIGHT_EMBEDDING_PROVIDER: "openai",
  STRAYLIGHT_ALLOW_DEGRADED_EMBEDDINGS: "false",
  STRAYLIGHT_EMBEDDING_MODEL: "text-embedding-3-small",
  STRAYLIGHT_EMBEDDING_DIMENSIONS: "1536",
  STRAYLIGHT_CAPTURE_MODEL: "gpt-5.6",
  STRAYLIGHT_CAPTURE_MAX_OUTPUT_TOKENS: "8192",
  STRAYLIGHT_ASSET_DESCRIPTION_MODEL: "gpt-5.6",
  STRAYLIGHT_ASSET_DESCRIPTION_MAX_OUTPUT_TOKENS: "4096",
  STRAYLIGHT_ASSET_DESCRIPTION_IMAGE_DETAIL: "high",
  STRAYLIGHT_DREAM_MODEL: "gpt-5.6",
  STRAYLIGHT_DREAM_SCHEDULER_ENABLED: "false",
  STRAYLIGHT_DREAM_SCHEDULER_POLL_SECONDS: "15",
  STRAYLIGHT_DREAM_INACTIVITY_SECONDS: "60",
  STRAYLIGHT_DREAM_COOLDOWN_SECONDS: "900",
  STRAYLIGHT_DREAM_DIRTY_THRESHOLD: "10",
  STRAYLIGHT_BACKGROUND_JOB_LEASE_SECONDS: "300",
  STRAYLIGHT_MATERIALIZE_TOKEN_BUDGET: "24000",
  STRAYLIGHT_REQUEST_TIMEOUT_SECONDS: "30",
  STRAYLIGHT_TRANSFER_TIMEOUT_SECONDS: "3600",
  STRAYLIGHT_MAX_CONCURRENT_TRANSFERS: "8",
  STRAYLIGHT_READINESS_TIMEOUT_SECONDS: "5",
  STRAYLIGHT_REQUESTS_PER_MINUTE: "600",
  STRAYLIGHT_INTENTION_LEDGER: preserve(),
  STRAYLIGHT_LEXICAL_SINGLE_SCAN: preserve(),
  STRAYLIGHT_RESUME_DELTAS: preserve(),
  STRAYLIGHT_SEARCH_CHAR_CAP: preserve(),
  STRAYLIGHT_SEARCH_FAIR_SHARE: preserve(),
  STRAYLIGHT_SEARCH_TOP1_HYDRATION: preserve(),
  STRAYLIGHT_SEMANTIC_LANE: preserve(),
  STRAYLIGHT_TODOIST_SYNC_ENABLED: preserve(),
  STRAYLIGHT_MESSAGING_ENABLED: preserve(),
  STRAYLIGHT_SEMANTIC_DEADLINE_MS: "2500",
  STRAYLIGHT_SEMANTIC_QUERY_PROVIDER_TIMEOUT_MS: "5000",
  STRAYLIGHT_SEMANTIC_QUERY_CONCURRENCY: "8",
  // Requalified 2026-08-03 from the post-GIN-fix gate battery. Semantic work
  // is now opportunistic for hybrid retrieval, so it cannot inflate these
  // foreground measurements; the thresholds continue to pause backfill on
  // real exact+lexical degradation.
  // Pre-fix values 120/107 predated semantic-on and paused continuously.
  STRAYLIGHT_EMBEDDING_BACKFILL_OPEN_P95_LIMIT_MS: "500",
  STRAYLIGHT_EMBEDDING_BACKFILL_SEARCH_P95_LIMIT_MS: "350",
  STRAYLIGHT_SUPERSESSION_DEMOTION: preserve(),
  STRAYLIGHT_VERBATIM_SPANS: preserve(),
  STRAYLIGHT_ACCOUNT_EXPORT_TTL_HOURS: "24",
  STRAYLIGHT_ACCOUNT_EXPORT_TEMP_DIR: "/tmp/straylight-exports",
  STRAYLIGHT_ACCOUNT_DELETION_BACKUP_RETENTION_DAYS: "30",
  STRAYLIGHT_DATABASE_MAX_CONNECTIONS: "8",
  STRAYLIGHT_ALLOWED_ORIGINS: "https://straylight.rourkem.com",
  STRAYLIGHT_PUBLIC_URL: "https://straylight.rourkem.com",
  STRAYLIGHT_APNS_APP_ID: "com.rourkem.straylight",
  // Owner-alpha push delivery is enabled for the signed-device sandbox
  // rollout. API and worker must retain the same value.
  STRAYLIGHT_APNS_DELIVERY_ENABLED: "true",
  AUTH_EMAIL_FROM: "Straylight <login@solark.io>",
  AUTH_EMAIL_REPLY_TO: "rourkem@rourkem.com",
  STRAYLIGHT_S3_ENDPOINT: "",
  STRAYLIGHT_S3_REGION: "us-west-2",
  STRAYLIGHT_S3_BUCKET: "rourkem-straylight-alpha-us-west-2",
  STRAYLIGHT_S3_FORCE_PATH_STYLE: "false",
  STRAYLIGHT_S3_CREATE_BUCKET: "false",
  STRAYLIGHT_METRICS_ENABLED: "true",
  STRAYLIGHT_DOGSTATSD_ADDR: "datadog-agent.railway.internal:8125",
  STRAYLIGHT_METRICS_FLUSH_SECONDS: "3",
  OPENAI_BASE_URL: "https://api.openai.com/v1",
  DD_ENV: "alpha",
  DD_SERVICE: "straylight",
  DD_VERSION: preserve(),
  STRAYLIGHT_BUILD_REVISION: preserve(),
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
    STRAYLIGHT_BUILD_REVISION: preserve(),
    POSTGRES_DB: "straylight",
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

const api = service("api", {
  build: {
    builder: "DOCKERFILE",
    dockerfilePath: "apps/api/Dockerfile",
    watchPatterns: ["apps/api/**"],
  },
  start: "/usr/local/bin/straylight serve",
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
    STRAYLIGHT_CONTINUATION_SECRET: preserve(),
    STRAYLIGHT_NOTIFICATION_TOKEN_ENCRYPTION_KEY: preserve(),
    STRAYLIGHT_SECRET_ENCRYPTION_KEY: preserve(),
    STRAYLIGHT_S3_ACCESS_KEY: preserve(),
    STRAYLIGHT_S3_SECRET_KEY: preserve(),
    RESEND_API_KEY: preserve(),
    STRAYLIGHT_SYSLOG_ADDR: "datadog-agent.railway.internal:514",
    OPENAI_API_KEY: preserve(),
  },
});

const worker = service("worker", {
  build: {
    builder: "DOCKERFILE",
    dockerfilePath: "apps/api/Dockerfile",
    watchPatterns: ["apps/api/**"],
  },
  start: "/usr/local/bin/straylight worker",
  preDeploy: "/usr/local/bin/straylight migrate",
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
    STRAYLIGHT_BUILD_REVISION: api.env.STRAYLIGHT_BUILD_REVISION,
    // The API owns activation. Reference that exact service variable so the
    // worker cannot drift to a separate preserved kill-switch value.
    STRAYLIGHT_TODOIST_SYNC_ENABLED:
      api.env.STRAYLIGHT_TODOIST_SYNC_ENABLED,
    STRAYLIGHT_MESSAGING_ENABLED:
      api.env.STRAYLIGHT_MESSAGING_ENABLED,
    DATABASE_URL_ADMIN: db.env.DATABASE_URL_ADMIN,
    DATABASE_URL_RW: db.env.DATABASE_URL_RW,
    DATABASE_URL_RO: db.env.DATABASE_URL_RO,
    STRAYLIGHT_CONTINUATION_SECRET: api.env.STRAYLIGHT_CONTINUATION_SECRET,
    STRAYLIGHT_NOTIFICATION_TOKEN_ENCRYPTION_KEY:
      api.env.STRAYLIGHT_NOTIFICATION_TOKEN_ENCRYPTION_KEY,
    STRAYLIGHT_SECRET_ENCRYPTION_KEY:
      api.env.STRAYLIGHT_SECRET_ENCRYPTION_KEY,
    STRAYLIGHT_APNS_TEAM_ID: preserve(),
    STRAYLIGHT_APNS_KEY_ID: preserve(),
    STRAYLIGHT_APNS_PRIVATE_KEY: preserve(),
    STRAYLIGHT_S3_ACCESS_KEY: api.env.STRAYLIGHT_S3_ACCESS_KEY,
    STRAYLIGHT_S3_SECRET_KEY: api.env.STRAYLIGHT_S3_SECRET_KEY,
    STRAYLIGHT_SYSLOG_ADDR: "datadog-agent.railway.internal:515",
    STRAYLIGHT_EMBEDDING_BACKFILL_FOREGROUND_STATUS_URL:
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
    STRAYLIGHT_API_URL: "http://api.railway.internal:8080",
    STRAYLIGHT_MCP_PUBLIC_URL: "https://straylight.rourkem.com",
    STRAYLIGHT_MESSAGING_ENABLED: api.env.STRAYLIGHT_MESSAGING_ENABLED,
    STRAYLIGHT_MCP_SEALING_KEY: preserve(),
    STRAYLIGHT_BUILD_REVISION: preserve(),
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
    STRAYLIGHT_API_HOST: api.env.RAILWAY_PRIVATE_DOMAIN,
    STRAYLIGHT_MCP_HOST: mcp.env.RAILWAY_PRIVATE_DOMAIN,
    STRAYLIGHT_DNS_RESOLVER: "[fd12::10]",
    STRAYLIGHT_BUILD_REVISION: preserve(),
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
    DD_HOSTNAME: "straylight-railway-alpha",
    DD_ENV: "alpha",
    DD_VERSION: preserve(),
    DD_TAGS: "project:straylight platform:railway",
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
  const application = group("Application", [web, api, worker, mcp]);
  const storage = group("Storage", [db, postgresData]);
  const operations = group("Operations", [datadog]);

  return project("Straylight", {
    resources: [application, storage, operations],
  });
});
