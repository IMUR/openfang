# SurrealKV Technical Deep Dive

> Research report for pattern extraction. Source: [github.com/surrealdb/surrealkv](https://github.com/surrealdb/surrealkv), v0.21.0, Apache-2.0 license.

## Overview

SurrealKV is a versioned, embedded, ACID-compliant key-value store written in pure Rust. It replaced an earlier VART-based (Versioned Adaptive Radix Trie) design because the entire index had to fit in memory, limiting scalability. The current design is an LSM tree, enabling datasets larger than RAM.

**Key source files** (within `surrealkv/src/`):

| Path | Purpose |
|------|---------|
| `lsm.rs` | `CoreInner`, `Core`, `Tree` — the main entry point and orchestration |
| `transaction.rs` | `Transaction`, `Entry`, `TransactionRangeIterator`, `TransactionHistoryIterator` |
| `commit.rs` | `CommitPipeline` — lock-free commit queue (inspired by CockroachDB Pebble) |
| `memtable/` | Lock-free skip list (`crossbeam-skiplist`) |
| `sstable/table.rs` | SSTable writer/reader, block format, footer, bloom filter integration |
| `sstable/block.rs` | Data block format with restarts and binary search |
| `sstable/index_block.rs` | Partitioned index structure |
| `sstable/filter_block.rs` | Bloom filter per SSTable |
| `wal/mod.rs` | WAL segment format, record types, buffered writer |
| `wal/manager.rs` | WAL rotation, multi-segment management |
| `wal/recovery.rs` | WAL replay on startup, corruption repair |
| `vlog.rs` | Value Log (WiscKey pattern) — large value separation |
| `levels/` | `LevelManifest`, level metadata, SSTable tracking |
| `compaction/` | Leveled compaction, score-based strategy |
| `bplustree/` | Optional disk-based B+tree for versioned index |
| `snapshot.rs` | Snapshot tracker, point-in-time views |
| `stall.rs` | Write stall controller for backpressure |
| `cache.rs` | LRU block cache for SSTable data blocks |

**Key dependencies**: `crossbeam-skiplist`, `snap` (Snappy), `lz4_flex`, `crc32fast`, `quick_cache`, `parking_lot`, `tokio`.

---

## 1. On-Disk Format

### Directory Layout

```
database_path/
├── wal/                    # Write-ahead log segments
│   ├── 00000000000000000001.wal
│   └── 00000000000000000002.wal
├── sstables/               # SSTable files
│   ├── 00000000000000000001.sst
│   └── 00000000000000000002.sst
├── manifest/               # Level manifest files
│   └── 00000000000000000001.manifest
├── vlog/                   # Value log files (if enabled)
│   ├── 00000000000000000001.vlog
│   └── 00000000000000000002.vlog
├── versioned_index/        # B+tree index (if enabled)
└── LOCK                    # Lock file (fs2 on non-Windows)
```

### WAL Format (`src/wal/mod.rs`)

WAL files are divided into **32KB blocks**. Records span blocks via fragmentation:

```
File Layout:
  +-----+-------------+--+----+----------+------+-- ... ----+
  | r0  |     r1      |P | r2 |    r3    |  r4  |           |
  +-----+-------------+--+----+----------+------+-- ... ----+
  <--- 32KB -------><--- 32KB ------------>

Record header (7 bytes):
  +---------+-----------+-----------+--- ... ---+
  |CRC (4B) | Size (2B) | Type (1B) | Payload   |
  +---------+-----------+-----------+--- ... ---+

RecordType: Full(1), First(2), Middle(3), Last(4), Empty(0), SetCompressionType(9)
```

- **CRC**: CRC32 over type + payload
- **Size**: payload length (big-endian)
- Records too large for remaining block space are fragmented as First/Middle/Last
- Default max segment size: 100MB
- LZ4 compression supported per-segment

### SSTable Format (`src/sstable/table.rs`)

```
┌─────────────────────────────────────────────────────────────────┐
│                        SSTable File Layout                       │
├─────────────────────────────────────────────────────────────────┤
│  [Data Block 1]           ← Sorted key-value pairs                   │
│  [Data Block 2]                                                  │
│  ...                                                             │
│  [Data Block N]                                                  │
├─────────────────────────────────────────────────────────────────┤
│  [Index Block - Partition 1]  ← Points to data blocks           │
│  [Index Block - Partition 2]                                     │
│  ...                                                             │
│  [Index Block - Partition M]                                     │
├─────────────────────────────────────────────────────────────────┤
│  [Top-Level Index Block]  ← Points to partition index blocks    │
├─────────────────────────────────────────────────────────────────┤
│  [Meta Index Block]       ← Filter block handle, metadata      │
├─────────────────────────────────────────────────────────────────┤
│  [Footer]                 ← 50 bytes                           │
└─────────────────────────────────────────────────────────────────┘
```

**Data block on disk**:
```
+------------------+-------------------+-----------------+
| Block Data       | Compression Type  | Masked CRC32    |
| (variable)       | (1 byte)          | (4 bytes)       |
+------------------+-------------------+-----------------+
```

**Footer (50 bytes)**:
```
Offset  Size  Field
0       1     Format version (1 = LSMV1)
1       1     Checksum type (1 = CRC32c)
2       16    Meta index block handle (varint encoded)
18      16    Index block handle (varint encoded)
34      8     Padding (zeros)
42      8     Magic number (0x57fb808b247547db)
```

**Index entries use separator keys** (not actual last keys). The separator is the shortest key `S` where `last_key <= S < next_key`. This creates "gaps" — a seek target may fall in the gap, requiring `advance_to_valid_entry()` to skip to the next block.

**Compression**: Per-level configurable. Supports `None` and `Snappy`. Default: no compression. L0 typically uncompressed for speed; L1+ can use Snappy.

**Bloom filter**: Per-SSTable, ~1% false positive rate. Stored in meta index block. Loaded into memory on SSTable open.

**Block cache**: LRU cache keyed on `(file_id, block_offset)`. Default block size: 4KB.

### InternalKey Encoding

Every key in the system is an `InternalKey`:

```
┌──────────────────┬───────────────────┬──────────────────┐
│    user_key      │    trailer (8B)   │  timestamp (8B)  │
│   (variable)     │  seq_num | kind   │   nanoseconds    │
└──────────────────┴───────────────────┴──────────────────┘

trailer = (seq_num << 8) | kind
```

- **seq_num**: 56-bit monotonic sequence number (upper 56 bits of 64-bit trailer)
- **kind**: 8-bit operation type: Delete(0), SoftDelete(1), Set(2), Replace(6)
- **timestamp**: Optional 8-byte nanosecond timestamp (for versioning)
- Sorting: user key ASC, then seq_num DESC (newest version first)
- `seq_num=0` is the **largest** internal key for a given user_key (used for bound computation)

### Value Format

Values are encoded as `ValueLocation`:

```
┌──────────┬──────────────────┐
│ 1 byte   │    value data     │
├──────────┼──────────────────┤
│ metadata│  inline or ptr    │
└──────────┴──────────────────┘
```

- **BIT_VALUE_POINTER** flag in metadata byte
- Inline: value stored directly
- Pointer: 25-byte `ValuePointer` with `(file_id, offset, size)` into VLog

### VLog Format (`src/vlog.rs`)

When enabled, large values (>1KB threshold, configurable) are stored separately:

```
VLog file:
┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐
│ Entry 1 │ │ Entry 2 │ │ Entry 3 │ │ Entry 4 │ ...
└─────────┘ └─────────┘ └─────────┘ └─────────┘

Default max file size: 256MB
Checksum: CRC32 per entry (configurable: Disabled or Full)
```

When versioning is enabled, `vlog_value_threshold` is set to 0, so **all** values go to VLog and indexes store only 25-byte ValuePointers.

---

## 2. Tree Structure

**LSM tree with leveled compaction**, inspired by RocksDB and Pebble.

### Level Structure

```
LEVEL 0 (Overlapping - recently flushed memtables)
┌─────────┐ ┌─────────┐ ┌─────────┐
│ SST A   │ │ SST B   │ │ SST C   │  ← May have overlapping key ranges
│ a───z   │ │ d───m   │ │ b───k   │
└─────────┘ └─────────┘ └─────────┘
     │           │           │
     └───────────┼───────────┘
                 ▼
LEVEL 1 (Non-overlapping, sorted)
┌─────────┬─────────┬─────────┐
│ SST 1   │ SST 2   │ SST 3   │  ← Non-overlapping, binary search
│ a───f   │ g───m   │ n───z   │
└─────────┴─────────┴─────────┘
                 │
                 ▼
LEVEL 2 (Non-overlapping, 10x larger)
...
```

**Target sizes** (approximate, 10x ratio between levels):

| Level | Target Size | Overlap? | Fan-out |
|-------|-------------|----------|---------|
| L0 | ~64MB (4×16MB SSTs) | Yes | All checked on read |
| L1 | ~256MB | No | Binary search |
| L2 | ~2.5GB | No | Binary search |
| L3+ | ~25GB+ | No | Binary search |

**Configurable**: `with_level_count(7)` (default), `with_block_size(4096)`, `with_max_memtable_size(100MB)`.

### MemTable

- **Data structure**: Lock-free skip list (`crossbeam-skiplist`)
- **Rotation**: When active memtable exceeds `max_memtable_size`, it becomes immutable and a new empty one is created
- **Flush queue**: Immutable memtables are flushed to L0 SSTables by background task
- **Conflict detection**: A `flushed_history` slot retains the most recently flushed memtable for conflict detection against long-running transactions

### Index (Partitioned)

- **Two-level**: Top-level index → partition index blocks → data blocks
- **Separator keys** as upper bounds (not actual last keys)
- **Bloom filter** per SSTable for early rejection
- **Block restarts** for efficient binary search within data blocks

---

## 3. Versioning and MVCC

### MVCC Implementation

**Snapshot isolation** with optimistic concurrency control.

Each transaction captures `visible_seq_num` at start time. Reads see only entries with `seq_num <= snapshot`.

```
Timeline:
     │           │           │           │
  seq=1       seq=2       seq=3       seq=4
  Set(A,1)    Set(A,2)    Set(A,3)    Delete(A)

Snapshot at seq=2 sees: A=2
Snapshot at seq=3 sees: A=3
Snapshot at seq=4 sees: A deleted
```

### Version Retention

- **Default (no versioning)**: Compaction drops old versions as soon as no active snapshot references them. Tombstones dropped at bottom level.
- **With versioning** (`with_versioning(true, retention_ns)`): All versions preserved during compaction. `retention_ns=0` = unlimited retention.
- **Replace kind (6)**: Supersedes all previous versions of a key, enabling aggressive GC.

### Time-Travel Queries

- `get_at(key, timestamp)`: Point-in-time read at specific nanosecond timestamp
- `history(start, end)`: Iterator over all versions in a key range
- **LSM-only versioning**: K-way merge across all LSM components (memtables + all SST levels)
- **B+tree versioned index** (`with_versioned_index(true)`): Disk-based B+tree storing all `(InternalKey → ValuePointer)` entries. Single lookup instead of k-way merge. Supports out-of-order timestamp inserts.

### B+tree Index (`src/bplustree/`)

- Optional, disk-based page management
- Stores `(InternalKey → ValuePointer)` entries sorted by `(user_key, timestamp)`
- Inspired by SQLite's overflow page design for large entries
- Synchronous updates during LSM writes (read-after-write consistency)
- Trade-off: Fast history queries but slower insert performance

---

## 4. Transactions

### Implementation (`src/transaction.rs`)

**Three modes**: `ReadWrite`, `ReadOnly`, `WriteOnly`.

### Transaction Flow

1. **Buffering**: Writes go to `BTreeMap<Key, Vec<Entry>>` (write_set). No locks held, no disk I/O. RYOW supported via write_set check on reads.
2. **Commit**:
   - Validate write conflicts against memtables (`check_keys_conflict`)
   - Assign monotonically increasing sequence numbers via Oracle
   - Enter CommitPipeline (3 phases)
3. **Rollback**: Clear write_set and snapshot. Savepoints supported via `set_savepoint()`/`rollback_to_savepoint()`.

### Commit Pipeline (`src/commit.rs`)

Inspired by CockroachDB Pebble. Three-phase design:

```
Phase 1 (Serialized):  Acquire write mutex → assign seq_num → write to WAL → enqueue
Phase 2 (Concurrent): Apply batch to active memtable (lock-free skip list)
Phase 3 (Multi-consumer): Publish visible_seq_num → complete waiters
```

- **Lock-free queue**: Single-producer, multi-consumer `CommitQueue` using atomic operations
- **Semaphore**: Limits concurrent commits to 8 (prevents memory exhaustion)
- **Write stall**: Backpressure when too many immutable memtables or L0 files accumulate

### Conflict Detection (Optimistic CC)

At commit time, the Oracle checks all keys in the write set against:
1. Active memtable (most recent writes)
2. Immutable memtables (newest first)
3. Flushed history (most recently flushed)

If any key was modified after `start_seq_num`, returns `TransactionWriteConflict`. If memtable history is insufficient (flushed too long ago), returns `TransactionRetry`.

### Durability

- **Eventual** (default): Written to OS buffer cache, no fsync. Fast but may lose data on crash.
- **Immediate**: fsync before commit returns. Slower but guarantees durability.

---

## 5. Compaction

### Leveled Compaction Strategy (`src/compaction/`)

Score-based compaction (similar to RocksDB). Each level has a target size with ~10x ratio.

**Compaction triggers**:
- L0 file count exceeds threshold (default: 4)
- Score-based: computed from L0 size and level ratios

**Compaction process**:
1. Select input SSTables (overlapping range in source level)
2. Merge with overlapping SSTables in target level
3. Drop obsolete versions (seq_num < min active snapshot)
4. Drop tombstones at bottom level
5. Update VLog discard statistics
6. Atomically update manifest

**Snapshot-aware**: Compaction never drops versions visible to active snapshots. The `SnapshotTracker` stores active snapshot sequence numbers.

**Write stalls**: When L0 accumulates too many files or too many immutable memtables, writes are stalled until compaction catches up. Configurable thresholds.

---

## 6. Crash Recovery

### WAL-Based Recovery (`src/wal/recovery.rs`)

On startup:

1. Load manifest to get `log_number` (which WALs are already flushed)
2. Replay all WAL segments with ID >= `log_number`
3. Each WAL segment creates one memtable
4. Flush all but the last memtable to SST
5. Last memtable becomes the active memtable
6. Set `visible_seq_num = max(manifest_last_seq, wal_max_seq)`

### Recovery Modes

- **AbsoluteConsistency**: Fail immediately on any WAL corruption
- **TolerateCorruptedWithRepair**: Attempt to repair corrupted segments, then retry

### Atomicity Guarantees

- **Manifest updates are atomic**: Changeset (new SSTs + log_number) written atomically. If manifest write fails, SST changeset is reverted.
- **Orphaned file cleanup**: On startup, SST files not referenced in manifest are deleted (safe because WAL still has the data)
- **VLog orphan cleanup**: VLog files not referenced by any SST are cleaned up

### Lock File

Uses `fs2` (non-Windows) to prevent multiple processes from opening the same database.

---

## 7. Memory Management

### No mmap

SurrealKV does **not** use mmap. All file I/O goes through:
- `Arc<dyn File>` abstraction (`src/vfs.rs`)
- Standard Rust `File` with `BufWriter` for writes
- `read_at()` for positioned reads

### Buffer Pool

- **Block cache**: LRU cache (`quick_cache` crate) for SSTable data blocks. Keyed on `(file_id, block_offset)`. Separate caches for normal and history blocks.
- **MemTable arena**: Fixed-size arena (`max_memtable_size`, default 100MB). When full, memtable is rotated.
- **No global allocator overrides**: Standard Rust allocator (though SurrealDB optionally uses jemalloc/mimalloc at the server level)

### Key Memory Structures

| Structure | Location | Size |
|-----------|----------|------|
| Active MemTable | Heap (arc-swap) | Up to `max_memtable_size` (100MB default) |
| Immutable MemTables | Heap (Vec) | Multiple, pending flush |
| Bloom Filters | Heap (per-SSTable) | ~10 bits per key |
| Block Cache | Heap (LRU) | Configurable |
| B+tree pages | Heap (page cache) | If versioned index enabled |

### Background Tasks

- **MemTable flush**: Async task, triggered when memtable rotates
- **Level compaction**: Async task, triggered by L0 file count or score
- **VLog GC**: Async task, triggered by discard ratio threshold
- **Task manager** (`src/task.rs`): Coordinates background operations, uses tokio runtime

---

## Summary of Extractable Patterns

1. **Lock-free commit pipeline**: Serialized WAL writes + concurrent memtable applies + atomic visibility publish (Pebble pattern)
2. **Score-based leveled compaction**: RocksDB-style with snapshot awareness
3. **WiscKey value separation**: Large values in separate VLog, 25-byte pointers in index
4. **Dual-index versioning**: LSM-only (k-way merge) or optional B+tree for timestamp queries
5. **Optimistic CC with memtable conflict detection**: Check active, immutable, and flushed history
6. **Write stall backpressure**: Stall writes when compaction falls behind
7. **Separator-key index**: Upper-bound keys in index (not exact last keys) with gap-aware seeking
8. **Atomic manifest updates**: Changeset + rollback for crash-safe metadata changes
9. **CRC32 with masking**: Block checksums use rotated + delta-masked CRC32
10. **Lock file for single-process access**: `fs2`-based exclusive lock
