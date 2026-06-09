#!/usr/bin/env bash
# Regenerate aip-proto's committed Rust + descriptor set from the BSR (ADR-0011).
#
# This is the single source of truth for the generated artifacts; CI's drift
# check runs it and asserts `git diff --exit-code`. It needs the buf toolchain
# and network access to the Buf Schema Registry — consumers do not, because the
# output is committed.
#
#   1. `buf dep update`  — refresh buf.lock to the pinned googleapis digest.
#   2. `buf generate`    — neoeinstein-prost emits the prost structs (committed).
#   3. `buf build`       — emit the combined, import-complete FileDescriptorSet
#                          that backs the runtime DescriptorPool (so extension
#                          annotations like google.api.resource are readable).
#
# The empty `aip.proto` anchor package (proto/imports.proto) is dropped — it
# exists only to pull the googleapis imports into generation.
set -euo pipefail
cd "$(dirname "$0")"

buf dep update
rm -rf src/gen
buf generate --include-imports
rm -rf src/gen/aip
buf build --as-file-descriptor-set -o src/descriptor_set.binpb
