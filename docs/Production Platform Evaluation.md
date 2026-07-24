# Straylight Production Platform Evaluation

Status: recommendation ready; owner selection pending

## Decision Context

The first production deployment serves the owner and a small invite-only
cohort. The data matters more than minimizing the hosting bill, but the
platform should not add operational machinery that does not improve safety,
reasoning quality, or token efficiency.

The platform must support:

- the existing Dockerized Rust API and worker plus TypeScript web application;
- managed PostgreSQL with pgvector;
- a managed, versioned cloud S3 object store;
- private service networking;
- Datadog DogStatsD metrics and structured-log delivery;
- one-time migrations, health checks, rollbacks, and immutable releases; and
- a portable path to a larger cloud deployment without changing the storage or
  agent-facing contracts.

MinIO remains the local development and destructive-test object store. It is
not a production candidate.

## Comparison

| Platform | Strengths for Straylight | Material drawbacks | Assessment |
| --- | --- | --- | --- |
| Render | Docker web, private, and worker services; fully managed Postgres with pgvector, backup, and HA options; private networking; native HTTPS log streaming to Datadog; declarative Blueprints | Requires an external S3 provider; does not run Compose directly; the strongest database protections require paid plans | Best alpha fit |
| Railway | Very fast service setup, good developer experience, Docker builds, private networking, and convenient environment references | Compose must be translated service by service; Railway Postgres templates are explicitly unmanaged; Railway Buckets lack object versioning and server-side encryption; external log delivery needs a separate forwarder; private DogStatsD uses additional IPv6 configuration | Fast, but a poor fit for Straylight's data and observability requirements |
| Fly.io | Strong Docker portability and regional placement; managed Postgres offers pgvector, HA, backups, and private networking | More infrastructure decisions and operational work; Datadog logs require a Vector-based log shipper; the production topology is less turnkey for a small cohort | Good technical runner-up when regional placement matters |
| AWS ECS/Fargate | Deepest integration across ECS, RDS PostgreSQL, S3, IAM, KMS, networking, backup, and Datadog; clearest long-term control path | Highest initial configuration, infrastructure-as-code, security-policy, and operating burden; slower iteration for the first few users | Strong eventual platform, unnecessary complexity for the first alpha |

## Recommendation

Use **Render for application compute and managed PostgreSQL**, **AWS S3 for
versioned object storage**, and **Datadog for metrics and logs**.

The initial service topology should be:

1. a public Render web service for the SPA and API edge;
2. a private Render API service if the edge remains separate;
3. a Render background worker;
4. a one-shot migration command on each release;
5. Render managed PostgreSQL with pgvector;
6. a private Datadog Agent service for DogStatsD;
7. Render's HTTPS Datadog log stream; and
8. a private AWS S3 bucket with versioning enabled and a least-privilege
   application identity.

This is the smallest platform that matches the current safety contract without
operating a database, object-store server, log forwarder, or general-purpose
host. The application remains portable because PostgreSQL, S3, Docker, and
DogStatsD are provider-neutral boundaries. Revisit AWS ECS/Fargate when the
cohort, compliance needs, traffic, or organization justify the added control.

No provider-specific deployment manifest should be treated as production-ready
until the owner accepts this recommendation and the exact Postgres and S3
products pass the existing live qualification gates.

## Primary Sources

- [Render service types](https://render.com/docs/service-types)
- [Render private services](https://render.com/docs/private-services)
- [Render Postgres](https://render.com/docs/postgresql)
- [Render log streams](https://render.com/docs/log-streams)
- [Render Blueprints](https://render.com/docs/infrastructure-as-code)
- [Railway Docker Compose mapping](https://docs.railway.com/guides/docker-compose)
- [Railway databases](https://docs.railway.com/databases)
- [Railway storage buckets](https://docs.railway.com/storage-buckets)
- [Railway third-party observability](https://docs.railway.com/guides/third-party-observability)
- [Fly.io managed Postgres](https://fly.io/docs/mpg/)
- [Fly.io log export](https://fly.io/docs/monitoring/exporting-logs/)
- [AWS Fargate or Lambda decision guide](https://docs.aws.amazon.com/decision-guides/latest/fargate-or-lambda/fargate-or-lambda.html)
