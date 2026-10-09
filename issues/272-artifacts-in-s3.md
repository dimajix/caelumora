# 272 — Artifacts can be kept only on the server's own disk, not in S3 or the like

**Status:** open
**Area:** Context and knowledge

## The problem

Published artifacts (issue [271](271-publishing-artifacts.md)) keep their metadata in the
`EntityStore` and their content in a configured artifact storage (`ArtifactStorage`, one per
configured storage, as `artifacts.storage` names them). Only a `local` storage is foreseen: a
directory on the server's host.

An artifact is kept longer than a workspace, and is meant to outlive it. Over time artifacts add up,
and a server's disk is the wrong place for content that is to be kept, backed up and scaled
independently of the server - and, once Caelumora runs in several instances (issue
[122](122-several-instances.md)), a local directory is reachable from one of them only.

## Why it matters

Object storage - Amazon S3 and the services that speak its API (MinIO, Ceph, Cloudflare R2, the
object stores of most cloud providers) - is where content of this kind is usually kept: cheap,
durable, independent of the server, reachable from every instance.

## Considerations

**A second provider of `ArtifactStorage`.** 271 defines the trait - put, get as a stream, delete, by
key, without permissions, which are the `ArtifactManager`'s - and the `local` provider. An `s3`
provider beside it: a bucket, a prefix, an endpoint (for services other than AWS), a region, and
credentials from the secrets (`SecretManager`), never from the configuration file. Nothing above
the trait changes; an artifact's document names the storage it lies in.

**Implementation.** Two candidates:

- the `object_store` crate (Apache Arrow): local file system, S3 and S3-compatible services, GCS
  and Azure behind one interface, with streaming uploads and downloads. `ArtifactStorage` would be a
  thin layer over it, and GCS and Azure would come at little cost. To check: its version, the weight
  of its dependencies, and whether `local` should move onto it as well, so that both providers
  share one implementation.
- the AWS SDK (`aws-sdk-s3`): complete, but heavy, and AWS-specific in its configuration.

**What may be put there.** A bucket at an external provider is a destination like any other: a
storage takes no artifact classified above what may flow to it - the `destination` of a workspace
storage (issue [251](251-open-terminal.md), part B2), asked as publishing there. A storage without a
`destination` should take nothing above `public`, as for workspace storages with a shell.

**Downloads.** 271 streams a download through the server, which asks for permission on every
request. Presigned URLs - a redirect to a time-limited link straight to the bucket - would take the
traffic off the server, but the link works for whoever has it until it expires, and the headers
that keep a download from being shown inline (`Content-Disposition: attachment`, `nosniff`) must
then be forced through the request. An option per storage, if at all; not the default.

**Errors and logs.** An endpoint and a bucket name may say more than they should: errors and logs
name the storage by its configured name and redact the endpoint, as remote integrations do (issue
[188](188-remote-helpers-on-llm-error.md)).

**Tests.** The provider is tested against a stand-in - an in-memory implementation of the S3 API,
or `object_store`'s own -, not against a real bucket; a test against a real service can be
offered behind a feature or environment variable.

## Done when

An artifact storage can be configured as `s3` - AWS or an S3-compatible service - with credentials
from the secrets and a `destination`; artifacts are published to it and downloaded from it as from
a local storage; `CONFIGURATION.md` and `STORAGE.md` say so.

## History

- **2026-10-09** — Opened at the maintainer's request while deciding 271: artifacts are kept in a
  local storage only for now; object storage follows here.
