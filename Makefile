SHELL := /bin/sh
ENV_FILE ?= .env
BACKUP_ROOT ?= backups
COMPOSE := docker compose --env-file $(ENV_FILE) -f compose.yaml

.PHONY: config production-config managed-production-config production-images production-secrets pull build up down ps logs migrate mcp db-shell minio-version object-store-check backup production-backup managed-production-backup backup-prune restore-drill production-restore-drill managed-production-restore-drill production-deploy production-rollback rollback-compatibility public-health observability-up observability-status observability-logs datadog-configure datadog-validate release-artifacts validate

config:
	@test -f $(ENV_FILE) || { echo "missing $(ENV_FILE); start from .env.example" >&2; exit 1; }
	$(COMPOSE) config --quiet

production-config:
	@test -f $(ENV_FILE) || { echo "missing $(ENV_FILE); start from .env.example" >&2; exit 1; }
	./scripts/validate-production-config.sh $(ENV_FILE)
	$(COMPOSE) -f compose.production.yaml config --quiet

managed-production-config:
	@test -f $(ENV_FILE) || { echo "missing $(ENV_FILE); start from production.managed-s3.env.example" >&2; exit 1; }
	./scripts/validate-production-config.sh $(ENV_FILE)
	$(COMPOSE) -f compose.production.yaml -f compose.managed-s3.yaml config --quiet

production-images:
	./scripts/verify-production-images.sh $(ENV_FILE)

production-secrets:
	@test -n "$(SECRETS_DIR)" || { echo "set SECRETS_DIR=/path/to/secrets" >&2; exit 1; }
	@test -n "$(OPENAI_KEY_FILE)" || { echo "set OPENAI_KEY_FILE=/path/to/key-file" >&2; exit 1; }
	@test -n "$(RESEND_KEY_FILE)" || { echo "set RESEND_KEY_FILE=/path/to/key-file" >&2; exit 1; }
	@test -n "$(DATADOG_KEY_FILE)" || { echo "set DATADOG_KEY_FILE=/path/to/key-file" >&2; exit 1; }
	@test -n "$(APNS_PRIVATE_KEY_FILE)" || { echo "set APNS_PRIVATE_KEY_FILE=/path/to/AuthKey.p8" >&2; exit 1; }
	./scripts/init-production-secrets.sh "$(SECRETS_DIR)" "$(OPENAI_KEY_FILE)" "$(RESEND_KEY_FILE)" "$(DATADOG_KEY_FILE)" "$(APNS_PRIVATE_KEY_FILE)"

pull:
	$(COMPOSE) pull --ignore-buildable

build:
	$(COMPOSE) build db minio minio-init migrate api worker mcp web edge

up: config
	$(COMPOSE) up -d --build

down:
	$(COMPOSE) down

ps:
	$(COMPOSE) ps

logs:
	$(COMPOSE) logs --tail=200 -f

migrate:
	$(COMPOSE) run --rm migrate

mcp: config
	$(COMPOSE) run --rm -T mcp

db-shell:
	$(COMPOSE) exec db psql -U admin -d $${POSTGRES_DB:-brunn}

minio-version:
	$(COMPOSE) run --rm --no-deps minio --version

object-store-check:
	ENV_FILE=$(ENV_FILE) ./scripts/qualify-object-store.sh

backup:
	ENV_FILE=$(ENV_FILE) ./scripts/backup.sh "$(BACKUP_ROOT)"

production-backup:
	ENV_FILE=$(ENV_FILE) COMPOSE_OVERRIDE_FILE=compose.production.yaml ./scripts/backup.sh "$(BACKUP_ROOT)"

managed-production-backup:
	ENV_FILE=$(ENV_FILE) ./scripts/managed-s3-backup.sh

backup-prune:
	./scripts/prune-backups.sh --apply "$(BACKUP_ROOT)"

restore-drill:
	@test -n "$(BACKUP_DIR)" || { echo "set BACKUP_DIR=/path/to/backup" >&2; exit 1; }
	ENV_FILE=$(ENV_FILE) ./scripts/restore-drill.sh "$(BACKUP_DIR)"

production-restore-drill:
	@test -n "$(BACKUP_DIR)" || { echo "set BACKUP_DIR=/path/to/backup" >&2; exit 1; }
	ENV_FILE=$(ENV_FILE) COMPOSE_OVERRIDE_FILE=compose.production.yaml ./scripts/restore-drill.sh "$(BACKUP_DIR)"

managed-production-restore-drill:
	@test -n "$(BACKUP_DIR)" || { echo "set BACKUP_DIR=/path/to/managed-backup" >&2; exit 1; }
	@test -n "$(DRILL_ENV_FILE)" || { echo "set DRILL_ENV_FILE=/path/to/dedicated-drill.env" >&2; exit 1; }
	./scripts/managed-s3-restore-drill.sh "$(BACKUP_DIR)" "$(DRILL_ENV_FILE)"

production-deploy:
	ENV_FILE=$(ENV_FILE) BACKUP_ROOT=$(BACKUP_ROOT) ./scripts/deploy-production.sh

production-rollback:
	ENV_FILE=$(ENV_FILE) ./scripts/rollback-production.sh

rollback-compatibility:
	@test -n "$(TARGET_API_IMAGE)" || { echo "set TARGET_API_IMAGE=<immutable-image>" >&2; exit 1; }
	ENV_FILE=$(ENV_FILE) ./scripts/verify-rollback-compatibility.sh "$(TARGET_API_IMAGE)"

public-health:
	@test -n "$(PUBLIC_HOST)" || { echo "set PUBLIC_HOST=<approved-hostname>" >&2; exit 1; }
	./scripts/check-public-health.sh "$(PUBLIC_HOST)"

observability-up: config
	@grep -Eq '^DD_API_KEY=.+$$' $(ENV_FILE) || { echo "DD_API_KEY is empty in $(ENV_FILE)" >&2; exit 1; }
	@grep -Eq '^BRUNN_METRICS_ENABLED=(true|1)$$' $(ENV_FILE) || { echo "set BRUNN_METRICS_ENABLED=true in $(ENV_FILE)" >&2; exit 1; }
	$(COMPOSE) --profile observability up -d --build datadog-agent api worker

observability-status:
	$(COMPOSE) --profile observability ps datadog-agent api worker

observability-logs:
	$(COMPOSE) --profile observability logs --tail=200 -f datadog-agent api worker

datadog-configure:
	@test -n "$${DD_API_KEY}" || { echo "DD_API_KEY is not set" >&2; exit 1; }
	@test -n "$${DD_APP_KEY}" || { echo "DD_APP_KEY is not set" >&2; exit 1; }
	python3 infra/datadog/configure_percentiles.py
	python3 infra/datadog/configure_monitors.py

datadog-validate:
	python3 infra/datadog/configure_percentiles.py --dry-run
	python3 infra/datadog/configure_monitors.py --dry-run

release-artifacts:
	./scripts/fingerprint-release.sh

validate: config
	@echo "Compose configuration is valid."
