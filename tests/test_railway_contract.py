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
        self.assertIn("sizeMB: 5000", RAILWAY)
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
        self.assertIn('STRAYLIGHT_DNS_RESOLVER: "[fd12::10]"', RAILWAY)
        self.assertIn(
            "resolver ${STRAYLIGHT_DNS_RESOLVER} valid=10s ipv6=on;",
            WEB_PROXY,
        )
        self.assertIn("server ${STRAYLIGHT_API_HOST}:8080 resolve;", WEB_PROXY)
        self.assertRegex(
            WEB_PROXY,
            r"location \^~ /api/v1/admin/ \{\s*return 404;",
        )
        self.assertIn("Strict-Transport-Security", WEB_PROXY)
        self.assertIn(
            "install -d -o 101 -g 101 -m 0755 /etc/nginx/templates",
            WEB_DOCKERFILE,
        )

    def test_api_has_no_database_administrator_credential(self):
        api_block = RAILWAY.split('const api = service("api"', 1)[1].split(
            'const worker = service("worker"', 1
        )[0]
        worker_block = RAILWAY.split('const worker = service("worker"', 1)[1].split(
            'const web = service("web"', 1
        )[0]
        self.assertNotIn("DATABASE_URL_ADMIN", api_block)
        self.assertIn("DATABASE_URL_ADMIN: db.env.DATABASE_URL_ADMIN", worker_block)
        self.assertIn(
            'preDeploy: "/usr/local/bin/straylight migrate"',
            worker_block,
        )

    def test_reasoning_and_token_contract_is_frozen(self):
        expected = {
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

    def test_shared_api_dockerfile_uses_railway_scoped_locked_cache_mounts(self):
        cache_mounts = re.findall(
            r"--mount=type=cache,([^\\\s]+)",
            API_DOCKERFILE,
        )
        self.assertGreaterEqual(len(cache_mounts), 3)
        for cache_mount in cache_mounts:
            self.assertIn("target=", cache_mount)
            self.assertIn("sharing=locked", cache_mount)
            self.assertRegex(
                cache_mount,
                r"id=s/[0-9a-f-]{36}-/",
            )


if __name__ == "__main__":
    unittest.main()
