# Vendored crates

## `mlua` 0.11.6

The published crate, MIT-licensed (`mlua/LICENSE`), with one change in
`src/luau/heap_dump.rs`: `HeapDump::new` answers `None` on
`wasm32-unknown-unknown`. There the `libc` crate declares no stdio, so the heap
dump's `tmpfile`/`fread` calls do not compile. The imports only the dump uses,
and `src/luau/json.rs`'s parser, are allowed unused there. 0.12.2 has the same
calls.

Only the browser player's workspace builds this copy (its `[patch.crates-io]`);
the app builds the published crate. When `mastik-vm`'s pin moves, take the new
version's source from crates.io and apply the same change. Delete this copy once
upstream gates the heap dump on a target with stdio.
