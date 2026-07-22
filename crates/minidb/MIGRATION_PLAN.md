# minidb migration plan

Original package: `packages/minidb`

The Rust crate preserves the original log-structured engine and its public responsibilities. It does not substitute SQLite, sled, or another database because that would change WAL/snapshot formats, recovery behavior, compaction, indexes, locking, and cluster semantics.

## Dependency order

1. Leaf utilities: `crc32`, `skiplist`, `query`, `rename-replace`, `wal`, `cluster/utils`.
2. Formats and in-memory state: `codec`, `store`, `index-manager`, `dt-index`, `compound-index`.
3. Persistence readers: `value-reader`, `snapshot`, `recovery`, `lockfile`.
4. Text search: `text-postings`, then `text-index`.
5. Compaction: `wal + snapshot + store + rename-replace`.
6. Embedded database: `MiniDb` composes all preceding modules.
7. Optional RESP server.
8. Cluster: utilities, types, router/topology/shard/coordinator/lock-pool, then `ClusterDb`.

## Rust dependency mapping

- Node asynchronous filesystem, timers, and TCP: Tokio.
- JavaScript objects and JSON codec/metadata: Serde and `serde_json::Value`.
- JavaScript regular expressions in JSON filters: `regex`.
- Domain errors: typed Rust errors via `thiserror`.
- CRC-32, frame encoding, WAL, recovery, indexes, text postings, locking protocol, compaction, and sharding: migrated in this crate to preserve behavior and formats.

Only tests that provide essential parity evidence are migrated: binary-format vectors, ordered structures/query semantics, WAL durability, recovery/corruption behavior, core database lifecycle and indexes, lock exclusion, RESP framing, and shard routing/cross-shard operations.
