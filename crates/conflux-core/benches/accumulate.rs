//! The evidence: does vectorizing the combine step actually help,
//! and where does it stop helping?
//!
//! Two comparisons, at model dimensions spanning what this project's own
//! examples actually train — `python/conflux_client/examples/`'s logistic
//! regression is a few thousand parameters, its MNIST CNN a few hundred
//! thousand, and 1M is a small-CNN upper bound. The 8-element case is
//! there to find the floor: SIMD has per-call setup cost, and below some
//! size a scalar loop should win. Reporting that honestly is the point —
//! a benchmark that only measures the size where the answer is flattering
//! isn't evidence.
//!
//! Run with: `cargo bench -p conflux-core`

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;

/// The loop every family member's combine step used before the shared
/// helper existed, kept verbatim so the comparison is against what was
/// replaced rather than a re-imagining of it.
fn scalar_accumulate_weighted(acc: &mut [f32], src: &[f32], weight: f32) {
    for (a, s) in acc.iter_mut().zip(src) {
        *a += s * weight;
    }
}

const LANES: usize = 8;

/// A copy of `weights.rs`'s helper. Duplicated because that module is
/// `pub(crate)` — benches link against the crate's public API only. Kept
/// byte-identical to the real one; `weights.rs`'s own differential test
/// is what guarantees the shipped version is correct, this copy only has
/// to be representative of its cost.
fn simd_accumulate_weighted(acc: &mut [f32], src: &[f32], weight: f32) {
    let n = acc.len().min(src.len());
    let chunks = n / LANES;
    let splat = wide::f32x8::splat(weight);

    for chunk in 0..chunks {
        let base = chunk * LANES;
        let mut a_arr = [0.0f32; LANES];
        let mut s_arr = [0.0f32; LANES];
        a_arr.copy_from_slice(&acc[base..base + LANES]);
        s_arr.copy_from_slice(&src[base..base + LANES]);
        let out = wide::f32x8::new(a_arr) + wide::f32x8::new(s_arr) * splat;
        acc[base..base + LANES].copy_from_slice(out.as_array());
    }

    for i in chunks * LANES..n {
        acc[i] += src[i] * weight;
    }
}

/// A second SIMD shape: `chunks_exact` instead of index arithmetic plus
/// `copy_from_slice` into stack arrays. Same math, fewer moves — used to
/// tell "SIMD doesn't help here" apart from "that particular SIMD
/// implementation was doing redundant copying."
// Deliberately `chunks_exact`, not `as_chunks`: the iterator shape (with
// `by_ref`/`into_remainder`) is exactly what this variant exists to measure,
// and this is a bench, not shipped code.
#[allow(clippy::chunks_exact_to_as_chunks)]
fn simd_chunked_accumulate_weighted(acc: &mut [f32], src: &[f32], weight: f32) {
    let splat = wide::f32x8::splat(weight);
    let mut a_chunks = acc.chunks_exact_mut(LANES);
    let mut s_chunks = src.chunks_exact(LANES);

    for (a, s) in a_chunks.by_ref().zip(s_chunks.by_ref()) {
        let av = wide::f32x8::new(a.try_into().unwrap());
        let sv = wide::f32x8::new(s.try_into().unwrap());
        a.copy_from_slice((av + sv * splat).as_array());
    }

    for (a, s) in a_chunks
        .into_remainder()
        .iter_mut()
        .zip(s_chunks.remainder())
    {
        *a += s * weight;
    }
}

fn sample(len: usize, offset: f32) -> Vec<f32> {
    (0..len)
        .map(|i| (i as f32 * 0.37 + offset) * 1e-3)
        .collect()
}

fn bench_accumulate(c: &mut Criterion) {
    // 8: one SIMD chunk, no tail — the floor case.
    // 10_000: a logistic-regression-scale model.
    // 1_000_000: a small CNN.
    for dim in [8usize, 10_000, 1_000_000] {
        let mut group = c.benchmark_group(format!("accumulate_weighted/dim={dim}"));
        let src = sample(dim, 1.0);
        let start = sample(dim, -2.0);

        group.bench_with_input(BenchmarkId::new("scalar", dim), &dim, |b, _| {
            b.iter_batched_ref(
                || start.clone(),
                |acc| scalar_accumulate_weighted(black_box(acc), black_box(&src), 0.25),
                criterion::BatchSize::SmallInput,
            );
        });

        group.bench_with_input(BenchmarkId::new("simd", dim), &dim, |b, _| {
            b.iter_batched_ref(
                || start.clone(),
                |acc| simd_accumulate_weighted(black_box(acc), black_box(&src), 0.25),
                criterion::BatchSize::SmallInput,
            );
        });

        group.bench_with_input(BenchmarkId::new("simd_chunked", dim), &dim, |b, _| {
            b.iter_batched_ref(
                || start.clone(),
                |acc| simd_chunked_accumulate_weighted(black_box(acc), black_box(&src), 0.25),
                criterion::BatchSize::SmallInput,
            );
        });

        group.finish();
    }
}

/// The realistic shape: one round's whole combine step, ten clients'
/// updates accumulated into one vector. This is what a round actually
/// pays, as opposed to the cost of a single call.
fn bench_full_combine(c: &mut Criterion) {
    const CLIENTS: usize = 10;
    for dim in [10_000usize, 1_000_000] {
        let mut group = c.benchmark_group(format!("combine_{CLIENTS}_clients/dim={dim}"));
        let updates: Vec<Vec<f32>> = (0..CLIENTS).map(|i| sample(dim, i as f32)).collect();

        group.bench_function("scalar", |b| {
            b.iter(|| {
                let mut acc = vec![0.0f32; dim];
                for u in &updates {
                    scalar_accumulate_weighted(&mut acc, black_box(u), 0.1);
                }
                black_box(acc)
            });
        });

        group.bench_function("simd", |b| {
            b.iter(|| {
                let mut acc = vec![0.0f32; dim];
                for u in &updates {
                    simd_accumulate_weighted(&mut acc, black_box(u), 0.1);
                }
                black_box(acc)
            });
        });

        group.finish();
    }
}

criterion_group!(benches, bench_accumulate, bench_full_combine);
criterion_main!(benches);
