# minidb migration plan

Original package: `packages/minidb`

Status: complete. All 27 TypeScript source modules have Rust counterparts in this crate; no migration placeholders remain.

The Rust crate preserves the original log-structured engine and its public responsibilities. It does not substitute SQLite, sled, or another database because that would change WAL/snapshot formats, recovery behavior, compaction, indexes, locking, and cluster semantics.

## Dependency order

1. Leaf utilities: `crc32`, `skiplist`, `query`, `rename-replace`, `wal`, `cluster/utils`.
2. Formats and in-memory state: `codec`, `store`, `index-manager`, `dt-index`, `compound-index`.
3. Persistence readers: `value-reader`, `snapshot`, `recovery`, `lockfile`.
4. Text search: `text-postings`, then `text-index`.
5. Compaction: `wal + snapshot + store + rename-replace`.
6. Embedded database: `MiniDb` composes all preceding modules.
7. RESP server.
8. Cluster: utilities, types, router/topology/shard/coordinator/lock-pool, then `ClusterDb`.

## Source mapping

- `index.ts` -> `minidb.rs`
- `server.ts` -> `server.rs`
- `codec.ts`, `crc32.ts`, `wal.ts` -> `codec.rs`, `crc32.rs`, `wal.rs`
- `store.ts`, `skiplist.ts`, `query.ts` -> `store.rs`, `skiplist.rs`, `query.rs`
- `index-manager.ts`, `dt-index.ts`, `compound-index.ts` -> `index_manager.rs`, `dt_index.rs`, `compound_index.rs`
- `text-index.ts`, `text-postings.ts` -> `text_index.rs`, `text_postings.rs`
- `snapshot.ts`, `recovery.ts`, `value-reader.ts` -> `snapshot.rs`, `recovery.rs`, `value_reader.rs`
- `compaction.ts`, `rename-replace.ts`, `lockfile.ts` -> `compaction.rs`, `rename_replace.rs`, `lockfile.rs`
- `cluster/*.ts` -> `cluster/*.rs`, preserving the router, topology, shard, coordinator, lock-pool, registry, and public `ClusterDb` boundaries.

## Rust dependency mapping

- Node asynchronous filesystem, timers, and TCP: Tokio.
- JavaScript objects and JSON codec/metadata: Serde and `serde_json::Value`.
- JavaScript regular expressions in JSON filters: `regex`.
- Domain errors: typed Rust errors via `thiserror`.
- CRC-32, frame encoding, WAL, recovery, indexes, text postings, locking protocol, compaction, and sharding: migrated in this crate to preserve behavior and formats.

Only tests that provide essential parity evidence are migrated: binary-format vectors, ordered structures/query semantics, WAL durability, recovery/corruption behavior, core database lifecycle and indexes, lock exclusion, RESP framing, and shard routing/cross-shard operations.
