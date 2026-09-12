//! Benchmarks for the protocol hot paths.
//!
//! Run with `cargo bench -p rukkit-protocol`.

use std::hint::black_box;

use bytes::BytesMut;
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use rukkit_protocol::codec::{Decoder, Encoder, DEFAULT_COMPRESSION_THRESHOLD};
use rukkit_protocol::crypt::{Decryptor, Encryptor};
use rukkit_protocol::nbt::{self, NbtCompound, NbtTag};
use rukkit_protocol::reader::PacketReader;
use rukkit_protocol::varint;
use rukkit_protocol::writer::PacketWrite;

fn varint_values() -> Vec<i32> {
    // A spread across every byte-length class, which is what a real packet
    // stream looks like: mostly small ids and lengths, occasionally large.
    (0..1024)
        .map(|i| match i % 5 {
            0 => i,
            1 => i * 127,
            2 => i * 16_384,
            3 => i * 2_097_152,
            _ => -i,
        })
        .collect()
}

fn bench_varint(c: &mut Criterion) {
    let values = varint_values();

    let mut group = c.benchmark_group("varint");
    group.throughput(Throughput::Elements(values.len() as u64));

    group.bench_function("write", |b| {
        let mut out = Vec::with_capacity(values.len() * 5);
        b.iter(|| {
            out.clear();
            for &v in &values {
                varint::write_varint(&mut out, black_box(v));
            }
            black_box(out.len())
        });
    });

    let mut encoded = Vec::new();
    for &v in &values {
        varint::write_varint(&mut encoded, v);
    }

    group.bench_function("read", |b| {
        b.iter(|| {
            let mut offset = 0;
            let mut sum = 0i64;
            while offset < encoded.len() {
                let (value, used) = varint::read_varint(&encoded[offset..]).unwrap();
                sum += i64::from(value);
                offset += used;
            }
            black_box(sum)
        });
    });

    group.bench_function("len", |b| {
        b.iter(|| {
            let mut total = 0usize;
            for &v in &values {
                total += varint::varint_len(black_box(v));
            }
            black_box(total)
        });
    });

    group.finish();
}

/// A payload shaped like a real chunk packet: large and highly repetitive.
fn chunk_like_payload() -> Vec<u8> {
    let mut payload = Vec::with_capacity(64 * 1024);
    payload.push(0x27);
    for i in 0..64 * 1024 {
        // Long runs with occasional variation, as paletted section data has.
        payload.push(if i % 97 == 0 { (i % 251) as u8 } else { 0 });
    }
    payload
}

fn bench_framing(c: &mut Criterion) {
    let small = {
        let mut p = vec![0x1Au8];
        p.extend_from_slice(&[0x42; 32]);
        p
    };
    let large = chunk_like_payload();

    let mut group = c.benchmark_group("framing");

    group.throughput(Throughput::Bytes(small.len() as u64));
    group.bench_function("encode_small_uncompressed", |b| {
        let mut encoder = Encoder::new();
        let mut out = BytesMut::with_capacity(1024);
        b.iter(|| {
            out.clear();
            encoder.encode(black_box(&small), &mut out).unwrap();
            black_box(out.len())
        });
    });

    group.throughput(Throughput::Bytes(large.len() as u64));
    group.bench_function("encode_chunk_compressed", |b| {
        let mut encoder = Encoder::new();
        encoder.enable_compression(DEFAULT_COMPRESSION_THRESHOLD);
        let mut out = BytesMut::with_capacity(large.len());
        b.iter(|| {
            out.clear();
            encoder.encode(black_box(&large), &mut out).unwrap();
            black_box(out.len())
        });
    });

    // Pre-encode once so the decode benchmark measures only decoding.
    let wire = {
        let mut encoder = Encoder::new();
        encoder.enable_compression(DEFAULT_COMPRESSION_THRESHOLD);
        let mut out = BytesMut::new();
        encoder.encode(&large, &mut out).unwrap();
        out.to_vec()
    };

    group.bench_function("decode_chunk_compressed", |b| {
        b.iter(|| {
            let mut decoder = Decoder::new();
            decoder.enable_compression(DEFAULT_COMPRESSION_THRESHOLD);
            let mut bytes = wire.clone();
            decoder.feed(&mut bytes);
            black_box(decoder.decode().unwrap().unwrap().len())
        });
    });

    group.finish();
}

fn bench_crypt(c: &mut Criterion) {
    let secret = [0x5Au8; 16];
    let mut data = vec![0u8; 16 * 1024];
    for (i, b) in data.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }

    let mut group = c.benchmark_group("crypt");
    group.throughput(Throughput::Bytes(data.len() as u64));

    group.bench_function("aes128_cfb8_encrypt", |b| {
        let mut encryptor = Encryptor::new(&secret).unwrap();
        let mut buffer = data.clone();
        b.iter(|| {
            encryptor.encrypt(black_box(&mut buffer));
        });
    });

    group.bench_function("aes128_cfb8_decrypt", |b| {
        let mut decryptor = Decryptor::new(&secret).unwrap();
        let mut buffer = data.clone();
        b.iter(|| {
            decryptor.decrypt(black_box(&mut buffer));
        });
    });

    group.finish();
}

fn registry_like_nbt() -> NbtTag {
    let mut root = NbtCompound::new();
    for i in 0..64 {
        let mut entry = NbtCompound::new();
        entry
            .insert("name", format!("minecraft:entry_{i}"))
            .insert("id", i)
            .insert("has_skylight", i % 2 == 0)
            .insert("height", 384i32)
            .insert("min_y", -64i32)
            .insert("ambient_light", 0.0f32);
        root.insert(format!("entry_{i}"), entry);
    }
    NbtTag::Compound(root)
}

fn bench_nbt(c: &mut Criterion) {
    let tag = registry_like_nbt();
    let mut encoded = Vec::new();
    nbt::write_network(&tag, &mut encoded);

    let mut group = c.benchmark_group("nbt");
    group.throughput(Throughput::Bytes(encoded.len() as u64));

    group.bench_function("write", |b| {
        let mut out = Vec::with_capacity(encoded.len());
        b.iter(|| {
            out.clear();
            nbt::write_network(black_box(&tag), &mut out);
            black_box(out.len())
        });
    });

    group.bench_function("read", |b| {
        b.iter(|| {
            let mut r = PacketReader::new(black_box(&encoded));
            black_box(nbt::read_network(&mut r).unwrap())
        });
    });

    group.finish();
}

fn bench_strings(c: &mut Criterion) {
    let names: Vec<String> = (0..256).map(|i| format!("player_name_{i}")).collect();

    let mut group = c.benchmark_group("strings");
    group.throughput(Throughput::Elements(names.len() as u64));

    group.bench_function("write", |b| {
        let mut out = Vec::with_capacity(8192);
        b.iter(|| {
            out.clear();
            for name in &names {
                out.write_string(black_box(name));
            }
            black_box(out.len())
        });
    });

    let mut encoded = Vec::new();
    for name in &names {
        encoded.write_string(name);
    }

    group.bench_function("read", |b| {
        b.iter(|| {
            let mut r = PacketReader::new(black_box(&encoded));
            let mut total = 0usize;
            while !r.is_empty() {
                total += r.read_string(32767).unwrap().len();
            }
            black_box(total)
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_varint,
    bench_framing,
    bench_crypt,
    bench_nbt,
    bench_strings
);
criterion_main!(benches);
