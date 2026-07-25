from __future__ import annotations

import json
import os
import re
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


class ProductionContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        compose_environment = {
            **os.environ,
            "OPENAI_API_KEY": "",
            "STRAYLIGHT_S3_ACCESS_KEY": "",
            "STRAYLIGHT_S3_SECRET_KEY": "",
            "STRAYLIGHT_MINIO_ACCESS_KEY": "",
            "STRAYLIGHT_MINIO_SECRET_KEY": "",
        }
        result = subprocess.run(
            [
                "docker",
                "compose",
                "--env-file",
                str(ROOT / ".env.example"),
                "--file",
                str(ROOT / "compose.yaml"),
                "--file",
                str(ROOT / "compose.production.yaml"),
                "--profile",
                "*",
                "config",
                "--format",
                "json",
            ],
            cwd=ROOT,
            env=compose_environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if result.returncode != 0:
            raise AssertionError(f"production Compose contract failed: {result.stderr}")
        cls.compose = json.loads(result.stdout)
        managed = subprocess.run(
            [
                "docker",
                "compose",
                "--env-file",
                str(ROOT / "production.managed-s3.env.example"),
                "--file",
                str(ROOT / "compose.yaml"),
                "--file",
                str(ROOT / "compose.production.yaml"),
                "--file",
                str(ROOT / "compose.managed-s3.yaml"),
                "--profile",
                "observability",
                "config",
                "--format",
                "json",
            ],
            cwd=ROOT,
            env=compose_environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if managed.returncode != 0:
            raise AssertionError(
                f"managed S3 Compose contract failed: {managed.stderr}"
            )
        cls.managed_compose = json.loads(managed.stdout)

    def test_internal_services_publish_no_host_ports(self):
        for service in ["db", "minio", "api", "web"]:
            self.assertNotIn("ports", self.compose["services"][service], service)
        edge_ports = {
            (item["target"], item["published"], item["protocol"])
            for item in self.compose["services"]["edge"]["ports"]
        }
        self.assertEqual(
            {(80, "80", "tcp"), (443, "443", "tcp"), (443, "443", "udp")},
            edge_ports,
        )

    def test_application_secrets_are_file_backed_and_direct_values_are_empty(self):
        expected_files = {
            "DATABASE_URL_RW_FILE": "/run/secrets/database_url_rw",
            "DATABASE_URL_RO_FILE": "/run/secrets/database_url_ro",
            "STRAYLIGHT_S3_ACCESS_KEY_FILE": "/run/secrets/minio_app_access_key",
            "STRAYLIGHT_S3_SECRET_KEY_FILE": "/run/secrets/minio_app_secret_key",
            "STRAYLIGHT_CONTINUATION_SECRET_FILE": "/run/secrets/continuation_signing_key",
            "OPENAI_API_KEY_FILE": "/run/secrets/openai_api_key",
        }
        direct_secrets = {
            "DATABASE_URL_RW",
            "DATABASE_URL_RO",
            "STRAYLIGHT_DATABASE_URL",
            "STRAYLIGHT_READ_ONLY_DATABASE_URL",
            "STRAYLIGHT_S3_ACCESS_KEY",
            "STRAYLIGHT_S3_SECRET_KEY",
            "STRAYLIGHT_MINIO_ACCESS_KEY",
            "STRAYLIGHT_MINIO_SECRET_KEY",
            "STRAYLIGHT_CONTINUATION_SECRET",
            "STRAYLIGHT_CONTINUATION_SIGNING_KEY",
            "OPENAI_API_KEY",
            "STRAYLIGHT_DEV_READ_WRITE_TOKEN",
            "STRAYLIGHT_DEV_READ_ONLY_TOKEN",
        }
        for service in ["migrate", "api", "worker"]:
            environment = self.compose["services"][service]["environment"]
            self.assertEqual("production", environment["STRAYLIGHT_ENV"])
            self.assertEqual("production", environment["DD_ENV"])
            self.assertEqual("unreleased", environment["DD_VERSION"])
            for name, path in expected_files.items():
                self.assertEqual(path, environment[name], f"{service}:{name}")
            for name in direct_secrets:
                self.assertEqual("", environment.get(name, ""), f"{service}:{name}")
        for service in ["migrate", "worker"]:
            self.assertEqual(
                "/run/secrets/database_url_admin",
                self.compose["services"][service]["environment"]["DATABASE_URL_ADMIN_FILE"],
            )
        self.assertEqual(
            "",
            self.compose["services"]["api"]["environment"]["DATABASE_URL_ADMIN_FILE"],
        )

    def test_runtime_services_have_explicit_limits(self):
        for service in [
            "db",
            "minio-permissions",
            "minio",
            "api",
            "worker",
            "web",
            "edge-permissions",
            "edge",
        ]:
            detail = self.compose["services"][service]
            self.assertNotIn(detail["mem_limit"], {"", "0", 0}, service)
            self.assertGreater(float(detail["cpus"]), 0, service)
            self.assertGreater(detail["pids_limit"], 0, service)

    def test_production_uses_prebuilt_release_images(self):
        expected = {
            "db": "straylight-postgres:release-blocked",
            "migrate": "straylight-api:release-blocked",
            "api": "straylight-api:release-blocked",
            "worker": "straylight-api:release-blocked",
            "web": "straylight-web:release-blocked",
            "mcp": "straylight-mcp:release-blocked",
            "edge": "straylight-caddy:release-blocked",
            "minio": "straylight-object-store:release-blocked",
            "minio-init": "straylight-object-store-client:release-blocked",
        }
        for service, image in expected.items():
            self.assertEqual(image, self.compose["services"][service]["image"])
            self.assertNotIn("build", self.compose["services"][service], service)
        edge_mounts = self.compose["services"]["edge"].get("volumes", [])
        self.assertFalse(
            any(item.get("target") == "/etc/caddy/Caddyfile" for item in edge_mounts)
        )

    def test_database_has_stable_collation_and_page_checksums(self):
        database = self.compose["services"]["db"]
        initdb_args = database["environment"]["POSTGRES_INITDB_ARGS"]
        self.assertIn("--locale-provider=builtin", initdb_args)
        self.assertIn("--builtin-locale=C.UTF-8", initdb_args)
        self.assertIn("--encoding=UTF8", initdb_args)
        self.assertIn("--data-checksums", initdb_args)
        self.assertEqual(
            ["CMD", "/usr/local/bin/straylight-postgres-healthcheck"],
            database["healthcheck"]["test"],
        )
        healthcheck = (ROOT / "infra/postgres/healthcheck.sh").read_text()
        self.assertIn("datlocprovider = 'b'", healthcheck)
        self.assertIn("datlocale = 'C.UTF-8'", healthcheck)
        self.assertIn("current_setting('data_checksums') = 'on'", healthcheck)

    def test_database_image_declares_the_postgres_runtime_user(self):
        dockerfile = (ROOT / "infra/postgres/Dockerfile").read_text()
        final_stage = dockerfile.rsplit("\nFROM ", maxsplit=1)[-1]
        self.assertRegex(final_stage, r"(?m)^USER postgres:postgres$")

    def test_object_store_policy_supports_the_qualified_versioning_contract(self):
        policy = json.loads((ROOT / "infra/minio/app-policy.json").read_text())
        actions = {
            action
            for statement in policy["Statement"]
            for action in statement["Action"]
        }
        self.assertTrue(
            {
                "s3:GetBucketVersioning",
                "s3:ListBucketVersions",
                "s3:GetObject",
                "s3:PutObject",
                "s3:DeleteObject",
                "s3:DeleteObjectVersion",
            }.issubset(actions)
        )

    def test_application_object_store_configuration_is_provider_neutral(self):
        config = (ROOT / "apps/api/src/config.rs").read_text()
        object_store = (ROOT / "apps/api/src/object_store.rs").read_text()
        example = (ROOT / ".env.example").read_text()
        evaluation = (ROOT / "docs/Object Store Evaluation.md").read_text()

        for declaration in [
            "pub s3_endpoint: Option<String>",
            "pub s3_access_key: Option<String>",
            "pub s3_secret_key: Option<String>",
            "pub s3_force_path_style: bool",
            "pub s3_create_bucket: bool",
        ]:
            self.assertIn(declaration, config)
        for alias in [
            "STRAYLIGHT_MINIO_ENDPOINT",
            "STRAYLIGHT_MINIO_REGION",
            "STRAYLIGHT_MINIO_BUCKET",
            "STRAYLIGHT_MINIO_ACCESS_KEY",
            "STRAYLIGHT_MINIO_SECRET_KEY",
            "STRAYLIGHT_MINIO_FORCE_PATH_STYLE",
            "STRAYLIGHT_MINIO_CREATE_BUCKET",
        ]:
            self.assertIn(alias, config)

        self.assertIn("validate_explicit_s3_credentials", config)
        self.assertIn('deployment_environment != "production"', config)
        self.assertRegex(
            object_store,
            r"if let \(Some\(access_key\), Some\(secret_key\)\)[\s\S]*?"
            r"credentials_provider",
        )
        self.assertRegex(
            object_store,
            r"if let Some\(endpoint\) = &config\.s3_endpoint[\s\S]*?"
            r"endpoint_url",
        )
        self.assertIn(
            ".force_path_style(config.s3_force_path_style)",
            object_store,
        )
        self.assertIn("if !self.create_bucket", object_store)

        for setting in [
            "STRAYLIGHT_S3_FORCE_PATH_STYLE",
            "STRAYLIGHT_S3_CREATE_BUCKET",
        ]:
            self.assertIn(setting, example)
            self.assertIn(setting, evaluation)
        self.assertIn("default credential chain", example)
        self.assertIn("default chain", evaluation)
        self.assertIn("workload identity", evaluation)

    def test_managed_s3_overlay_removes_local_object_store_runtime(self):
        services = self.managed_compose["services"]
        self.assertNotIn("minio-permissions", services)
        self.assertNotIn("minio", services)
        self.assertNotIn("minio-init", services)
        for service in ["migrate", "api", "worker"]:
            environment = services[service]["environment"]
            self.assertEqual("", environment["STRAYLIGHT_S3_ENDPOINT"])
            self.assertEqual("us-west-2", environment["STRAYLIGHT_S3_REGION"])
            self.assertEqual(
                "replace-carrystate-production",
                environment["STRAYLIGHT_S3_BUCKET"],
            )
            self.assertEqual("false", environment["STRAYLIGHT_S3_CREATE_BUCKET"])
            self.assertEqual("", environment["STRAYLIGHT_MINIO_ENDPOINT"])
            self.assertEqual("", environment["STRAYLIGHT_MINIO_BUCKET"])
            self.assertEqual("", environment["STRAYLIGHT_MINIO_ACCESS_KEY"])
            self.assertEqual("", environment["STRAYLIGHT_MINIO_SECRET_KEY"])
            self.assertEqual("", environment["STRAYLIGHT_S3_ACCESS_KEY"])
            self.assertEqual("", environment["STRAYLIGHT_S3_SECRET_KEY"])
            self.assertNotIn("AWS_ACCESS_KEY_ID", environment)
            self.assertNotIn("AWS_SECRET_ACCESS_KEY", environment)
            self.assertNotIn("AWS_SESSION_TOKEN", environment)
            secret_targets = {item["target"] for item in services[service]["secrets"]}
            self.assertFalse(
                {
                    "/run/secrets/minio_app_access_key",
                    "/run/secrets/minio_app_secret_key",
                }
                & secret_targets
            )

    def test_managed_s3_backup_restore_and_deploy_are_first_class(self):
        paths = [
            ROOT / "scripts/managed-s3-backup.sh",
            ROOT / "scripts/managed-s3-restore-drill.sh",
            ROOT / "scripts/verify-managed-backup.sh",
        ]
        for path in paths:
            self.assertTrue(path.stat().st_mode & 0o111, path.name)

        deploy = (ROOT / "scripts/deploy-production.sh").read_text()
        rollback = (ROOT / "scripts/rollback-production.sh").read_text()
        compatibility = (
            ROOT / "scripts/verify-rollback-compatibility.sh"
        ).read_text()
        managed_backup = paths[0].read_text()
        managed_restore = paths[1].read_text()
        verifier = paths[2].read_text()
        makefile = (ROOT / "Makefile").read_text()

        self.assertIn("STRAYLIGHT_OBJECT_STORE_MODE", deploy)
        self.assertIn("compose.managed-s3.yaml", deploy)
        self.assertIn("scripts/managed-s3-backup.sh", deploy)
        self.assertIn("object_store_mode", deploy)
        self.assertIn("COMPOSE_MANAGED_S3_FILE", rollback)
        self.assertIn("COMPOSE_MANAGED_S3_FILE", compatibility)
        self.assertIn("portable-all-versions", managed_backup)
        self.assertIn("STRAYLIGHT_MANAGED_BACKUP_ROOT", managed_backup)
        self.assertIn("STRAYLIGHT_RESTORE_DRILL", managed_restore)
        self.assertIn("cleanup-restore", managed_restore)
        self.assertIn("verify_host_stability", managed_restore)
        self.assertIn("straylight-managed-s3-coordinated-backup@v1", verifier)
        self.assertIn("managed-production-backup:", makefile)
        self.assertIn("managed-production-restore-drill:", makefile)

    def test_shipped_base_images_are_digest_pinned(self):
        dockerfiles = [
            ROOT / "apps/api/Dockerfile",
            ROOT / "apps/web/Dockerfile",
            ROOT / "apps/mcp/Dockerfile",
            ROOT / "infra/postgres/Dockerfile",
            ROOT / "infra/minio/Dockerfile",
            ROOT / "infra/minio-client/Dockerfile",
            ROOT / "infra/caddy/Dockerfile",
        ]
        for dockerfile in dockerfiles:
            local_stages: set[str] = set()
            for line in dockerfile.read_text().splitlines():
                if not line.startswith("FROM "):
                    continue
                parts = line.split()
                image = parts[1]
                if image not in local_stages:
                    self.assertRegex(
                        image,
                        r"@sha256:[0-9a-f]{64}$",
                        f"unpinned base image in {dockerfile}: {image}",
                    )
                if len(parts) >= 4 and parts[2].upper() == "AS":
                    local_stages.add(parts[3])
        for service in [
            "minio-permissions",
            "api-tools-port",
            "datadog-agent",
            "edge-permissions",
        ]:
            image = self.compose["services"][service]["image"]
            self.assertRegex(
                image,
                r"@sha256:[0-9a-f]{64}$",
                f"unpinned production image: {service}={image}",
            )

    def test_backup_retention_does_not_exceed_account_deletion_retention(self):
        environment = self.compose["services"]["worker"]["environment"]
        backup_days = int(
            re.search(
                r"STRAYLIGHT_BACKUP_RETENTION_DAYS=(\d+)",
                (ROOT / ".env.example").read_text(),
            ).group(1)
        )
        deletion_days = int(
            environment["STRAYLIGHT_ACCOUNT_DELETION_BACKUP_RETENTION_DAYS"]
        )
        self.assertLessEqual(backup_days, deletion_days)

    def test_public_proxy_does_not_expose_administrative_api(self):
        nginx = (ROOT / "apps/web/nginx.conf").read_text()
        self.assertIn("location ^~ /api/v1/admin/", nginx)
        self.assertRegex(
            nginx,
            r"location \^~ /api/v1/admin/ \{\s*return 404;",
        )
        self.assertRegex(
            nginx,
            r"location /api/ \{[\s\S]*?add_header Cache-Control \"no-store\" always;",
        )

    def test_public_proxy_has_independent_bounded_request_limits(self):
        nginx = (ROOT / "apps/web/nginx.conf").read_text()
        self.assertIn(
            "limit_req_zone $binary_remote_addr zone=straylight_ready:1m rate=1r/s;",
            nginx,
        )
        self.assertIn(
            "limit_req_zone $binary_remote_addr "
            "zone=straylight_api_limit:10m rate=20r/s;",
            nginx,
        )
        self.assertRegex(
            nginx,
            r"location = /api/ready \{[\s\S]*?"
            r"limit_req zone=straylight_ready burst=5 nodelay;[\s\S]*?"
            r'add_header Cache-Control "no-store" always;',
        )
        self.assertRegex(
            nginx,
            r"location /api/ \{[\s\S]*?"
            r"limit_req zone=straylight_api_limit burst=40 nodelay;",
        )
        self.assertRegex(
            nginx,
            r"location /api/ \{[\s\S]*?client_max_body_size 73m;",
        )
        self.assertRegex(
            nginx,
            r"location /api/ \{[\s\S]*?proxy_read_timeout 3600s;",
        )
        self.assertRegex(
            nginx,
            r"location /api/ \{[\s\S]*?proxy_send_timeout 3600s;",
        )

    def test_binary_transfers_have_separate_timeout_and_concurrency_bounds(self):
        config = (ROOT / "apps/api/src/config.rs").read_text()
        api = (ROOT / "apps/api/src/api.rs").read_text()
        compose = (ROOT / "compose.yaml").read_text()
        validator = (ROOT / "scripts/validate-production-config.sh").read_text()
        for setting in [
            "STRAYLIGHT_TRANSFER_TIMEOUT_SECONDS",
            "STRAYLIGHT_MAX_CONCURRENT_TRANSFERS",
        ]:
            self.assertIn(setting, config)
            self.assertIn(setting, compose)
            self.assertIn(setting, validator)
        self.assertIn("ConcurrencyLimitLayer", api)
        self.assertIn("state.config.transfer_timeout", api)
        self.assertIn("state.config.request_timeout", api)

    def test_spa_deep_links_do_not_redirect_to_the_internal_container_port(self):
        nginx = (ROOT / "apps/web/nginx.conf").read_text()
        self.assertRegex(
            nginx,
            r"location / \{\s*try_files \$uri /index\.html;",
        )
        self.assertNotIn("try_files $uri $uri/", nginx)

    def test_build_context_excludes_secrets_and_operational_data(self):
        patterns = set((ROOT / ".dockerignore").read_text().splitlines())
        self.assertTrue(
            {
                ".env*",
                "*.env",
                "production.env*",
                "secrets",
                "**/secrets",
                "backups",
                "**/backups",
                "release-artifacts",
                "operator-output",
                "deployment-records",
            }.issubset(patterns)
        )

    def test_production_validator_accepts_only_immutable_non_placeholder_config(self):
        revision = "a" * 40
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            secrets = directory / "secrets"
            secrets.mkdir(mode=0o700)
            secrets.chmod(0o700)
            admin_password = "a" * 24
            rw_password = "r" * 24
            ro_password = "o" * 24
            values = {
                "postgres_admin_password": admin_password,
                "postgres_app_rw_password": rw_password,
                "postgres_app_ro_password": ro_password,
                "database_url_rw": f"postgres://app_rw:{rw_password}@db:5432/straylight",
                "database_url_ro": f"postgres://app_ro:{ro_password}@db:5432/straylight",
                "database_url_admin": f"postgres://admin:{admin_password}@db:5432/straylight",
                "minio_root_user": "root-user",
                "minio_root_password": "m" * 24,
                "minio_app_access_key": "app-access",
                "minio_app_secret_key": "s" * 24,
                "continuation_signing_key": "c" * 32,
                "openai_api_key": "sk-unit-" + ("o" * 32),
                "dd_api_key": "d" * 32,
            }
            for name, value in values.items():
                path = secrets / name
                path.write_text(value)
                path.chmod(0o600)
            env_file = directory / "production.env"
            env_file.write_text(
                "\n".join(
                    [
                        "STRAYLIGHT_ENV=production",
                        f"STRAYLIGHT_RELEASE_REVISION={revision}",
                        f"DD_VERSION={revision}",
                        "DD_ENV=production",
                        "STRAYLIGHT_EMBEDDING_PROVIDER=openai",
                        "STRAYLIGHT_ALLOW_DEGRADED_EMBEDDINGS=false",
                        "STRAYLIGHT_EMBEDDING_MODEL=text-embedding-3-small",
                        "STRAYLIGHT_EMBEDDING_DIMENSIONS=1536",
                        "STRAYLIGHT_CAPTURE_MODEL=gpt-5.6",
                        "STRAYLIGHT_CAPTURE_MAX_OUTPUT_TOKENS=8192",
                        "STRAYLIGHT_DREAM_MODEL=gpt-5.6",
                        "STRAYLIGHT_MATERIALIZE_TOKEN_BUDGET=24000",
                        "OPENAI_BASE_URL=https://api.openai.com/v1",
                        "STRAYLIGHT_DREAM_SCHEDULER_ENABLED=true",
                        "STRAYLIGHT_METRICS_ENABLED=true",
                        "STRAYLIGHT_DOGSTATSD_ADDR=datadog-agent:8125",
                        "STRAYLIGHT_REQUESTS_PER_MINUTE=600",
                        "STRAYLIGHT_REQUEST_TIMEOUT_SECONDS=30",
                        "STRAYLIGHT_TRANSFER_TIMEOUT_SECONDS=3600",
                        "STRAYLIGHT_MAX_CONCURRENT_TRANSFERS=8",
                        "STRAYLIGHT_READINESS_TIMEOUT_SECONDS=3",
                        "STRAYLIGHT_METRICS_FLUSH_SECONDS=3",
                        "STRAYLIGHT_BACKUP_RETENTION_DAYS=30",
                        "STRAYLIGHT_ACCOUNT_DELETION_BACKUP_RETENTION_DAYS=30",
                        "STRAYLIGHT_DATADOG_NOTIFY=ops@straylight.dev",
                        "STRAYLIGHT_ALLOWED_ORIGINS=",
                        "STRAYLIGHT_PUBLIC_HOST=alpha.straylight.dev",
                        "STRAYLIGHT_ACME_EMAIL=ops@straylight.dev",
                        "STRAYLIGHT_DATABASE_IMAGE=database@sha256:" + ("d" * 64),
                        "STRAYLIGHT_OBJECT_STORE_IMAGE=object-store@sha256:"
                        + ("e" * 64),
                        "STRAYLIGHT_OBJECT_STORE_CLIENT_IMAGE=object-store-client@sha256:"
                        + ("f" * 64),
                        "STRAYLIGHT_API_IMAGE=straylight-api@sha256:" + ("1" * 64),
                        "STRAYLIGHT_WEB_IMAGE=straylight-web@sha256:" + ("2" * 64),
                        "STRAYLIGHT_MCP_IMAGE=straylight-mcp@sha256:" + ("3" * 64),
                        "STRAYLIGHT_EDGE_IMAGE=straylight-caddy@sha256:" + ("4" * 64),
                        "DATADOG_AGENT_IMAGE=agent@sha256:" + ("b" * 64),
                        "STRAYLIGHT_SECRETS_DIR=./secrets",
                        "",
                    ]
                )
            )
            validator = ROOT / "scripts/validate-production-config.sh"
            accepted = subprocess.run(
                [str(validator), str(env_file)],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertEqual(0, accepted.returncode, accepted.stderr)

            env_file.write_text(
                env_file.read_text().replace(
                    "alpha.straylight.dev", "memory.example.com"
                )
            )
            rejected = subprocess.run(
                [str(validator), str(env_file)],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertNotEqual(0, rejected.returncode)
            self.assertIn("placeholder", rejected.stderr)

            mutable = env_file.read_text().replace(
                "STRAYLIGHT_API_IMAGE=straylight-api@sha256:" + ("1" * 64),
                "STRAYLIGHT_API_IMAGE=straylight-api:latest",
            )
            env_file.write_text(
                mutable.replace("memory.example.com", "alpha.straylight.dev")
            )
            rejected = subprocess.run(
                [str(validator), str(env_file)],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertNotEqual(0, rejected.returncode)
            self.assertIn("pinned by sha256 digest", rejected.stderr)

            env_file.write_text(
                env_file.read_text()
                .replace(
                    "STRAYLIGHT_API_IMAGE=straylight-api:latest",
                    "STRAYLIGHT_API_IMAGE=straylight-api@sha256:" + ("1" * 64),
                )
                .replace(
                    "STRAYLIGHT_MATERIALIZE_TOKEN_BUDGET=24000",
                    "STRAYLIGHT_MATERIALIZE_TOKEN_BUDGET=12000",
                )
            )
            rejected = subprocess.run(
                [str(validator), str(env_file)],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertNotEqual(0, rejected.returncode)
            self.assertIn(
                "must remain 24000 for this release contract", rejected.stderr
            )

            env_file.write_text(
                env_file.read_text()
                .replace(
                    "STRAYLIGHT_MATERIALIZE_TOKEN_BUDGET=12000",
                    "STRAYLIGHT_MATERIALIZE_TOKEN_BUDGET=24000",
                )
                .replace(
                    "STRAYLIGHT_METRICS_ENABLED=true",
                    "STRAYLIGHT_METRICS_ENABLED=false",
                )
            )
            rejected = subprocess.run(
                [str(validator), str(env_file)],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertNotEqual(0, rejected.returncode)
            self.assertIn(
                "must remain true for this release contract", rejected.stderr
            )

    def test_managed_s3_validator_needs_no_minio_secrets_and_fails_closed(self):
        revision = "a" * 40
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            secrets = directory / "secrets"
            secrets.mkdir(mode=0o700)
            secrets.chmod(0o700)
            admin_password = "a" * 24
            rw_password = "r" * 24
            ro_password = "o" * 24
            values = {
                "postgres_admin_password": admin_password,
                "postgres_app_rw_password": rw_password,
                "postgres_app_ro_password": ro_password,
                "database_url_rw": (
                    f"postgres://app_rw:{rw_password}@db:5432/straylight"
                ),
                "database_url_ro": (
                    f"postgres://app_ro:{ro_password}@db:5432/straylight"
                ),
                "database_url_admin": (
                    f"postgres://admin:{admin_password}@db:5432/straylight"
                ),
                "continuation_signing_key": "c" * 32,
                "openai_api_key": "sk-unit-" + ("o" * 32),
                "dd_api_key": "d" * 32,
            }
            for name, value in values.items():
                path = secrets / name
                path.write_text(value)
                path.chmod(0o600)

            env_file = directory / "production.env"
            env_file.write_text(
                "\n".join(
                    [
                        "STRAYLIGHT_ENV=production",
                        "STRAYLIGHT_OBJECT_STORE_MODE=managed-s3",
                        f"STRAYLIGHT_RELEASE_REVISION={revision}",
                        f"DD_VERSION={revision}",
                        "DD_ENV=production",
                        "STRAYLIGHT_EMBEDDING_PROVIDER=openai",
                        "STRAYLIGHT_ALLOW_DEGRADED_EMBEDDINGS=false",
                        "STRAYLIGHT_EMBEDDING_MODEL=text-embedding-3-small",
                        "STRAYLIGHT_EMBEDDING_DIMENSIONS=1536",
                        "STRAYLIGHT_CAPTURE_MODEL=gpt-5.6",
                        "STRAYLIGHT_CAPTURE_MAX_OUTPUT_TOKENS=8192",
                        "STRAYLIGHT_DREAM_MODEL=gpt-5.6",
                        "STRAYLIGHT_MATERIALIZE_TOKEN_BUDGET=24000",
                        "OPENAI_BASE_URL=https://api.openai.com/v1",
                        "STRAYLIGHT_DREAM_SCHEDULER_ENABLED=true",
                        "STRAYLIGHT_METRICS_ENABLED=true",
                        "STRAYLIGHT_DOGSTATSD_ADDR=datadog-agent:8125",
                        "STRAYLIGHT_REQUESTS_PER_MINUTE=600",
                        "STRAYLIGHT_REQUEST_TIMEOUT_SECONDS=30",
                        "STRAYLIGHT_TRANSFER_TIMEOUT_SECONDS=3600",
                        "STRAYLIGHT_MAX_CONCURRENT_TRANSFERS=8",
                        "STRAYLIGHT_READINESS_TIMEOUT_SECONDS=3",
                        "STRAYLIGHT_METRICS_FLUSH_SECONDS=3",
                        "STRAYLIGHT_BACKUP_RETENTION_DAYS=30",
                        "STRAYLIGHT_ACCOUNT_DELETION_BACKUP_RETENTION_DAYS=30",
                        "STRAYLIGHT_DATADOG_NOTIFY=ops@carrystate.dev",
                        "STRAYLIGHT_ALLOWED_ORIGINS=",
                        "STRAYLIGHT_PUBLIC_HOST=alpha.carrystate.dev",
                        "STRAYLIGHT_ACME_EMAIL=ops@carrystate.dev",
                        "STRAYLIGHT_S3_ENDPOINT=",
                        "STRAYLIGHT_S3_REGION=us-west-2",
                        "STRAYLIGHT_S3_BUCKET=carrystate-production",
                        "STRAYLIGHT_S3_FORCE_PATH_STYLE=false",
                        "STRAYLIGHT_S3_CREATE_BUCKET=false",
                        "STRAYLIGHT_S3_ACCESS_KEY=",
                        "STRAYLIGHT_S3_SECRET_KEY=",
                        "STRAYLIGHT_S3_ACCESS_KEY_FILE=",
                        "STRAYLIGHT_S3_SECRET_KEY_FILE=",
                        "STRAYLIGHT_MANAGED_BACKUP_ROOT=/var/backups/carrystate",
                        "STRAYLIGHT_DATABASE_IMAGE=database@sha256:"
                        + ("d" * 64),
                        "STRAYLIGHT_API_IMAGE=straylight-api@sha256:"
                        + ("1" * 64),
                        "STRAYLIGHT_WEB_IMAGE=straylight-web@sha256:"
                        + ("2" * 64),
                        "STRAYLIGHT_MCP_IMAGE=straylight-mcp@sha256:"
                        + ("3" * 64),
                        "STRAYLIGHT_EDGE_IMAGE=straylight-caddy@sha256:"
                        + ("4" * 64),
                        "DATADOG_AGENT_IMAGE=agent@sha256:" + ("b" * 64),
                        "STRAYLIGHT_SECRETS_DIR=./secrets",
                        "",
                    ]
                )
            )
            validator = ROOT / "scripts/validate-production-config.sh"
            accepted = subprocess.run(
                [str(validator), str(env_file)],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertEqual(0, accepted.returncode, accepted.stderr)
            self.assertFalse(any(path.name.startswith("minio") for path in secrets.iterdir()))

            env_file.write_text(
                env_file.read_text().replace(
                    "STRAYLIGHT_S3_ACCESS_KEY=",
                    "STRAYLIGHT_S3_ACCESS_KEY=one-sided-key",
                )
            )
            rejected = subprocess.run(
                [str(validator), str(env_file)],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertNotEqual(0, rejected.returncode)
            self.assertIn("direct keys must be empty", rejected.stderr)

            env_file.write_text(
                env_file.read_text()
                .replace(
                    "STRAYLIGHT_S3_ACCESS_KEY=one-sided-key",
                    "STRAYLIGHT_S3_ACCESS_KEY=",
                )
                .replace(
                    "STRAYLIGHT_S3_ACCESS_KEY_FILE=",
                    "STRAYLIGHT_S3_ACCESS_KEY_FILE=/run/secrets/s3-access-key",
                )
            )
            rejected = subprocess.run(
                [str(validator), str(env_file)],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertNotEqual(0, rejected.returncode)
            self.assertIn("files must both be set or both omitted", rejected.stderr)

            env_file.write_text(
                env_file.read_text()
                .replace(
                    "STRAYLIGHT_S3_ACCESS_KEY_FILE=/run/secrets/s3-access-key",
                    "STRAYLIGHT_S3_ACCESS_KEY_FILE=",
                )
                + "STRAYLIGHT_MINIO_ENDPOINT=http://minio:9000\n"
            )
            rejected = subprocess.run(
                [str(validator), str(env_file)],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertNotEqual(0, rejected.returncode)
            self.assertIn(
                "STRAYLIGHT_MINIO_ENDPOINT must be empty in managed-s3 mode",
                rejected.stderr,
            )

    def test_backup_supports_production_override_and_file_backed_store_secrets(self):
        backup = (ROOT / "scripts/backup.sh").read_text()
        restore = (ROOT / "scripts/restore-drill.sh").read_text()
        restore_compose = json.loads(
            subprocess.run(
                [
                    "docker",
                    "compose",
                    "--env-file",
                    str(ROOT / ".env.example"),
                    "--file",
                    str(ROOT / "compose.yaml"),
                    "--file",
                    str(ROOT / "compose.restore-drill.yaml"),
                    "config",
                    "--format",
                    "json",
                ],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=True,
            ).stdout
        )
        self.assertIn("COMPOSE_OVERRIDE_FILE", backup)
        self.assertIn("load_secret MINIO_ROOT_USER", backup)
        self.assertIn("load_secret MINIO_ROOT_PASSWORD", backup)
        self.assertIn("COMPOSE_OVERRIDE_FILE", restore)
        self.assertIn("load_secret MINIO_ROOT_USER", restore)
        self.assertIn("load_secret MINIO_ROOT_PASSWORD", restore)
        self.assertNotIn("docker cp", restore)
        self.assertRegex(restore, r"docker exec -i[\s\S]*?pg_restore")
        self.assertIn("verify_host_stability", restore)
        self.assertIn(".RestartCount", restore)
        self.assertEqual(
            str(3 * 1024**3), restore_compose["services"]["db"]["mem_limit"]
        )
        self.assertEqual(
            str(1024**3), restore_compose["services"]["minio"]["mem_limit"]
        )
        self.assertEqual(
            str(1024**3), restore_compose["services"]["api"]["mem_limit"]
        )

    def test_backup_manifest_captures_runtime_and_compose_identity(self):
        backup = (ROOT / "scripts/backup.sh").read_text()
        verify = (ROOT / "scripts/verify-backup.sh").read_text()
        for evidence in [
            "runtime-images.json",
            "compose-service-hashes.txt",
            "database-invariants.json",
            "database-object-pinning.json",
            "database-object-verification.json",
        ]:
            self.assertIn(evidence, backup)
            self.assertIn(evidence, verify)
        self.assertIn("immutable_image_id", backup)
        self.assertIn("runtime_identity", backup)
        self.assertIn("scripts/database-invariants.sql", backup)
        self.assertIn("straylight-postgres-healthcheck", (ROOT / "scripts/restore-drill.sh").read_text())

    def test_backup_normalizes_relative_destination_before_docker_mount(self):
        backup = (ROOT / "scripts/backup.sh").read_text()
        create_root = backup.index('mkdir -p "$backup_root"')
        normalize = backup.index(
            'backup_root=$(CDPATH= cd -- "$backup_root" && pwd)',
            create_root,
        )
        work_dir = backup.index('work_dir="$backup_root/', normalize)
        docker_mount = backup.index('--volume "$work_dir:/backup"', work_dir)
        self.assertLess(create_root, normalize)
        self.assertLess(normalize, work_dir)
        self.assertLess(work_dir, docker_mount)

    def test_deploy_and_rollback_paths_are_executable_and_gated(self):
        scripts = {
            name: ROOT / "scripts" / name
            for name in [
                "check-public-health.sh",
                "deploy-production.sh",
                "qualify-object-store.sh",
                "rollback-production.sh",
                "verify-production-images.sh",
                "verify-rollback-compatibility.sh",
            ]
        }
        for name, path in scripts.items():
            self.assertTrue(path.stat().st_mode & 0o111, name)

        deploy = scripts["deploy-production.sh"].read_text()
        self.assertIn("verify-production-images.sh", deploy)
        self.assertIn("scripts/backup.sh", deploy)
        self.assertIn("qualify-object-store.sh", deploy)
        self.assertIn("check-public-health.sh", deploy)
        self.assertIn("production deployment requires a clean Git worktree", deploy)
        stop_writers = deploy.index("compose stop api worker")
        migrate = deploy.index(
            "compose up -d --no-build --pull never --force-recreate migrate"
        )
        start_candidate = deploy.index(
            "compose up -d --no-build --pull never api worker web datadog-agent edge"
        )
        self.assertLess(stop_writers, migrate)
        self.assertLess(migrate, start_candidate)

        rollback = scripts["rollback-production.sh"].read_text()
        self.assertIn("verify-rollback-compatibility.sh", rollback)
        self.assertIn("check-public-health.sh", rollback)
        self.assertNotIn("down --volumes", rollback)

        compatibility = scripts["verify-rollback-compatibility.sh"].read_text()
        self.assertIn('if [ -n "$compose_override_file" ]', compatibility)
        self.assertIn("/ready", compatibility)

    def test_deploy_steps_propagate_injected_and_real_failures(self):
        deploy = (ROOT / "scripts/deploy-production.sh").read_text()
        step_helper = ROOT / "scripts/deploy-steps.sh"
        self.assertIn('scripts/deploy-steps.sh"', deploy)
        candidate = deploy.split("deploy_candidate() {", 1)[1].split(
            "\n}\n\nif ! deploy_candidate", 1
        )[0]
        expected_steps = (
            "database-start",
            "database-ready",
            "object-store-start",
            "object-store-ready",
            "object-store-init-start",
            "object-store-init-complete",
            "object-store-qualify",
            "writers-stop",
            "migration-start",
            "migration-complete",
            "candidate-start",
            "api-ready",
            "web-ready",
            "datadog-ready",
            "edge-ready",
            "worker-ready",
            "public-health",
        )
        for step in expected_steps:
            self.assertRegex(
                candidate,
                rf"deploy_step {re.escape(step)}[\s\S]{{0,220}}"
                r"\|\|\s+return \$\?",
            )
            with tempfile.TemporaryDirectory() as temporary:
                marker = Path(temporary) / "reached"
                injected = subprocess.run(
                    [
                        "/bin/sh",
                        "-c",
                        """
. "$1"
marker_path=$2
fault_step=$3
run_command() {
  printf reached >"$marker_path"
}
candidate() {
  deploy_step "$fault_step" run_command || return $?
  printf continued >"$marker_path"
}
STRAYLIGHT_DEPLOY_FAIL_STEP=$fault_step
export STRAYLIGHT_DEPLOY_FAIL_STEP
if candidate; then
  exit 90
else
  status=$?
fi
[ "$status" -eq 97 ] || exit 91
[ ! -e "$marker_path" ] || exit 92
""",
                        "deploy-fault-test",
                        str(step_helper),
                        str(marker),
                        step,
                    ],
                    text=True,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    check=False,
                )
                self.assertEqual(0, injected.returncode, injected.stderr)

        command_failure = subprocess.run(
            [
                "/bin/sh",
                "-c",
                """
. "$1"
fail_command() {
  return 23
}
if deploy_step command-failure fail_command; then
  exit 90
else
  status=$?
fi
[ "$status" -eq 23 ]
""",
                "deploy-command-test",
                str(step_helper),
            ],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(0, command_failure.returncode, command_failure.stderr)

    def test_backups_quiesce_migrate_pin_verify_then_snapshot(self):
        for name in ("backup.sh", "managed-s3-backup.sh"):
            script = (ROOT / "scripts" / name).read_text()
            stop_flag = script.index("writers_stop_attempted=true")
            stop = script.index("compose stop --timeout 30 api worker", stop_flag)
            migrate = script.index("compose run --rm migrate", stop)
            active_uploads = script.index("active_uploads=", migrate)
            pin = script.index(
                "object-store-backup pin-database",
                active_uploads,
            )
            verify = script.index(
                "object-store-backup verify-database",
                pin,
            )
            snapshot = script.index("capturing PostgreSQL snapshot", verify)
            self.assertLess(stop_flag, stop)
            self.assertLess(stop, migrate)
            self.assertLess(migrate, active_uploads)
            self.assertLess(active_uploads, pin)
            self.assertLess(pin, verify)
            self.assertLess(verify, snapshot)
            self.assertIn("wait_original_container_ready", script)
            self.assertIn(
                '.recovery-operation.lock"',
                script,
            )

    def test_release_fingerprint_uses_the_images_built_for_the_candidate(self):
        fingerprint = (ROOT / "scripts/fingerprint-release.sh").read_text()
        self.assertIn('release_env_file="$work_dir/.candidate.env"', fingerprint)
        for component in [
            "api",
            "web",
            "mcp",
            "postgres",
            "minio",
            "minio-client",
            "caddy",
        ]:
            self.assertIn(f'"straylight-{component}:$revision"', fingerprint)
        self.assertIn('grep -v \'^straylight-\'', fingerprint)

    def test_production_example_requires_digest_pinned_release_images(self):
        values = {}
        for line in (ROOT / "production.env.example").read_text().splitlines():
            if "=" not in line or line.startswith("#"):
                continue
            key, value = line.split("=", 1)
            values[key] = value
        for name in [
            "STRAYLIGHT_API_IMAGE",
            "STRAYLIGHT_DATABASE_IMAGE",
            "STRAYLIGHT_WEB_IMAGE",
            "STRAYLIGHT_MCP_IMAGE",
            "STRAYLIGHT_EDGE_IMAGE",
            "STRAYLIGHT_OBJECT_STORE_IMAGE",
            "STRAYLIGHT_OBJECT_STORE_CLIENT_IMAGE",
        ]:
            self.assertRegex(values[name], r"@sha256:[0-9a-f]{64}$", name)

    def test_ci_green_requires_live_stack_and_restore_gates(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        self.assertIn("integration:", workflow)
        self.assertIn("tests/live_api_smoke.py", workflow)
        self.assertIn("tests/live_alpha_safety.py", workflow)
        self.assertIn("tests/live_runtime_safety.py", workflow)
        self.assertIn("scripts/verify-rollback-compatibility.sh", workflow)
        self.assertIn("scripts/qualify-object-store.sh", workflow)
        self.assertIn("scripts/restore-drill.sh", workflow)
        self.assertRegex(
            workflow,
            r"needs: \[rust, web, mcp, contracts, integration, supply-chain\]",
        )

    def test_datadog_agent_has_no_host_control_mounts_and_uses_a_secret(self):
        agent = self.compose["services"]["datadog-agent"]
        self.assertEqual([], agent.get("volumes", []))
        self.assertEqual("ENC[dd_api_key]", agent["environment"]["DD_API_KEY"])
        self.assertEqual(
            "docker.secrets", agent["environment"]["DD_SECRET_BACKEND_TYPE"]
        )
        self.assertIn(
            "/run/secrets/dd_api_key",
            {
                item["target"]
                for item in agent["secrets"]
            },
        )

    def test_production_secret_initializer_creates_a_valid_private_bundle(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            openai_key = directory / "openai.key"
            datadog_key = directory / "datadog.key"
            openai_key.write_text("sk-unit-" + ("o" * 32))
            datadog_key.write_text("d" * 32)
            destination = directory / "generated"
            result = subprocess.run(
                [
                    str(ROOT / "scripts/init-production-secrets.sh"),
                    str(destination),
                    str(openai_key),
                    str(datadog_key),
                ],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertEqual(0, result.returncode, result.stderr)
            self.assertEqual(0o700, destination.stat().st_mode & 0o777)
            self.assertNotIn(openai_key.read_text(), result.stdout + result.stderr)
            self.assertNotIn(datadog_key.read_text(), result.stdout + result.stderr)
            self.assertEqual(
                {
                    "continuation_signing_key",
                    "database_url_admin",
                    "database_url_ro",
                    "database_url_rw",
                    "dd_api_key",
                    "minio_app_access_key",
                    "minio_app_secret_key",
                    "minio_root_password",
                    "minio_root_user",
                    "openai_api_key",
                    "postgres_admin_password",
                    "postgres_app_ro_password",
                    "postgres_app_rw_password",
                },
                {path.name for path in destination.iterdir()},
            )
            self.assertTrue(
                all((path.stat().st_mode & 0o777) == 0o600 for path in destination.iterdir())
            )


if __name__ == "__main__":
    unittest.main()
