# redb Technical Research

Embedded key-value store for Rust. Pure Rust, ACID, MVCC, copy-on-write B-trees. Loosely inspired by LMDB.

- **Crate**: `redb` v3.1.1 (stable file format, 68 releases)
- **License**: Apache-2.0 / MIT
- **Dependencies**: `libc` only (no other runtime deps)
- **Source**: https://github.com/cberner/redb

---

## 1. Storage Format

### File Layout

The file is divided into a **super-header** (512 bytes, padded to page boundary) followed by one or more **regions**.

```
[Super-header: 512 bytes]
  [Database header: 64 bytes]     -- immutable: magic, page size, region geometry
  [Commit slot 0: 128 bytes]      -- double-buffered transaction state
  [Commit slot 1: 128 bytes]      -- double-buffered transaction state
  [Footer padding: 192+ bytes]

[Region 0]
  [Region header]                  -- allocator state for this region
  [Data pages...]                  -- B-tree pages

[Region 1]
  ...
```

### B-Tree Structure

All data structures (except the super-header) are **copy-on-write**. Two page types:

**Branch pages** (type byte = 2):
- Header: type (1B), padding (1B), num_keys (2B), padding (4B)
- Child page checksums: `num_keys + 1` x 16 bytes (XXH3_128)
- Child page numbers: `num_keys + 1` x 8 bytes
- Key end offsets (optional, omitted for fixed-width keys): `num_keys` x 4 bytes
- Key data (alignment-padded)

**Leaf pages** (type byte = 1):
- Header: type (1B), reserved (1B), num_entries (2B)
- Key end offsets (optional): `num_entries` x 4 bytes
- Value end offsets (optional): `num_entries` x 4 bytes
- Key data (alignment-padded)
- Value data (alignment-padded)

### Page Allocation

Pages have a base size (defaults to OS page size, typically 4 KiB). Pages can be **variably sized** in power-of-2 multiples of the base size. Each region uses a **buddy allocator** for page allocation within that region. The allocator state is tracked using `BtreeBitmap` (64-way tree with bit-packed nodes) and `U64GroupedBitmap`.

### Logical B-Trees

The database contains:
- **Table tree** (system): name -> table definition mapping
- **Data trees** (per table): key -> value mapping
- **Freed pages**: stored in system tables (one for data tree, one for system tree) mapping transaction IDs to freed page lists

All multi-byte integers are **little-endian**.

---

## 2. Transactions

### Transaction Model

redb provides **serializable isolation** -- all writes are applied sequentially. There is no snapshot isolation per se; the MVCC mechanism ensures readers see a consistent snapshot.

### Write Transactions

- Only **one write transaction** at a time (`begin_write()` blocks if one is in progress)
- Obtained via `db.begin_write()` -> `WriteTransaction`
- Must be explicitly `commit()` or `abort()` (or dropped, which aborts)
- Can open/create/delete/rename tables
- Supports **savepoints** (ephemeral and persistent) for rollback within a transaction

### Read Transactions

- **Multiple concurrent read transactions** allowed alongside a writer
- Obtained via `db.begin_read()` -> `ReadTransaction`
- See a snapshot of the database at the time they were created
- Can open tables as `ReadOnlyTable` or `ReadOnlyUntypedTable`

### Durability Levels

```rust
pub enum Durability {
    None,       // Not persisted until followed by Immediate commit
    Immediate,  // fsync before commit() returns (default)
}
```

Set per-transaction via `write_txn.set_durability()`.

### Commit Strategies

**1-Phase + Checksum (1PC+C)** -- default:
1. Write all data + checksums + monotonically incrementing transaction ID
2. Flip the "god byte" primary bit
3. Call `fsync`
4. On crash: verify primary has higher transaction ID and valid checksums; fall back to secondary if not

**2-Phase Commit (2PC)** -- opt-in via `set_two_phase_commit(true)`:
1. Write all data to inactive commit slot
2. `fsync`
3. Flip god byte
4. `fsync`
5. Mitigates theoretical XXH3 collision attacks from malicious input

**Quick Repair** -- opt-in via `set_quick_repair(true)`:
- Saves allocator state with each commit
- Enables 2PC automatically
- Crash recovery is near-instant (no full tree walk needed)
- Trade-off: slower commits, faster recovery

---

## 3. Concurrency

### MVCC Implementation

MVCC is built on the copy-on-write B-tree structure:

1. **Read transactions** take a private copy of the B-tree root pointer and register with the database
2. Pages referenced by any live read transaction are protected from being freed
3. When a write transaction frees a page, it goes into a **pending free queue**
4. Pages are only actually reclaimed after all transactions that could reference them have completed (**epoch-based reclamation**)

### Concurrency Guarantees

- Multiple readers can coexist with each other and with a single writer
- Readers never block writers, writers never block readers
- Only one write transaction at a time (serialized)
- `Database` is `Send + Sync`, both transaction types are `Send + Sync`

### Key Invariants

- Committed pages are never modified in-place
- Freed pages transition to "pending free" state and are only reclaimed after all prior transactions complete
- Pages only reference pages from the same or earlier transactions

---

## 4. Memory Management

### Not mmap-based

redb does **not** use memory-mapped files. It uses a `StorageBackend` trait with explicit `read()` and `write()` calls. The default backend is `FileBackend` which uses standard file I/O (`pread`/`pwrite`).

### In-Memory Cache

Configurable via `Builder::set_cache_size()`:
- Default: **1 GiB**
- Caches recently accessed pages
- `db.cache_stats()` returns `CacheStats` with usage information
- No direct control over eviction policy from the API

### StorageBackend Trait

```rust
pub trait StorageBackend: 'static + Debug + Send + Sync {
    fn len(&self) -> Result<u64, Error>;
    fn read(&self, offset: u64, out: &mut [u8]) -> Result<(), Error>;
    fn set_len(&self, len: u64) -> Result<(), Error>;
    fn sync_data(&self) -> Result<(), Error>;
    fn write(&self, offset: u64, data: &[u8]) -> Result<(), Error>;
    fn close(&self) -> Result<(), Error> { ... }  // provided
}
```

Built-in implementations:
- `FileBackend` -- standard file I/O (default for on-disk databases)
- `InMemoryBackend` -- in-memory only (for testing)

This is pluggable -- custom backends can be provided via `Builder::create_with_backend()`.

### Zero-Copy Access

`AccessGuard<'a, V>` provides zero-copy access to values stored on disk. The `.value()` method returns `V::SelfType<'a>` which can be a borrowed reference (e.g., `&str`, `&[u8]`) for types that support it. The guard holds a reference to the underlying page cache entry.

---

## 5. Crash Recovery

### Automatic Recovery

redb is **crash-safe by default**. On open, it:
1. Reads the god byte to determine the primary commit slot
2. Verifies the primary slot's checksum and transaction ID
3. If invalid, falls back to the secondary slot
4. Rebuilds allocator state by walking all referenced B-trees (data, system, freed)

### Assumptions About Media

1. Single-byte writes are atomic
2. `fsync` guarantees durability
3. Powersafe overwrite (writes don't corrupt adjacent bytes)

### Repair

`db.check_integrity()` forces a full integrity check and repair. Returns:
- `Ok(true)` -- passed checks
- `Ok(false)` -- failed but was repaired
- `Err(Corrupted)` -- unrepairable

### Quick Repair Path

When `set_quick_repair(true)` is enabled:
- Allocator state is persisted in a system table during each commit
- 2PC guarantees the primary commit slot is valid
- Recovery is near-instant: just load the saved allocator state

---

## 6. Key/Value Types

### Value Trait

```rust
pub trait Value: Debug {
    type SelfType<'a>: Debug + 'a where Self: 'a;
    type AsBytes<'a>: AsRef<[u8]> + 'a where Self: 'a;

    fn fixed_width() -> Option<usize>;
    fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a> where Self: 'a;
    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a> where Self: 'b;
    fn type_name() -> TypeName;
}
```

Key design: `SelfType<'a>` enables zero-copy deserialization. For `&str` and `&[u8]`, `from_bytes` returns a borrowed slice. For integers, it returns the value by copy.

### Key Trait

```rust
pub trait Key: Value {
    fn compare(data1: &[u8], data2: &[u8]) -> Ordering;
}
```

Extends `Value` with a comparison function operating on serialized bytes.

### Built-in Implementations

**Keys** (all implement both `Key` and `Value`):
- All integer types: `u8`-`u128`, `i8`-`i128`
- `bool`, `char`, `()`
- `&str`, `String`
- `&[u8]`, `&[u8; N]`
- `Option<T>` where `T: Key`
- Tuples up to 12 elements

**Values only** (additionally):
- `f32`, `f64`
- `Vec<T>` where `T: Value`
- `[T; N]` where `T: Value`

### Type Safety

Tables are typed at definition time:
```rust
const TABLE: TableDefinition<&str, Vec<u8>> = TableDefinition::new("my_data");
```

The table name and type are stored in the system table tree. Opening a table with a different type than what was stored will return an error (type mismatch detection via `TypeName`).

### Fixed vs Variable Width

`fixed_width()` returns `Some(N)` for fixed-size types (integers, etc.) and `None` for variable-size types (strings, byte slices, Vec). This affects on-disk layout: fixed-width types omit the per-entry offset arrays in leaf pages, saving space and improving locality.

---

## 7. Limitations

### Single Writer

Only one write transaction can be in progress at a time. `begin_write()` blocks until the current write completes. There is no optimistic concurrency or lock-free write path.

### No Secondary Indexes

Tables are simple key-value maps. There is no built-in secondary index support. Range scans are available via the B-tree ordering.

### No Native TTL or Expiration

No built-in time-to-live or automatic expiration of entries.

### No Cross-Table Transactions

All tables within a single write transaction are committed atomically, but there is no concept of cross-database transactions.

### File Format Stability

The file format is currently stable (v3), but there have been breaking changes between major versions (v1 -> v2 added length fields, v2 -> v3 restructured freed pages). Tuple serialization changed between v2 and v3 (requires `Legacy<T>` wrapper for v2-format tuples).

### Space Amplification

COW B-trees produce write amplification. Compaction (`db.compact()`) is available but requires exclusive access and can be slow. The benchmark data shows 4 GiB uncompacted vs 1.69 GiB compacted for redb (vs 893 MiB for RocksDB which uses LSM trees).

### Removal Performance

Deletions are slow compared to RocksDB/fjall (23s vs 6-10s in benchmarks) because COW requires rewriting B-tree pages.

### No Encryption

No built-in encryption at rest. Would need to be implemented at the `StorageBackend` level.

### No Replication

Single-file, single-process database. No built-in replication, sharding, or distribution.

### Non-Durable Commits Cannot Free Pages

When using `Durability::None`, freeing pages is not permitted because the transaction could be rolled back at any time.

---

## 8. API Surface

### Core Types

| Type | Description |
|------|-------------|
| `Database` | Opened database file, `Send + Sync` |
| `ReadOnlyDatabase` | Read-only opened database |
| `WriteTransaction` | Read/write transaction (single at a time) |
| `ReadTransaction` | Read-only transaction (multiple concurrent) |
| `Table<'txn, K, V>` | Writable table handle |
| `ReadOnlyTable<K, V>` | Read-only table handle |
| `ReadOnlyUntypedTable` | Read-only table without type parameters |
| `MultimapTable<'txn, K, V>` | One-to-many table |
| `TableDefinition<'a, K, V>` | Typed table definition (const) |
| `MultimapTableDefinition<'a, K, V>` | Typed multimap table definition |
| `AccessGuard<'a, V>` | Zero-copy accessor to stored data |
| `Range<'a, K, V>` | Double-ended iterator over key-value pairs |
| `Savepoint` | Transaction savepoint for rollback |
| `Builder` | Database configuration builder |
| `Durability` | `None` or `Immediate` |
| `TypeName` | Globally unique type identifier |

### Key Traits

| Trait | Purpose |
|-------|---------|
| `Value` | Serialization/deserialization to/from bytes |
| `Key` | `Value` + ordered comparison on bytes |
| `ReadableTable<K, V>` | `get()`, `range()`, `first()`, `last()`, `iter()` |
| `ReadableTableMetadata` | `len()` -- O(1) since v2 |
| `TableHandle` | Table name abstraction |
| `StorageBackend` | Pluggable storage (file, memory, custom) |

### Typical Usage Pattern

```rust
let db = Database::create("path.redb")?;

// Write
let write_txn = db.begin_write()?;
{
    let mut table = write_txn.open_table(TABLE)?;
    table.insert("key", &value)?;
}
write_txn.commit()?;

// Read
let read_txn = db.begin_read()?;
let table = read_txn.open_table(TABLE)?;
if let Some(guard) = table.get("key")? {
    let val: &str = guard.value();  // zero-copy
}
```

### Builder Configuration

```rust
let db = Database::builder()
    .set_cache_size(512 * 1024 * 1024)  // 512 MiB cache
    .create("path.redb")?;
```

---

## Implications for Mind

### What redb Provides

- **Durable, crash-safe storage** with ACID guarantees -- suitable for the Trace Store (append-only epistemic history)
- **Typed tables** with compile-time type safety -- can define separate tables for different subsystem stores
- **MVCC reads** -- the Context Buffer (working memory) can read without blocking the Consolidator (background writes)
- **Savepoints** -- could be used for atomic multi-step operations during consolidation
- **Range scans** -- useful for time-range queries on the trace store
- **Zero-copy access** via `AccessGuard` -- important for performance when reading large values (episodic entries)
- **Pluggable storage** -- `StorageBackend` trait allows in-memory backend for testing or custom backends for special needs
- **Single binary, no external services** -- aligns with the Mind project constraint

### What Mind Must Build On Top

- **Schema Store abstraction** -- redb is a raw KV store; the fast-KV fact layer needs a schema/key design on top
- **Similarity Index** -- redb has no vector search; `usearch` handles this separately
- **Graph structure** -- `petgraph` is separate; relationship traversal between epistemic entries needs custom indexing
- **Consolidator background process** -- redb's single-writer constraint means the consolidator must coordinate write access with the orchestrator
- **Compaction strategy** -- periodic `compact()` calls needed to manage space amplification from COW
- **Key design** -- needs careful design for the trace store (append-only, time-ordered) vs schema store (point lookups)
- **Multi-value / multimap** -- available via `MultimapTable` but may need custom encoding for the trace store's append-only pattern

### Key Design Decisions for Mind

1. **Quick repair** should likely be enabled (`set_quick_repair(true)`) to avoid slow recovery walks on large trace stores
2. **Cache size** should be tuned based on expected working set size for the Context Buffer
3. **Durability::Immediate** is appropriate for the trace store (epistemic history must be durable)
4. **Durability::None** could be used for transient working state during consolidation, followed by an Immediate commit
5. **Table layout** needs to be designed: one table per subsystem store, or a unified table with key prefixes?
