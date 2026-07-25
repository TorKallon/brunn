# Straylight Object Store Evaluation

Status: managed cloud architecture approved; exact provider qualification pending

Straylight stores source and artifact blobs behind a versioned S3 interface.
Production uses a managed cloud object store. MinIO is reserved for local
development, destructive integration tests, and migration tooling. Provider
labels are not enough: every production candidate must pass the live
`object-store-check` contract before use.

## Application Configuration

The application uses the AWS SDK and supports two credential modes:

- If `STRAYLIGHT_S3_ACCESS_KEY_FILE` and `STRAYLIGHT_S3_SECRET_KEY_FILE` are
  both set, Straylight reads that explicit static credential pair from mounted
  secret files. Direct environment values remain available for local tooling
  but are rejected by the managed-production validator.
- If both are absent, Straylight leaves credential resolution to the AWS
  default chain. This supports standard AWS environment variables, shared
  profiles, web identity, ECS task identity, and EC2 instance identity without
  putting long-lived keys in Straylight configuration.

Setting only one explicit key is a configuration error. Every existing
`STRAYLIGHT_MINIO_*` endpoint, region, bucket, access-key, and secret-key name
remains a lower-precedence alias for local installations.

| Setting | Default | Purpose |
| --- | --- | --- |
| `STRAYLIGHT_S3_REGION` | `us-east-1` | Signing and bucket region; `STRAYLIGHT_MINIO_REGION` is an alias |
| `STRAYLIGHT_S3_BUCKET` | `straylight` | Existing object bucket; `STRAYLIGHT_MINIO_BUCKET` is an alias |
| `STRAYLIGHT_S3_ENDPOINT` | unset | Optional custom S3-compatible endpoint; `STRAYLIGHT_MINIO_ENDPOINT` is an alias |
| `STRAYLIGHT_S3_FORCE_PATH_STYLE` | `true` with a custom endpoint, otherwise `false` | Override bucket addressing; `STRAYLIGHT_MINIO_FORCE_PATH_STYLE` is an alias |
| `STRAYLIGHT_S3_CREATE_BUCKET` | `true` outside production, `false` in production | Allow the application to create a missing bucket; `STRAYLIGHT_MINIO_CREATE_BUCKET` is an alias |
| `STRAYLIGHT_S3_ACCESS_KEY_FILE` and `STRAYLIGHT_S3_SECRET_KEY_FILE` | unset | Optional mounted static credentials; direct values and the corresponding MinIO aliases remain available outside managed production |

For AWS S3 production, provision and version the bucket outside the
application, grant the runtime a workload identity, and omit the endpoint and
explicit Straylight key pair. Startup still performs `HeadBucket`; it fails
closed if the bucket is absent because production does not create buckets by
default. For local MinIO, the existing aliases continue to select the custom
endpoint, explicit app user, path-style addressing, and development bucket
creation behavior.

## Required Contract

The qualification command verifies:

- enabled bucket versioning;
- conditional create and content-addressed deduplication;
- exact payload and metadata round trips;
- version IDs for repeated writes;
- identified delete markers;
- complete version and delete-marker enumeration;
- exact-key and prefix purges of every version.

These operations protect provenance, idempotent capture, account export, and
complete account deletion. A provider that cannot pass all checks is not a
supported production target.

## Candidate Evidence

| Candidate | Contract | Image scan | Alpha assessment |
| --- | --- | --- | --- |
| Community MinIO `RELEASE.2025-10-15T17-29-55Z` rebuilt with Go 1.25.12 | Pass | 3 critical, 26 high | Development, destructive tests, and migration source only; not a production candidate |
| RustFS `1.0.0-beta.4` at `sha256:f7cb98ef492fa3c3ed0dbd65df3ce2dd205c666e24e7d7234d9402c9ed1001f9` | Pass | 0 critical, 7 fixable high | Technically compatible; beta maturity requires owner acceptance |
| Garage 2.x | Fail by documented capability | Not run | Unsupported because bucket versioning is absent |
| MinIO AIStor Free | Fail by licensed capability | Not run | Unsupported because version-specific deletion is withheld |
| MinIO AIStor Enterprise | Requires license-backed live qualification | Pending | Viable self-hosted candidate after commercial approval |
| AWS S3 | Requires credentialed live qualification | Pending | Recommended first production candidate |
| Another managed, versioned S3 provider | Requires credentialed live qualification | Pending | Supported only if the complete behavior contract passes |

The community MinIO rebuild reduced the scan from 4 critical and 40 high
findings to 3 critical and 26 high findings without changing MinIO source.
The remaining critical findings include two MinIO advisories with no community
fix and one vulnerable gRPC dependency. It remains a release blocker even
though the live behavior contract passes.

The separately hardened MinIO client image used for bucket initialization,
backup, restore, and qualification scans at zero critical and zero high
findings. This removes deployment tooling from the residual risk, but it does
not make the Community MinIO server acceptable.

RustFS passed in a capped, isolated, throwaway container without host ports.
Its seven high findings are fixable base-image packages; a hardened derived
image can address those if RustFS is selected. Its current beta release status
cannot be removed by packaging.

## Sources

- [Garage S3 compatibility](https://garagehq.deuxfleurs.fr/documentation/reference-manual/s3-compatibility/)
- [MinIO AIStor licenses](https://docs.min.io/aistor/operations/licenses/)
- [RustFS releases](https://github.com/rustfs/rustfs/releases)
- [RustFS Docker installation](https://docs.rustfs.com/installation/docker/)

## Decision

The production architecture optimizes for managed maturity and a lower
operations burden. Self-hosted object storage is no longer in the first
production path. AWS S3 is the leading candidate because it implements the
required versioning and deletion semantics without adding another stateful
service to operate.

Production remains blocked until the exact bucket, region, application
identity, encryption configuration, and versioning behavior pass the same live
qualification and security gates in the target environment. Backup policy,
cross-region copy, key custody, RPO, and RTO are intentionally deferred to the
separate recovery decision.
