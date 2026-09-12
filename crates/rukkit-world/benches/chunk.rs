//! Benchmarks for chunk storage and serialization.
//!
//! Run with `cargo bench -p rukkit-world`.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use rukkit_world::block::BlockStates;
use rukkit_world::chunk::{Chunk, ChunkSection, HeightLimits, SECTION_VOLUME};
use rukkit_world::generator::{ChunkGenerator, FlatGenerator};
use rukkit_world::palette::{ContainerKind, PalettedContainer};
use rukkit_world::{BitStorage, ChunkPos};

fn bench_bitstorage(c: &mut Criterion) {
    let mut group = c.benchmark_group("bitstorage");
    group.throughput(Throughput::Elements(SECTION_VOLUME as u64));

    for bits in [4u32, 5, 9, 15] {
        let mut storage = BitStorage::new(SECTION_VOLUME, bits);
        let max = (1u64 << bits) - 1;
        for i in 0..SECTION_VOLUME {
            storage.set(i, (i as u64 % (max + 1)) as u32);
        }

        group.bench_function(format!("get_{bits}bit"), |b| {
            b.iter(|| {
                let mut sum = 0u64;
                for i in 0..SECTION_VOLUME {
                    sum += u64::from(storage.get(black_box(i)));
                }
                black_box(sum)
            });
        });

        group.bench_function(format!("for_each_{bits}bit"), |b| {
            b.iter(|| {
                let mut sum = 0u64;
                storage.for_each(|_, value| sum += u64::from(value));
                black_box(sum)
            });
        });

        group.bench_function(format!("set_{bits}bit"), |b| {
            let mut target = BitStorage::new(SECTION_VOLUME, bits);
            b.iter(|| {
                for i in 0..SECTION_VOLUME {
                    target.set(black_box(i), (i as u64 % (max + 1)) as u32);
                }
            });
        });
    }

    group.finish();
}

fn bench_palette(c: &mut Criterion) {
    let states = BlockStates::placeholder();
    let direct_bits = states.direct_bits();

    let mut group = c.benchmark_group("palette");
    group.throughput(Throughput::Elements(SECTION_VOLUME as u64));

    // A uniform section: the case that dominates a real world.
    group.bench_function("get_uniform", |b| {
        let container = PalettedContainer::filled(ContainerKind::Blocks, direct_bits, states.stone);
        b.iter(|| {
            let mut sum = 0u64;
            for i in 0..SECTION_VOLUME {
                sum += u64::from(container.get(black_box(i)));
            }
            black_box(sum)
        });
    });

    // A typical surface section: a handful of block types.
    group.bench_function("get_indirect", |b| {
        let mut container =
            PalettedContainer::filled(ContainerKind::Blocks, direct_bits, states.air);
        for i in 0..SECTION_VOLUME {
            container.set(i, (i % 8) as u32);
        }
        b.iter(|| {
            let mut sum = 0u64;
            for i in 0..SECTION_VOLUME {
                sum += u64::from(container.get(black_box(i)));
            }
            black_box(sum)
        });
    });

    group.bench_function("fill_whole_section", |b| {
        let mut container =
            PalettedContainer::filled(ContainerKind::Blocks, direct_bits, states.air);
        b.iter(|| {
            container.fill(black_box(states.stone));
        });
    });

    group.bench_function("set_promoting_to_direct", |b| {
        b.iter(|| {
            let mut container =
                PalettedContainer::filled(ContainerKind::Blocks, direct_bits, states.air);
            // Forces both a widen and the switch to direct storage.
            for i in 0..SECTION_VOLUME {
                container.set(i, (i % 400) as u32);
            }
            black_box(container.bits_per_entry())
        });
    });

    group.bench_function("count_non_air_uniform", |b| {
        let container = PalettedContainer::filled(ContainerKind::Blocks, direct_bits, states.stone);
        b.iter(|| black_box(container.count(|id| !states.is_air(id))));
    });

    group.bench_function("count_non_air_indirect", |b| {
        let mut container =
            PalettedContainer::filled(ContainerKind::Blocks, direct_bits, states.air);
        for i in 0..SECTION_VOLUME / 2 {
            container.set(i, states.stone);
        }
        b.iter(|| black_box(container.count(|id| !states.is_air(id))));
    });

    group.finish();
}

fn bench_serialization(c: &mut Criterion) {
    let states = BlockStates::placeholder();
    let limits = HeightLimits::OVERWORLD;

    let flat = FlatGenerator::default().generate(ChunkPos::new(0, 0), limits, &states);

    // A worst case: every section varied enough to be worth a wide palette.
    let mut varied = Chunk::empty(ChunkPos::new(0, 0), limits, &states);
    for index in 0..limits.section_count() {
        let section = varied.section_mut(index).unwrap();
        for i in 0..SECTION_VOLUME {
            section.set_block(i & 0xF, (i >> 8) & 0xF, (i >> 4) & 0xF, (i % 64) as u32);
        }
    }

    let mut group = c.benchmark_group("chunk_serialize");

    group.bench_function("flat_chunk", |b| {
        let mut out = Vec::with_capacity(flat.sections_wire_len());
        b.iter(|| {
            out.clear();
            flat.write_sections(&states, &mut out);
            black_box(out.len())
        });
    });

    group.bench_function("varied_chunk", |b| {
        let mut out = Vec::with_capacity(varied.sections_wire_len());
        b.iter(|| {
            out.clear();
            varied.write_sections(&states, &mut out);
            black_box(out.len())
        });
    });

    group.bench_function("empty_chunk", |b| {
        let empty = Chunk::empty(ChunkPos::new(0, 0), limits, &states);
        let mut out = Vec::with_capacity(empty.sections_wire_len());
        b.iter(|| {
            out.clear();
            empty.write_sections(&states, &mut out);
            black_box(out.len())
        });
    });

    group.finish();
}

fn bench_generation(c: &mut Criterion) {
    let states = BlockStates::placeholder();
    let limits = HeightLimits::OVERWORLD;
    let generator = FlatGenerator::default();

    let mut group = c.benchmark_group("generation");

    group.bench_function("flat_chunk", |b| {
        let mut n = 0i32;
        b.iter(|| {
            n += 1;
            black_box(generator.generate(ChunkPos::new(n, 0), limits, &states))
        });
    });

    // View distance 10 is 21x21 chunks, the load on a player joining.
    group.throughput(Throughput::Elements(441));
    group.bench_function("view_distance_10", |b| {
        b.iter(|| {
            let mut total = 0usize;
            for x in -10..=10 {
                for z in -10..=10 {
                    let chunk = generator.generate(ChunkPos::new(x, z), limits, &states);
                    total += chunk.sections().len();
                }
            }
            black_box(total)
        });
    });

    group.finish();
}

fn bench_heightmaps(c: &mut Criterion) {
    let states = BlockStates::placeholder();
    let limits = HeightLimits::OVERWORLD;
    let chunk = FlatGenerator::default().generate(ChunkPos::new(0, 0), limits, &states);

    let mut group = c.benchmark_group("heightmaps");
    group.bench_function("recompute", |b| {
        let mut working = chunk.clone();
        b.iter(|| {
            working.recompute_heightmaps(black_box(&states));
        });
    });
    group.finish();
}

fn bench_section_count(c: &mut Criterion) {
    let states = BlockStates::placeholder();
    let mut group = c.benchmark_group("section");

    group.bench_function("non_air_count_solid", |b| {
        let section = ChunkSection::filled(&states, states.stone, 0);
        b.iter(|| black_box(section.non_air_count(&states)));
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_bitstorage,
    bench_palette,
    bench_serialization,
    bench_generation,
    bench_heightmaps,
    bench_section_count
);
criterion_main!(benches);
