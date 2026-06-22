#!/usr/bin/env bash
# Regenerate aip-proto's committed Rust + descriptor set from the BSR (ADR-0011).
#
# This is the single source of truth for the generated artifacts, and it is
# reproducible: it regenerates against the googleapis digest already pinned in
# `buf.lock`, so re-running it with no local changes is a no-op. CI's drift check
# runs it and asserts `git diff --exit-code`. It needs the buf toolchain and
# network access to the Buf Schema Registry — consumers do not, because the
# output is committed.
#
#   1. `buf generate`    — neoeinstein-prost emits the prost structs (committed),
#                          resolving deps from the locked digest.
#   2. `buf build`       — emit the combined, import-complete FileDescriptorSet
#                          that backs the runtime DescriptorPool (so extension
#                          annotations like google.api.resource are readable).
#                          Source info is excluded: proto comments/locations are
#                          ~85% of the bytes and nothing at runtime reads them.
#
# Bumping the pinned digest is a *separate*, deliberate step — `buf dep update`
# rewrites buf.lock to the latest googleapis commit — kept out of this script so
# the drift check stays reproducible (it would otherwise report drift the moment
# googleapis publishes anything). After a bump, just re-run this script: the
# `ReflectMessage` impls are emitted by protoc-gen-prost-aip from the schema, so
# there is no hand-maintained message list to refresh (issue #191).
#
# The empty `aip.proto` anchor package (proto/imports.proto) generates a
# comment-only placeholder — it exists only to pull the googleapis imports into
# generation — which the no-code sweep below drops (the same sweep as
# examples/freight-server/regen.sh, which also sheds extern_path placeholders).
set -euo pipefail
cd "$(dirname "$0")"

rm -rf src/gen
buf generate --include-imports

# Drop every generated file with no code line (no line that starts with
# anything but a `//` comment).
grep -rLZ '^[^/]' src/gen --include='*.rs' | xargs -0r rm --

# Drop the google.rpc ReflectMessage orphan. google/longrunning/operations.proto
# imports google/rpc/status.proto (Operation.error), so --include-imports pulls it
# in; neoeinstein-prost suppresses its structs (extern-mapped onto tonic-types in
# buf.gen.yaml), but the aip plugin still emits a `.aip.rs` ReflectMessage orphan
# for the extern'd Status (it has a body, so the no-code sweep above misses it).
# Remove only that orphan reflect impl, not the whole package: a google.rpc type
# that is later NOT extern-mapped would still get its struct generated and kept.
# The google.rpc descriptors stay in the descriptor set below (reflection on
# Operation needs them); the empty dir is swept away by the prune below.
rm -f src/gen/google/rpc/*.aip.rs

find src/gen -type d -empty -delete

buf build --as-file-descriptor-set --exclude-source-info -o src/descriptor_set.binpb
