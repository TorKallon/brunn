from __future__ import annotations

import json
import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RAILWAY = (ROOT / ".railway/railway.ts").read_text()
WEB_PROXY = (ROOT / "apps/web/nginx.railway.conf.template").read_text()
WEB_DOCKERFILE = (ROOT / "apps/web/Dockerfile.railway").read_text()
API_DOCKERFILE = (ROOT / "apps/api/Dockerfile").read_text()
MCP_DOCKERFILE = (ROOT / "apps/mcp/Dockerfile.remote").read_text()
DATABASE_DOCKERFILE = (ROOT / "infra/postgres/Dockerfile").read_text()


class RailwayContractTests(unittest.TestCase):
    def test_iac_sdk_is_exactly_pinned(self):
        package = json.loads((ROOT / ".railway/package.json").read_text())
        lock = json.loads((ROOT / ".railway/package-lock.json").read_text())
        self.assertEqual("3.6.0", package["dependencies"]["railway"])
        self.assertEqual(
            "3.6.0",
            lock["packages"]["node_modules/railway"]["version"],
        )

    def test_topology_has_only_the_expected_services_and_one_database_volume(self):
        services = re.findall(r'const ([a-z]+) = service\("([^"]+)"', RAILWAY)
        self.assertEqual(
            {
                ("db", "db"),
                ("api", "api"),
                ("worker", "worker"),
                ("mcp", "mcp"),
                ("web", "web"),
                ("datadog", "datadog-agent"),
            },
            set(services),
        )
        self.assertEqual(
            1,
            len(re.findall(r'volume\("postgres-data"', RAILWAY)),
        )
        self.assertIn('"/var/lib/postgresql/data": postgresData', RAILWAY)
        self.assertIn('PGDATA: "/var/lib/postgresql/data/pgdata"', RAILWAY)
        self.assertIn("sizeMB: 20000", RAILWAY)
        self.assertNotIn("minio", RAILWAY.lower())
        self.assertIn(
            "COPY infra/postgres/init/ /docker-entrypoint-initdb.d/",
            DATABASE_DOCKERFILE,
        )
        self.assertIn(
            "COPY infra/postgres/healthcheck.sh "
            "/usr/local/bin/straylight-postgres-healthcheck",
            DATABASE_DOCKERFILE,
        )

    def test_public_web_is_the_only_domain_boundary(self):
        self.assertNotIn("domains:", RAILWAY)
        self.assertIn(
            'STRAYLIGHT_API_HOST: api.env.RAILWAY_PRIVATE_DOMAIN',
            RAILWAY,
        )
        self.assertIn(
            'STRAYLIGHT_MCP_HOST: mcp.env.RAILWAY_PRIVATE_DOMAIN',
            RAILWAY,
        )
        self.assertIn('STRAYLIGHT_DNS_RESOLVER: "[fd12::10]"', RAILWAY)
        self.assertIn(
            "resolver ${STRAYLIGHT_DNS_RESOLVER} valid=10s ipv6=on;",
            WEB_PROXY,
        )
        self.assertIn("server ${STRAYLIGHT_API_HOST}:8080 resolve;", WEB_PROXY)
        self.assertIn("server ${STRAYLIGHT_MCP_HOST}:8080 resolve;", WEB_PROXY)
        self.assertRegex(
            WEB_PROXY,
            r"location \^~ /api/v1/admin/ \{\s*return 404;",
        )
        self.assertIn("Strict-Transport-Security", WEB_PROXY)
        self.assertIn(
            "install -d -o 101 -g 101 -m 0755 /etc/nginx/templates",
            WEB_DOCKERFILE,
        )
        self.assertRegex(
            WEB_PROXY,
            r"location ~ \^/\(\?:mcp\|authorize\|token\|register\|oauth/consent",
        )
        mcp_proxy = WEB_PROXY.split(
            "location ~ ^/(?:mcp|authorize|token|register|oauth/consent", 1
        )[1]
        self.assertIn("client_max_body_size 5m;", mcp_proxy)

    def test_public_web_has_redundant_ingress_and_zero_downtime_rollouts(self):
        web_block = RAILWAY.split('const web = service("web"', 1)[1].split(
            'const datadog = service("datadog-agent"', 1
        )[0]
        self.assertIn('replicas: { "us-west2": 2 }', web_block)
        self.assertIn('restartPolicyType: "ALWAYS"', web_block)
        self.assertIn("restartPolicyMaxRetries: null", web_block)
        self.assertIn("overlapSeconds: 30", web_block)
        self.assertIn("drainingSeconds: 30", web_block)

    def test_foreground_api_has_redundant_replicas(self):
        api_block = RAILWAY.split('const api = service("api"', 1)[1].split(
            'const worker = service("worker"', 1
        )[0]
        self.assertIn('replicas: { "us-west2": 2 }', api_block)
        self.assertIn('restartPolicyType: "ALWAYS"', api_block)
        self.assertIn("restartPolicyMaxRetries: null", api_block)
        self.assertIn("overlapSeconds: 30", api_block)

    def test_single_replica_runtime_services_restart_without_exhaustion(self):
        worker_block = RAILWAY.split('const worker = service("worker"', 1)[1].split(
            'const mcp = service("mcp"', 1
        )[0]
        mcp_block = RAILWAY.split('const mcp = service("mcp"', 1)[1].split(
            'const web = service("web"', 1
        )[0]
        datadog_block = RAILWAY.split(
            'const datadog = service("datadog-agent"', 1
        )[1].split("export default", 1)[0]
        for block in (worker_block, mcp_block, datadog_block):
            self.assertIn('replicas: { "us-west2": 1 }', block)
            self.assertIn('restartPolicyType: "ALWAYS"', block)
            self.assertIn("restartPolicyMaxRetries: null", block)

    def test_semantic_only_has_a_realistic_bound_but_hybrid_stays_optional(self):
        self.assertIn('STRAYLIGHT_SEMANTIC_DEADLINE_MS: "2500"', RAILWAY)
        self.assertIn(
            "hybrid requests take semantic evidence only when it is ready",
            (ROOT / "docs" / "Architecture.md").read_text(),
        )

    def test_large_workspace_binary_uploads_stream_without_proxy_buffering(self):
        self.assertRegex(
            WEB_PROXY,
            r"location = /api/v1/workspace/binaries/content \{[\s\S]*?"
            r"client_max_body_size 4g;[\s\S]*?"
            r"proxy_pass http://straylight_api/v1/workspace/binaries/content;[\s\S]*?"
            r"proxy_request_buffering off;",
        )

    def test_api_has_no_database_administrator_credential(self):
        api_block = RAILWAY.split('const api = service("api"', 1)[1].split(
            'const worker = service("worker"', 1
        )[0]
        worker_block = RAILWAY.split('const worker = service("worker"', 1)[1].split(
            'const mcp = service("mcp"', 1
        )[0]
        self.assertNotIn("DATABASE_URL_ADMIN", api_block)
        self.assertIn("DATABASE_URL_ADMIN: db.env.DATABASE_URL_ADMIN", worker_block)
        self.assertIn(
            'preDeploy: "/usr/local/bin/straylight migrate"',
            worker_block,
        )
        self.assertIn(
            'STRAYLIGHT_EMBEDDING_BACKFILL_FOREGROUND_STATUS_URL:\n'
            '      "http://api.railway.internal:8080/health/foreground-latency"',
            worker_block,
        )

    def test_password_recovery_mail_secret_is_api_scoped(self):
        api_block = RAILWAY.split('const api = service("api"', 1)[1].split(
            'const worker = service("worker"', 1
        )[0]
        worker_block = RAILWAY.split('const worker = service("worker"', 1)[1].split(
            'const mcp = service("mcp"', 1
        )[0]
        web_block = RAILWAY.split('const web = service("web"', 1)[1].split(
            'const datadog = service("datadog-agent"', 1
        )[0]
        self.assertIn("RESEND_API_KEY: preserve()", api_block)
        self.assertNotIn("RESEND_API_KEY", worker_block)
        self.assertNotIn("RESEND_API_KEY", web_block)
        self.assertIn(
            'STRAYLIGHT_PUBLIC_URL: "https://straylight.rourkem.com"',
            RAILWAY,
        )
        self.assertIn('AUTH_EMAIL_FROM: "Straylight <login@solark.io>"', RAILWAY)

    def test_apns_delivery_is_enabled_with_provider_secrets_worker_only(self):
        api_block = RAILWAY.split('const api = service("api"', 1)[1].split(
            'const worker = service("worker"', 1
        )[0]
        worker_block = RAILWAY.split('const worker = service("worker"', 1)[1].split(
            'const mcp = service("mcp"', 1
        )[0]
        self.assertIn(
            'STRAYLIGHT_APNS_APP_ID: "com.rourkem.straylight"',
            RAILWAY,
        )
        self.assertIn('STRAYLIGHT_APNS_DELIVERY_ENABLED: "true"', RAILWAY)
        self.assertIn(
            "STRAYLIGHT_NOTIFICATION_TOKEN_ENCRYPTION_KEY: preserve()",
            api_block,
        )
        self.assertNotIn("STRAYLIGHT_APNS_TEAM_ID", api_block)
        self.assertNotIn("STRAYLIGHT_APNS_KEY_ID", api_block)
        self.assertNotIn("STRAYLIGHT_APNS_PRIVATE_KEY", api_block)
        self.assertIn("STRAYLIGHT_APNS_TEAM_ID: preserve()", worker_block)
        self.assertIn("STRAYLIGHT_APNS_KEY_ID: preserve()", worker_block)
        self.assertIn("STRAYLIGHT_APNS_PRIVATE_KEY: preserve()", worker_block)

    def test_browser_auth_has_a_small_rate_limited_proxy_boundary(self):
        self.assertIn(
            "zone=straylight_auth_limit:1m rate=2r/s",
            WEB_PROXY,
        )
        auth_proxy = WEB_PROXY.split("location ^~ /api/v1/auth/ {", 1)[1].split(
            "location /api/ {", 1
        )[0]
        self.assertIn("client_max_body_size 16k;", auth_proxy)
        self.assertIn("limit_req zone=straylight_auth_limit burst=10 nodelay;", auth_proxy)
        self.assertIn("proxy_pass http://straylight_api/v1/auth/;", auth_proxy)

    def test_reasoning_and_token_contract_is_frozen(self):
        expected = {
            "STRAYLIGHT_LEGACY_API_ENABLED": "false",
            "STRAYLIGHT_EVALUATION_API_ENABLED": "false",
            "STRAYLIGHT_EMBEDDING_PROVIDER": "openai",
            "STRAYLIGHT_ALLOW_DEGRADED_EMBEDDINGS": "false",
            "STRAYLIGHT_EMBEDDING_MODEL": "text-embedding-3-small",
            "STRAYLIGHT_EMBEDDING_DIMENSIONS": "1536",
            "STRAYLIGHT_CAPTURE_MODEL": "gpt-5.6",
            "STRAYLIGHT_CAPTURE_MAX_OUTPUT_TOKENS": "8192",
            "STRAYLIGHT_DREAM_MODEL": "gpt-5.6",
            "STRAYLIGHT_DREAM_SCHEDULER_ENABLED": "false",
            "STRAYLIGHT_MATERIALIZE_TOKEN_BUDGET": "24000",
        }
        for name, value in expected.items():
            self.assertIn(f'{name}: "{value}"', RAILWAY)

    def test_release_revision_is_available_to_every_runtime(self):
        self.assertGreaterEqual(
            RAILWAY.count("STRAYLIGHT_BUILD_REVISION: preserve()"),
            3,
        )
        self.assertIn("STRAYLIGHT_BUILD_REVISION: api.env.STRAYLIGHT_BUILD_REVISION", RAILWAY)
        self.assertGreaterEqual(RAILWAY.count("DD_VERSION: preserve()"), 2)

    def test_remote_mcp_is_private_oauth_gateway(self):
        mcp_block = RAILWAY.split('const mcp = service("mcp"', 1)[1].split(
            'const web = service("web"', 1
        )[0]
        self.assertIn('dockerfilePath: "apps/mcp/Dockerfile.remote"', mcp_block)
        self.assertIn('healthcheck: "/healthz"', mcp_block)
        self.assertIn(
            'STRAYLIGHT_API_URL: "http://api.railway.internal:8080"',
            mcp_block,
        )
        self.assertIn(
            'STRAYLIGHT_MCP_PUBLIC_URL: "https://straylight.rourkem.com"',
            mcp_block,
        )
        self.assertIn("STRAYLIGHT_MCP_SEALING_KEY: preserve()", mcp_block)
        self.assertNotIn("STRAYLIGHT_API_TOKEN", mcp_block)

        oauth_proxy = WEB_PROXY.split(
            "# The public web service is the only edge.", 1
        )[1].split("location / {", 1)[0]
        self.assertIn('proxy_set_header Cookie "";', oauth_proxy)
        self.assertIn("proxy_hide_header Set-Cookie;", oauth_proxy)
        self.assertIn('ENTRYPOINT ["/nodejs/bin/node", "dist/remote.js"]', MCP_DOCKERFILE)

    def test_hosted_object_store_is_external_versioned_s3(self):
        self.assertIn('STRAYLIGHT_S3_ENDPOINT: ""', RAILWAY)
        self.assertIn('STRAYLIGHT_S3_REGION: "us-west-2"', RAILWAY)
        self.assertIn('STRAYLIGHT_S3_FORCE_PATH_STYLE: "false"', RAILWAY)
        self.assertIn('STRAYLIGHT_S3_CREATE_BUCKET: "false"', RAILWAY)
        self.assertIn("STRAYLIGHT_S3_ACCESS_KEY: preserve()", RAILWAY)
        self.assertIn("STRAYLIGHT_S3_SECRET_KEY: preserve()", RAILWAY)

    def test_datadog_metrics_and_logs_stay_on_private_networking(self):
        self.assertIn(
            'STRAYLIGHT_DOGSTATSD_ADDR: "datadog-agent.railway.internal:8125"',
            RAILWAY,
        )
        self.assertIn(
            'STRAYLIGHT_SYSLOG_ADDR: "datadog-agent.railway.internal:514"',
            RAILWAY,
        )
        self.assertIn(
            'STRAYLIGHT_SYSLOG_ADDR: "datadog-agent.railway.internal:515"',
            RAILWAY,
        )
        self.assertIn("DD_API_KEY: preserve()", RAILWAY)
        syslog = (ROOT / "deploy/railway/datadog-agent/syslog.yaml").read_text()
        self.assertIn("port: 514", syslog)
        self.assertIn("component:api", syslog)
        self.assertIn("port: 515", syslog)
        self.assertIn("component:worker", syslog)

    def test_railway_dockerfiles_pin_every_external_base(self):
        for path in [
            ROOT / "apps/api/Dockerfile",
            ROOT / "apps/web/Dockerfile.railway",
            ROOT / "apps/mcp/Dockerfile.remote",
            ROOT / "deploy/railway/datadog-agent/Dockerfile",
        ]:
            local_stages: set[str] = set()
            for line in path.read_text().splitlines():
                if not line.startswith("FROM "):
                    continue
                parts = line.split()
                image = parts[1]
                if image not in local_stages:
                    self.assertRegex(image, r"@sha256:[0-9a-f]{64}$", path)
                if len(parts) >= 4 and parts[2].upper() == "AS":
                    local_stages.add(parts[3])

    def test_shared_api_dockerfile_is_portable_across_railway_services(self):
        self.assertNotIn("--mount=type=cache", API_DOCKERFILE)
        self.assertIn("RUN cargo build --release", API_DOCKERFILE)
        self.assertIn("RUN cargo test --lib", API_DOCKERFILE)


if __name__ == "__main__":
    unittest.main()
