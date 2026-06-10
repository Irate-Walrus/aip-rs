#!/usr/bin/env bash
# Regenerate test-fixtures' committed descriptor set from the BSR (ADR-0011).
#
# Mirrors `crates/aip-proto/regen.sh` and `examples/freight-server/regen.sh`:
# reproducible against the googleapis digest pinned in `buf.lock`, so re-running
# with no local changes is a no-op. CI's drift check runs it and asserts
# `git diff --exit-code`. It needs the buf toolchain and network access to the
# Buf Schema Registry — consumers do not, because the output is committed.
#
# `buf build` emits the combined, import-complete FileDescriptorSet that backs
# the runtime DescriptorPool in `src/lib.rs` (so extension annotations like
# `google.api.field_behavior` stay readable, ADR-0009). Source info is excluded:
# proto comments/locations are ~85% of the bytes and nothing at runtime reads them.
set -euo pipefail
cd "$(dirname "$0")"

buf build --as-file-descriptor-set --exclude-source-info -o src/descriptor_set.binpb
