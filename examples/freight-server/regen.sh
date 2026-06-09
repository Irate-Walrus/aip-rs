#!/usr/bin/env bash
# Regenerate the freight example's committed Rust + descriptor set (ADR-0011).
#
# Mirrors `crates/aip-proto/regen.sh`: reproducible against the googleapis
# digest pinned in `buf.lock`, so re-running with no local changes is a no-op.
# It needs the buf toolchain, network access to the Buf Schema Registry, and a
# Rust toolchain (the resource-name plugin builds from this workspace) —
# consumers need none of that, because the output is committed.
#
#   1. `buf generate --include-imports` — neoeinstein-prost emits the
#      `einride.example.freight.v1` structs (`google.*` is extern_path'd onto
#      `aip-proto`, so no `google.*` message code is emitted); neoeinstein-tonic
#      emits the `FreightService` and `IAMPolicy` server traits (the
#      `proto/imports.proto` anchor is what brings `iam_policy.proto` into the
#      closure); and protoc-gen-prost-aip emits a typed resource-name wrapper
#      per `google.api.resource` annotation.
#   2. `buf build` — emit the combined, import-complete FileDescriptorSet that
#      backs the runtime DescriptorPool (so extension annotations like
#      `google.api.field_behavior` stay readable, ADR-0009) and doubles as the
#      grpcurl `-protoset` for both served services (see the README).
#      Source info is excluded: proto comments/locations are ~85% of the bytes
#      and nothing at runtime reads them.
set -euo pipefail
cd "$(dirname "$0")"

rm -rf src/gen
buf generate --include-imports

# extern_path'd packages (and the anchor) leave behind comment-only
# "@generated" placeholder files; drop every generated file with no code line
# (no line that starts with anything but a `//` comment).
grep -rLZ '^[^/]' src/gen --include='*.rs' | xargs -0r rm --
find src/gen -type d -empty -delete

buf build --as-file-descriptor-set --exclude-source-info -o src/descriptor_set.binpb
