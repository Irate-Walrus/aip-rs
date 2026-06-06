# aip-rs — working notes for contributors and agents

Orientation before you change anything:

- **`CONTEXT.md`** — the domain glossary. Use these exact terms in names, docs,
  and commit messages.
- **`docs/adr/`** — recorded design decisions. Read the relevant ADR before
  changing a subsystem.

## Every issue extends the example server

[`examples/freight-server`](examples/freight-server) is a runnable gRPC demo that
must grow with the library — it is how we prove the primitives work together and
gives us something to test against. **Implementing a feature is not done until
the example server uses it.**

When you implement (or extend) a feature crate as part of an issue:

1. Find the matching `TODO(aip #N)` seam(s) in
   [`examples/freight-server/src/service.rs`](examples/freight-server/src/service.rs).
   Replace the naive placeholder with a real call into the crate you just built,
   then delete the `TODO`.
2. If the feature unblocks a stubbed method — one returning `Unimplemented`, i.e.
   the Site/Shipment/batch handlers — implement it, following the Shipper
   handlers as the worked reference. Add the resource's storage to
   `examples/freight-server/src/storage.rs` as needed.
3. Keep the example compiling and runnable: `cargo run -p freight-server` must
   start, and the affected RPCs must behave correctly. The README has
   copy-paste `grpcurl` commands to check them.
4. Update the status table in
   [`examples/freight-server/README.md`](examples/freight-server/README.md).

See [`docs/adr/0006-example-freight-server.md`](docs/adr/0006-example-freight-server.md)
for why the demo exists and how it is meant to evolve.
