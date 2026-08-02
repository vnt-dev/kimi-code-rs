# kimi-code-minidb

An embedded, async, log-structured key-value and document database for Rust.

`kimi-code-minidb` is a faithful Rust port of the original TypeScript `packages/minidb` package. It preserves the original on-disk formats (WAL, snapshot), recovery behavior, compaction, indexing, locking, and cluster semantics rather than delegating to an external engine such as SQLite or sled. It is a workspace-internal crate (`publish = false`) and currently serves as the storage engine behind `crates/agent-core-v2`'s query store.

## Features

- **Durable log-structured storage** — append-only WAL with CRC-32-checked frames, snapshot-based compaction, and crash recovery with corruption resync (`Resync` / `Strict` modes) plus an `open_or_rebuild` self-healing path.
- **Configurable durability** — `FsyncPolicy::Always`, `EverySecond` (default), or `No`.
- **Pluggable value codecs** — binary (`Vec<u8>`), string, JSON (`serde_json::Value`), or your own via the `ValueCodec` trait.
- **TTL** — per-key expiry in milliseconds with active expiration (configurable sweep interval).
- **Atomic batches** — `batch()` writes multiple operations as a single WAL frame.
- **Rich queries** — ordered and prefix scans, plus a unified `query()` with key ranges, datetime columns, full-text search, Mongo-style filters (`$eq`, `$gt`, `$in`, `$regex`, `$and`, ...), projection, sort, and pagination.
- **Indexing** — equality/range secondary indexes (unique, sparse), compound group-by/order-by indexes, datetime range indexes, and TF-IDF full-text search with optional disk-backed postings.
- **Memory management** — `max_memory_bytes` with `Reject` or `EvictLru` policies, and a disk-backed value mode (`Memory` / `Disk` / `Auto`).
- **Safe concurrency** — cross-process single-writer lock file with stale-lock takeover, read-only mode, and a `readonly_on_lock_fail` fallback.
- **Maintenance** — manual and automatic compaction, backup/restore with manifest, and runtime stats.
- **Server mode** — an optional Redis-protocol (RESP2) TCP server exposing a `MiniDb<String>` (PING, GET, SET, DEL, MGET, TTL, COMPACT, ...), compatible with `redis-cli`.
- **Cluster mode** — `ClusterDb` shards keys by stable hash across N `MiniDb` instances, with a bounded lock-pooled handle pool, incremental WAL catch-up for readers, and a cluster-wide index registry.

## Quick start

The crate is async and requires a Tokio runtime. Add it as a path dependency:

```toml
[dependencies]
kimi-code-minidb = { path = "path/to/crates/minidb" }
tokio = { version = "1", features = ["full"] }
serde_json = "1"
```

```rust
use kimi_code_minidb::{MiniDb, minidb::SetOptions, index_manager::IndexDef};
use serde_json::{Value, json};

#[tokio::main]
async fn main() -> Result<(), kimi_code_minidb::MiniDbError> {
    // Open (or create) a database directory with the JSON codec.
    let db = MiniDb::open(MiniDb::<Value>::json_options("./my-db")).await?;

    db.set("a", json!({"name": "alpha", "score": 2}), SetOptions::default()).await?;

    // Secondary index on a JSON path.
    db.create_index("name", IndexDef::equality("name")).await?;
    assert_eq!(db.find_eq("name", &Value::from("alpha"))?[0].key, "a");

    db.close().await?;

    // Reopening recovers state from snapshot + WAL.
    let db = MiniDb::open(MiniDb::<Value>::json_options("./my-db")).await?;
    assert_eq!(db.get("a")?.unwrap()["score"], 2);
    db.close().await
}
```

## Configuration

`MiniDb::open` takes an `OpenOptions<V>`; the `json_options` / `string_options` / `buffer_options` constructors fill in the codec and defaults. Notable fields:

| Field | Default | Description |
| --- | --- | --- |
| `codec` | — | `ValueCodec` implementation for the value type |
| `fsync_policy` | `EverySecond` | When to fsync the WAL (`Always` / `EverySecond` / `No`) |
| `compact_threshold_bytes` | 64 MiB | WAL size that triggers automatic compaction |
| `auto_compact` | `true` | Enable automatic compaction |
| `active_expire_interval` | 100 ms | TTL sweep interval |
| `recovery_mode` | `Resync` | `Resync` skips corrupt frames; `Strict` fails on them |
| `value_mode` | `Auto` | Keep values `Memory`-resident, `Disk`-backed, or choose automatically |
| `max_memory_bytes` / `max_memory_policy` | unlimited / `Reject` | Memory cap and `Reject` or `EvictLru` behavior |
| `read_only` / `readonly_on_lock_fail` | `false` / `false` | Open without the writer lock, or fall back to read-only when the lock is held |

## API overview

- **Lifecycle** — `open`, `open_or_rebuild`, `close`, `renew_lock`, `recovery_info`
- **Key-value** — `get`, `get_record`, `set`, `del`, `batch`, `has`, `len`, `mget`, `mset`
- **TTL** — `expire`, `ttl`
- **Scanning** — `scan` (range), `prefix`
- **Unified query** — `query(&QueryOptions)` combining key exact/prefix/range, datetime ranges, text search, Mongo-style filter, projection, sort, skip/limit
- **Secondary indexes** — `create_index`, `drop_index`, `list_indexes`, `find_eq`, `find_range`
- **Compound indexes** — `create_compound_index`, `compound_range`
- **Full-text search** — `create_text_index`, `search`, `drop_text_index`, `list_text_indexes`
- **Datetime columns** — `datetime_columns`, `datetime_range`
- **Maintenance** — `compact`, `backup`, `restore`, `stats`, `compaction_stats`
- **Replication hook** — `catch_up_from_wal` for incremental readers (used by cluster mode)

Index and document-query APIs require the JSON codec and return `MiniDbError::JsonCodecRequired` otherwise. Errors are unified under `MiniDbError`, which wraps the subsystem errors (`StoreError`, `WalError`, `RecoveryError`, `LockError`, index errors, ...) plus `io::Error` and `serde_json::Error`.

## Architecture

- **`codec` + `crc32`** — binary frame format: 2-byte magic `MD`, 22-byte header, op types `SET` / `DEL` / `BATCH`, CRC-32 trailer; includes corruption scanners.
- **`wal`** — append-only `db.wal` with queued writes and the configured fsync policy; supports sealing during compaction rotation.
- **`store` + `skiplist`** — in-memory state: a `HashMap` of records, a Redis-style skiplist for ordered iteration, and an expiry min-heap. Values may live in memory or as `ValueRef::Disk` locations read through `value_reader`.
- **`snapshot` + `compaction` + `rename_replace`** — snapshots write live entries to a temp file; compaction rotates WAL and snapshot with pre-copy passes and an atomic swap.
- **`recovery`** — replays snapshot then WAL at open, resyncing or truncating around corrupt frames, and reports a `RecoveryInfo`.
- **`lockfile`** — cross-process single-writer lock (`db.lock`) with PID/timestamp owner records and dead-owner takeover.
- **Indexes** — `index_manager` (equality/range), `compound_index` (group-by + order-by), `dt_index` (datetime columns), and `text_index` + `text_postings` (TF-IDF full-text with optional on-disk postings). All indexes are in-memory, rebuilt from the store at open; definitions are persisted to JSON files in the database directory.
- **`query`** — dotted-path access (`get_path` / `set_path` / `project`) and the Mongo-style filter matcher.
- **`server`** — the optional RESP2 TCP server over a `MiniDb<String>`.
- **`cluster/`** — `ClusterDb` built from `topology` (cluster metadata), `router` (key → shard), `shard` (per-shard open options), `lock_pool` (bounded handle pool with lock renewal), and `coordinator` (cross-shard grouping and mode checks).

## On-disk layout

Each database directory contains:

- `db.wal` — append-only write-ahead log
- `db.snapshot` — compacted snapshot
- `db.lock` — single-writer lock file
- `db.indexes.json`, `db.textindexes.json`, `db.compound-indexes.json` — persisted index definitions
- Optional text postings file and backup manifests (`backup.manifest.json`)

Cluster mode adds `cluster.meta`, the cluster index registry, and `shard-XXXX/` subdirectories, each a full `MiniDb` directory.

## Cluster mode

`ClusterDb<V>` (`cluster::ClusterDb`) shards keys by a stable 32-bit hash across a fixed number of `MiniDb` shards and mirrors most of the `MiniDb` API (`get`/`set`/`del`/`mget`/`mset`/`batch`/`scan`/`prefix`/`query`, plus index operations via a persisted registry). Shard readers catch up incrementally by WAL offset. Cross-shard writes default to `CrossShardMode::BestEffort` (partial writes are possible and reported); `TwoPhaseCommit` is reserved and currently returns an error. The shard count is fixed at creation time.

## Limitations and caveats

- Keys must be non-empty and at most 128 bytes.
- Secondary, compound, and text indexes and document queries require the JSON codec.
- One writer per database directory; other processes must open read-only (or use cluster mode).
- There are no general multi-key transactions — the atomicity unit is a single `batch()` frame.
- The cluster shard count cannot be changed after creation.

## Testing

Tests live inline in each source module. Run them from the workspace root:

```shell
cargo test -p kimi-code-minidb
```

## License

MIT
