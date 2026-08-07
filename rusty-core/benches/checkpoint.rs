//! Benchmark: checkpoint serialization + save at increasing state sizes.
//!
//! Covers the three layers of the checkpoint write path at state sizes of
//! 1 KB / 100 KB / 1 MB:
//!
//! - `serialize`: `serde_json::to_vec_pretty(&Checkpoint)` — pure CPU cost,
//!   the same serialization `JsonFileCheckpointer` performs internally;
//! - `in_memory_put`: `InMemoryCheckpointer::put` — mutex + clone-free
//!   move into the store;
//! - `json_file_put`: `JsonFileCheckpointer::put` — serialize + atomic
//!   temp-write + rename + latest-pointer write;
//! - `json_file_get_latest`: `JsonFileCheckpointer::get_latest` — pointer
//!   read + file read + deserialize (the resume-path load cost).
//!
//! File benches use a dedicated temp root that is cleaned before and after.

mod common;

use common::{state_sized, temp_checkpoint_root, tokio_runtime};
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use rusty_agent_runtime::prelude::*;

const SIZES: [usize; 3] = [1_024, 102_400, 1_048_576];

fn make_checkpoint(bytes: usize, step: usize) -> Checkpoint {
    Checkpoint::new("bench-thread", step, state_sized(bytes), vec!["next".into()])
}

fn bench_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("checkpoint_serialize");
    for bytes in SIZES {
        group.bench_with_input(
            BenchmarkId::new("state_bytes", bytes),
            &bytes,
            |b, &bytes| {
                let checkpoint = make_checkpoint(bytes, 0);
                b.iter(|| {
                    let out = serde_json::to_vec_pretty(&checkpoint).expect("serializes");
                    criterion::black_box(out)
                });
            },
        );
    }
    group.finish();
}

fn bench_in_memory_put(c: &mut Criterion) {
    let rt = tokio_runtime();
    let mut group = c.benchmark_group("checkpoint_in_memory_put");
    for bytes in SIZES {
        group.bench_with_input(
            BenchmarkId::new("state_bytes", bytes),
            &bytes,
            |b, &bytes| {
                let store = InMemoryCheckpointer::new();
                b.iter_batched(
                    || make_checkpoint(bytes, 0),
                    |checkpoint| {
                        rt.block_on(async {
                            store.put(checkpoint).await.expect("put succeeds");
                        })
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

fn bench_json_file(c: &mut Criterion) {
    let rt = tokio_runtime();
    let root = temp_checkpoint_root("json-file");
    let _ = std::fs::remove_dir_all(&root);

    let mut group = c.benchmark_group("checkpoint_json_file_put");
    for bytes in SIZES {
        group.bench_with_input(
            BenchmarkId::new("state_bytes", bytes),
            &bytes,
            |b, &bytes| {
                let store = JsonFileCheckpointer::new(root.clone());
                b.iter_batched(
                    // Fresh checkpoint per iteration: ids are unique by
                    // construction, so puts never collide on disk.
                    || make_checkpoint(bytes, 0),
                    |checkpoint| {
                        rt.block_on(async {
                            store.put(checkpoint).await.expect("put succeeds");
                        })
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();

    // Load path: one checkpoint pre-saved per size; each iteration reads and
    // deserializes it (the pointer fast path, as on resume).
    let mut group = c.benchmark_group("checkpoint_json_file_get_latest");
    for bytes in SIZES {
        group.bench_with_input(
            BenchmarkId::new("state_bytes", bytes),
            &bytes,
            |b, &bytes| {
                let store = JsonFileCheckpointer::new(root.clone());
                let thread = format!("load-{bytes}");
                rt.block_on(async {
                    store
                        .put(Checkpoint::new(
                            thread.clone(),
                            0,
                            state_sized(bytes),
                            vec!["next".into()],
                        ))
                        .await
                        .expect("seed put succeeds");
                });
                b.iter(|| {
                    rt.block_on(async {
                        let cp = store
                            .get_latest(&thread)
                            .await
                            .expect("get_latest succeeds")
                            .expect("checkpoint exists");
                        criterion::black_box(cp)
                    })
                });
            },
        );
    }
    group.finish();

    let _ = std::fs::remove_dir_all(&root);
}

criterion_group!(
    benches,
    bench_serialize,
    bench_in_memory_put,
    bench_json_file
);
criterion_main!(benches);
