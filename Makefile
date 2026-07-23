SHELL := /bin/sh
ENV_FILE ?= .env
COMPOSE := docker compose --env-file $(ENV_FILE) -f compose.yaml

.PHONY: config pull build up down ps logs migrate mcp db-shell minio-version validate

config:
	@test -f $(ENV_FILE) || { echo "missing $(ENV_FILE); start from .env.example" >&2; exit 1; }
	$(COMPOSE) config --quiet

pull:
	$(COMPOSE) pull db minio-init

build:
	$(COMPOSE) build minio migrate api worker web

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
	$(COMPOSE) exec db psql -U admin -d $${POSTGRES_DB:-straylight}

minio-version:
	$(COMPOSE) run --rm --no-deps minio --version

validate: config
	@echo "Compose configuration is valid."
