# SurrealDB HNSW Vector Search: Technical Internals

Source: `surrealdb/surrealdb` repository, `main` branch (March 2026).

## Architecture Overview

SurrealDB's HNSW is a fully custom Rust implementation integrated directly into the database's KV-backed storage engine. It is **not** a wrapper around `hnswlib`, `usearch`, or any external ANN library. The implementation lives in:

| File | Purpose |
|------|---------|
| `surrealdb/core/src/idx/trees/hnsw/mod.rs` | Core `Hnsw<L0, L>` struct, `HnswState`, `VectorPendingUpdate` |
| `surrealdb/core/src/idx/trees/hnsw/index.rs` | `HnswIndex` — high-level concurrent API (two-phase writes) |
| `surrealdb/core/src/idx/trees/hnsw/layer.rs` | `HnswLayer<S>` — per-layer graph, search, insert, remove |
| `surrealdb/core/src/idx/trees/hnsw/elements.rs` | `HnswElements` — vector storage + LRU cache |
| `surrealdb/core/src/idx/trees/hnsw/cache.rs` | `VectorCache` — weighted LRU cache (quick_cache) |
| `surrealdb/core/src/idx/trees/hnsw/flavor.rs` | `HnswFlavor` — type-erased enum dispatch over neighbor set sizes |
| `surrealdb/core/src/idx/trees/hnsw/heuristic.rs` | `Heuristic` — 4 neighbor selection strategies |
| `surrealdb/core/src/idx/trees/hnsw/docs.rs` | `VecDocs` / `HnswDocs` — vector-to-document mappings |
| `surrealdb/core/src/idx/trees/hnsw/filter.rs` | `HnswTruthyDocumentFilter` — conditional search filters |
| `surrealdb/core/src/idx/trees/dynamicset.rs` | `DynamicSet` trait, `ArraySet<N>`, `AHashSet` |
| `surrealdb/core/src/idx/trees/vector.rs` | `Vector`, `SharedVector`, `SerializedVector`, `Distance` impls |
| `surrealdb/core/src/idx/trees/store/hnsw.rs` | `HnswIndexes` — registry of shared HNSW indexes |
| `surrealdb/core/src/catalog/schema/index.rs` | `HnswParams`, `Distance`, `VectorType` catalog definitions |
| `surrealdb/core/src/cnf/mod.rs` | `HNSW_CACHE_SIZE` config (default 256 MiB) |

---

## 1. HNSW Parameters

Defined in `HnswParams` (`catalog/schema/index.rs`):

```rust
pub(crate) struct HnswParams {
    pub dimension: u16,
    pub distance: Distance,
    pub vector_type: VectorType,
    pub m: u8,           // max connections per node (upper layers)
    pub m0: u8,          // max connections per node (layer 0)
    pub ml: Number,      // level multiplier
    pub ef_construction: u16,
    pub extend_candidates: bool,
    pub keep_pruned_connections: bool,
    pub use_hashed_vector: bool,
}
```

### Defaults and relationships

There is **no fixed default** for `m` or `ef_construction` at the struct level — these are user-specified at index creation time via `DEFINE INDEX ... HNSW DIMENSION ... DISTANCE ... M ... EFC ...`. However, the test code reveals the convention:

- **`m0` = `m * 2`**: Layer 0 (base layer) gets double the connections of upper layers. This is the standard HNSW convention.
- **`ml` = `1.0 / ln(m)`**: The level multiplier is computed from `m` as `1.0 / (m as f64).ln()`. This controls the probability distribution for random level assignment.
- **`ef_construction`**: User-specified, typically 100–500 in tests.
- **`ef` (search-time)**: User-specified per query, typically 40–500 in tests.

### Configuration via SurrealQL

```sql
DEFINE INDEX idx ON TABLE mt FIELD embedding 
    HNSW DIMENSION 1536 DISTANCE COSINE M 16 EFC 200 
    TYPE F32 EXTEND CANDIDATES KEEP PRUNED CONNECTIONS;
```

### Recall benchmarks from tests

With `M=8, EFC=100, DIMENSION=20, EUCLIDEAN` on 1000 random vectors:
- `ef=10` → recall ≥ 0.98
- `ef=40` → recall ≥ 1.0

---

## 2. Index Structure

### In-memory + On-disk hybrid

The HNSW graph lives **primarily in memory** but is **persisted to the KV store** on every mutation. On startup, the graph is reconstructed from the KV store.

#### Persisted state (`HnswState`)

```rust
struct HnswState {
    enter_point: Option<ElementId>,
    next_element_id: ElementId,
    layer0: LayerState,        // version + chunk count
    layers: Vec<LayerState>,   // one per upper layer
}
```

Stored under a single `Hs` key in the KV store. Contains the entry point, the monotonic element ID counter, and per-layer version numbers.

#### Per-layer graph storage

Each layer is an `UndirectedGraph<S>` backed by a `HashMap<ElementId, S>` where `S: DynamicSet` is the neighbor set. The graph is persisted using **per-node keys** (`Hn` prefix keys), not a monolithic blob:

```rust
// Each node's edge list is stored as an independent KV entry
async fn save_nodes(&self, tx: &Transaction, st: &mut LayerState, nodes: &[ElementId]) {
    for &node_id in nodes {
        if let Some(val) = self.graph.node_to_val(&node_id) {
            let key = self.ikb.new_hn_key(self.level, node_id);
            tx.set(&key, &val, None).await?;
        }
    }
    st.version += 1;
}
```

This is a **migration from a legacy chunk-based format** (`Hl` keys). The `load()` method handles three states:
1. **Fully migrated** (`chunks == 0`): loads from per-node `Hn` keys only.
2. **Legacy only** (`chunks > 0`): loads from chunk-based `Hl` keys.
3. **Mixed**: loads `Hl` baseline, overlays `Hn` per-node updates, then completes migration on writable transactions.

#### Vector storage

Vectors are stored under `He` prefix keys in the KV store as `SerializedVector` (revisioned serde). They are also cached in-memory via `VectorCache`.

### Key types in the KV store

| Key prefix | Content |
|------------|---------|
| `Hs` | `HnswState` — entry point, element ID counter, layer versions |
| `Hn{level}{node_id}` | Per-node neighbor list for a specific layer |
| `He{element_id}` | Serialized vector data |
| `Hp{appending_id}` | Pending vector update (two-phase write queue) |
| `Hl{level}{chunk_id}` | Legacy chunk-based graph storage (deprecated) |

---

## 3. Integration with SurrealKV

The HNSW index is fully integrated with SurrealDB's transactional KV store (SurrealKV/RocksDB/TiKV). All mutations go through the same transactional path as regular document operations.

### Two-phase write architecture

Writes are **decoupled** from graph mutations via a pending update queue:

1. **Phase 1 — Enqueue** (`HnswIndex::index`): Document changes are converted to `VectorPendingUpdate` entries and written to the KV store under `Hp` keys. This is **lock-free** — uses `AtomicU64` for sequencing.

2. **Phase 2 — Apply** (`HnswIndex::index_pendings`): A background task drains pending updates, acquires the write lock, and applies them to the in-memory graph. The graph state is then persisted.

```rust
pub(crate) struct HnswIndex {
    hnsw: RwLock<HnswFlavor>,         // graph protected by RwLock
    vec_docs: VecDocs,                // vector-to-document mappings
    next_appending_id: AtomicU64,     // lock-free pending counter
}
```

### Read path

Searches combine results from **both** the pending queue and the committed graph:

1. Scan pending updates → compute distances against query vector → add to result builder.
2. Acquire **read lock** on graph → perform HNSW KNN search → add to result builder.
3. Merge both result sets, deduplicating via a `RoaringTreemap` of pending doc IDs.

### Vector-to-document mapping

`VecDocs` maintains a bidirectional mapping between vectors and their owning documents (doc IDs). When `use_hashed_vector` is enabled, vectors are identified by their BLAKE3 hash rather than full comparison, enabling deduplication of identical vectors.

---

## 4. Distance Functions

Defined in `surrealdb/core/src/idx/trees/vector.rs` on the `Vector` enum. All distance computations are **monomorphized** per vector type — no dynamic dispatch.

### Supported metrics

| Metric | Implementation | Notes |
|--------|---------------|-------|
| **Euclidean** | `a.l2_dist(b)` via ndarray for float types; manual `Zip` for I16 | Default distance |
| **Cosine** | `1.0 - (a·b / (\|a\| * \|b\|))` | Specialized F64 and F32 paths using `dot()` directly |
| **Manhattan** | `a.l1_dist(b)` via ndarray for float types; manual for I16 | |
| **Chebyshev** | `a.linf_dist(b)` via ndarray for float types; manual for I16 | |
| **Hamming** | Element-wise inequality count via `Zip` | Works on all types |
| **Minkowski(p)** | `sum(\|a_i - b_i\|^p)^(1/p)` via generic `ToFloat` trait | Parameterized order |
| **Pearson** | `cov(x,y) / (σ_x * σ_y)` — correlation coefficient | Returns similarity, not distance |
| **Jaccard** | `intersection / union` using HashSet | Returns similarity |

### Vector types

```rust
pub enum Vector {
    F64(Array1<f64>),  // 8 bytes per element
    F32(Array1<f32>),  // 4 bytes per element (default)
    I64(Array1<i64>),  // 8 bytes per element
    I32(Array1<i32>),  // 4 bytes per element
    I16(Array1<i16>),  // 2 bytes per element
}
```

All five types support all eight distance metrics. The `ToFloat` trait provides a unified conversion path for integer types.

### SharedVector

Vectors are wrapped in `SharedVector(Arc<Vector>, u64)` — reference-counted with a precomputed `AHash` hashcode. This avoids expensive rehashing when vectors are used as cache keys.

---

## 5. Concurrency Model

### Read-write lock on the graph

```rust
hnsw: RwLock<HnswFlavor>
```

- **Reads** (searches): Acquire `read().await` on the `RwLock`. Multiple concurrent searches are allowed.
- **Writes** (inserts/removes): Acquire `write().await`. Exclusive access to the graph during mutation.

### Lock-free pending queue

The enqueue path (`index()`) is fully lock-free:
- `next_appending_id: AtomicU64` provides monotonic ordering.
- `HnswDocs::get_doc_id()` is a static method that reads directly from the KV store without acquiring any lock.
- Pending updates are written as individual KV entries.

### State synchronization

Before any write operation, `check_state()` is called to synchronize the in-memory graph with any changes persisted by other transactions. It compares per-layer version numbers and reloads only changed layers.

### Index registry

`HnswIndexes` is a shared registry (keyed by `(NamespaceId, DatabaseId, TableId, IndexId)`) using `Arc<RwLock<HashMap<...>>>` with a double-checked locking pattern for lazy initialization.

---

## 6. Memory Layout and Optimizations

### Neighbor sets: `DynamicSet` trait with compile-time specialization

The core optimization is the `HnswFlavor` enum, which dispatches to concrete `Hnsw<L0, L>` types parameterized by fixed-size or dynamic neighbor sets:

```rust
enum HnswFlavor {
    H5_9(Hnsw<ArraySet<9>, ArraySet<5>>),    // m=1..4, m0=1..8
    H5_17(Hnsw<ArraySet<17>, ArraySet<5>>),   // m=1..4, m0=9..16
    H9_17(Hnsw<ArraySet<17>, ArraySet<9>>),   // m=5..8, m0=1..16
    // ... more variants for common sizes ...
    Hset(Hnsw<AHashSet, AHashSet>),            // fallback for large m
}
```

Two backing implementations:

1. **`ArraySet<N>`** — stack-allocated fixed-size array of `ElementId` values. No heap allocation. Linear scan for `contains()`/`remove()`. Used for small, known-at-compile-time neighbor counts (up to ~29).

2. **`AHashSet`** — `ahash::HashSet<ElementId>` for larger neighbor counts. Heap-allocated.

This avoids dynamic dispatch (`dyn DynamicSet`) and heap allocation for the common case.

### Vector cache

`VectorCache` is a **weighted LRU cache** using `quick_cache::sync::Cache`:

- **Default size**: 256 MiB (`SURREAL_HNSW_CACHE_SIZE` env var).
- **Weighting**: `VectorWeighter` calculates memory as `vector_data_size + overhead`.
- **Per-index tracking**: `ElementsPerIndex` (a `DashMap` of `RoaringTreemap`) tracks which element IDs are cached per index, enabling efficient bulk eviction when an index is dropped.
- **Thread safety**: Uses `parking_lot::RwLock` (not `tokio::sync::RwLock`) in the eviction callback to avoid panics in async contexts.

### SIMD

There is **no explicit SIMD** in the HNSW code itself. SIMD comes indirectly through:
- **ndarray**: Uses `l2_dist()`, `l1_dist()`, `linf_dist()`, `dot()` which may use SIMD depending on the ndarray build configuration (the `blas` feature is not listed in the HNSW context).
- **`Zip::from(a).and(b)`**: ndarray's zip iterator for element-wise operations.

For I16 vectors, all distance computations use manual loops with the `ToFloat` trait — no SIMD.

### Search algorithm

The search follows the standard HNSW algorithm:

1. **Entry point descent**: Greedily traverse upper layers from the global entry point to find the closest node at layer 0.
2. **Layer 0 search**: Priority-queue-based beam search with `ef` candidates. Uses a `DoublePriorityQueue` (min-heap for candidates, max-heap for results) for efficient nearest/farthest extraction.
3. **Filter support**: `search_single_with_filter` adds a conditional check against `HnswTruthyDocumentFilter` during traversal, only adding matching documents to the result set.

---

## 7. Deletion

Deletion is **eager, not lazy**. There are no tombstones.

### Removal process (`HnswLayer::remove`)

1. **Disconnect**: `graph.remove_node_and_bidirectional_edges(&e_id)` removes the node and all its edges from the in-memory graph. Returns the set of former neighbors (`f_ids`).
2. **Reconnect neighbors**: For each former neighbor, perform a local search (`search_multi_with_ignore`) to find new connections, excluding the deleted node. Apply the neighbor selection heuristic to prune back to `m_max` connections.
3. **Persist**: Delete the removed node's `Hn` key from the KV store. Save all modified neighbor nodes.
4. **Update entry point**: If the deleted node was the entry point, find a replacement via `search_single_with_ignore`.
5. **Remove vector**: Delete from both the `VectorCache` and the KV store (`He` key).

### Vector-to-document cleanup

When a document is deleted:
- Its vectors are removed from the graph via `vec_docs.remove()`.
- The document ID is removed from `HnswDocs`.
- If multiple documents shared the same vector (deduplication via hashing), the vector is only removed from the graph when the last document referencing it is deleted.

---

## Design Decisions Worth Noting

1. **Type-erased enum dispatch over trait objects**: `HnswFlavor` uses an enum with 14 variants instead of `Box<dyn HnswTrait>`. This avoids vtable overhead while supporting compile-time monomorphization of neighbor set sizes.

2. **Per-node KV persistence**: Each node's edge list is stored as an independent KV entry, not a monolithic graph blob. This allows incremental saves and efficient loading.

3. **Two-phase writes with pending queue**: Decoupling document writes from graph mutations allows the enqueue path to be lock-free and non-blocking. The graph is only mutated during batch application.

4. **BLAKE3 vector hashing**: When `use_hashed_vector` is enabled, vectors are identified by their BLAKE3 hash (little-endian byte serialization). This enables deduplication — multiple documents with the same vector share a single graph node.

5. **`SerializedVector` for KV storage**: Vectors are stored as `Vec<T>` (not `ndarray::Array1`) in the KV store for simpler serialization. Conversion to `Array1` happens on load.

6. **No external dependencies for ANN**: The entire HNSW implementation depends only on `ahash`, `ndarray`, `ndarray-stats`, `quick-cache`, `dashmap`, `parking_lot`, `roaring`, and `blake3` — all pure-Rust crates.
