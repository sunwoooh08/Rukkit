# Rukkit

A Minecraft **Java Edition 26.2** server core written in Rust, built for throughput.

> **요약 (한국어)** — Paper를 한 줄씩 옮긴 포팅이 아닙니다. Paper는 Mojang 서버 코드를 포함한
> 100만 줄 이상의 Java 코드베이스이고, 그중 상당 부분은 재배포가 불가능해 Paper 자신도 패치
> 형태로 배포합니다. 대신 Rukkit은 **서버 성능을 실제로 좌우하는 계층** — 와이어 코덱, NBT,
> 압축·암호화, 팔레트 청크 저장소, 틱 루프 — 을 프로토콜 776 기준으로 밑바닥부터 구현합니다.
> 서버 목록 핑은 실제 클라이언트와 동작이 확인됐습니다. 로그인은 `login_finished`까지 도달하지만
> 이 패킷의 후행 필드가 26.2에서 어느 쪽인지 오프라인으로는 확정할 수 없어 `login_finished_flag`
> 설정으로 전환하게 해뒀습니다. 플레이 상태 패킷 ID도 아직 미검증입니다. 아래 **Scope** 참고.

---

## Scope, stated plainly

This is **not** a line-by-line port of Paper, and it is worth being direct about why.

Paper is over a million lines of Java built as a patch series on top of Spigot, CraftBukkit and
Mojang's own server. A large part of what it "contains" is decompiled Mojang code that cannot be
redistributed — which is exactly why Paper ships as patches rather than as source. Translating that
body of code is neither a single-session task nor a legally clean one.

What *is* both tractable and useful is the part that decides how a server actually performs. A
Minecraft server spends its time in a small number of places: framing and compressing packets,
encrypting the byte stream, storing and serializing chunks, and keeping a 20 Hz tick loop honest.
Rukkit implements those from first principles against the published wire format, with tests.

### What works today

| Area | Status |
| --- | --- |
| VarInt/VarLong, buffers, strings, positions, UUIDs | Complete, tested against reference encodings |
| Binary NBT, file and network variants, modified UTF-8 | Complete |
| Text components (JSON + NBT forms) | Complete |
| Frame codec: length prefixing, zlib, AES-128-CFB8 | Complete, round-tripped under all four combinations |
| Handshake / Status / Ping | **Works with a real client's server list** |
| Login (offline mode) → Configuration | Reaches the client; `login_finished` layout still being pinned down, see below |
| Paletted chunk storage, heightmaps, flat generation | Complete |
| Tick loop with TPS/MSPT percentiles | Complete |
| Play state | Structurally present, **packet ids provisional** — see below |
| Online mode (Mojang session auth) | Not implemented; refused explicitly at login |
| World persistence (Anvil region files) | Not implemented |
| Entities, physics, redstone, plugins | Not implemented |

231 tests pass, `clippy` is clean, and the status-ping and login paths are covered by integration
tests that drive a real TCP socket through the real codec. The binary has been run against a
protocol-level client: it loads its config, binds, and answers a server-list ping with the correct
status document and an exactly echoed pong payload.

### The provisional bit

Minecraft renumbers **play-state** packet ids nearly every release, and those numbers cannot be
derived from the wire format — they come from the version's own packet report. The tables in
[`packets/ids.rs`](crates/rukkit-protocol/src/packets/ids.rs) carry forward from the preceding
release line and are flagged `PROVISIONAL`; the server logs a warning if a connection reaches play.
Handshake, status, login and configuration ids have been stable since 1.20.2 and are treated as
settled.

The same applies one packet earlier, and a real 26.2 client found it: login reaches `login_finished`
and the client rejects it with

```
io.netty.handler.codec.DecoderException: Failed to decode packet 'clientbound/minecraft:login_finished'
```

That the client names the packet at all means framing, the compression switch and the packet id are
all correct — only the body disagrees. 1.21.2 appended a trailing "strict error handling" boolean
after the profile properties and a later version dropped it again, and which side 26.2 falls on
needs the version's packet report. So it is configurable rather than guessed:

```toml
login_finished_flag = "send-false"   # append the flag (default)
# login_finished_flag = "omit"       # leave it off
# login_finished_flag = "send-true"
```

If login fails on that packet, try `"omit"`. An integration test covers all three layouts, so
whichever turns out to be right is already exercised.

Configuration also ends by disconnecting with an explanatory message rather than entering the world,
because the client needs the 26.2 registry data (dimension types, biomes, damage types) before it
can render anything, and there is nothing truthful to send until those data files are loaded.
Sending plausible-looking fabricated registries would produce a client that fails confusingly rather
than one that fails clearly.

## Version targeting

| | |
| --- | --- |
| Minecraft | 26.2 (released 2026-06-16) |
| Protocol | 776 |
| Data version | 4903 |

2026 switched Minecraft to year-based version numbers: `26.1` replaced what would have been `1.22`,
and `26.2` followed. All three constants live in
[`version.rs`](crates/rukkit-protocol/src/version.rs).

## Layout

```
crates/
  rukkit-protocol/   wire format: varint, NBT, text, framing, compression, encryption, packets
  rukkit-world/      bit storage, paletted containers, chunks, heightmaps, generation
  rukkit-server/     tokio runtime, connection state machine, tick loop, player registry
```

The protocol crate does no I/O and contains no game logic, so the decode path is testable in
isolation and the server crate is free to choose its own threading model. The world crate depends on
the protocol crate for the wire format but not the reverse — chunk packets carry an opaque payload
that the world layer produces, which keeps the dependency acyclic.

## Where the performance comes from

These are design decisions, not micro-optimisations bolted on afterwards.

**Single-value chunk sections cost nothing.** Most 16³ sections in a real world hold exactly one
block — solid stone, or pure air. A paletted container in that state stores no per-block data at
all, so reading, counting and serializing it are O(1). Filling a section is a single assignment
rather than 4096 writes, which is what makes chunk generation keep up with player movement.

**Bit storage divides by multiplying.** Locating an entry means dividing its index by
`values_per_long`, which is not a power of two for most widths. A hardware divide is 20–40 cycles
and sits in the innermost loop of every chunk serialization, so the divisor is turned into a
multiply-and-shift at construction. A test verifies the identity holds over the full index range.

**Counting is per-palette-entry, not per-block.** A section's non-air count evaluates the predicate
once per distinct block type and then sums over the bit storage a cell at a time.

**The status response is built once per distinct player count.** Public servers answer far more
pings than logins; the JSON is cached and handed out as a shared `Arc<str>`.

**No allocation in the steady-state send path.** Each connection reuses one scratch buffer for
packet bodies and one for compression output.

**Compression and encryption state live on one task.** Handshake through configuration runs
sequentially on the task that owns the socket, so the codec's ordered state — which must switch at
exactly the right byte — needs no synchronisation at all.

**`panic = "unwind"` is deliberate.** One connection hitting a malformed-packet panic must take down
that task only, not the server. An integration test asserts exactly that.

### Benchmarks

```sh
cargo bench -p rukkit-protocol --bench codec
cargo bench -p rukkit-world --bench chunk
```

Measured on a 4-core container in criterion's `--quick` mode, release profile with fat LTO. These
are indicative, not competitive claims — there is no Java baseline measured here, and `--quick` uses
too few samples for criterion to report statistical significance.

| Benchmark | Result |
| --- | --- |
| VarInt write / read | 250 / 178 Melem/s |
| `varint_len` | 653 Melem/s |
| Frame encode, small packet, uncompressed | 8.0 ns (3.8 GiB/s) |
| Frame encode, 64 KiB chunk-like, zlib-6 | 339 µs (184 MiB/s) |
| Frame decode, same payload | 26.5 µs (2.3 GiB/s) |
| AES-128-CFB8, either direction | 47 MiB/s |
| NBT write / read, registry-shaped document | 514 / 147 MiB/s |
| Bit storage random `get` | ~535 Melem/s |
| Bit storage bulk `for_each` | ~1.28 Gelem/s |
| Palette `get`, uniform section | 3.26 Gelem/s |
| Palette `get`, indirect section | 466 Melem/s |
| Fill a whole 4096-block section | 0.93 ns |
| Non-air count, uniform section | 0.61 ns |
| Serialize a chunk: empty / flat / worst case | 276 ns / 6.4 µs / 155 µs |
| Generate a flat chunk | 32.7 µs |
| Generate a view-distance-10 area (441 chunks) | 14.3 ms |

Two of these are worth reading closely.

**The single-value fast paths are not a rounding error.** Counting non-air blocks in a uniform
section takes 0.61 ns against 9.0 µs for a palettised one — four orders of magnitude, because the
predicate runs once instead of 4096 times. Filling a section is 0.93 ns rather than 4096 writes.
Since most sections in a real world are uniform, this is where the memory and CPU budget is won.

**The benchmarks already earned their keep.** The first run showed flat chunk generation at 231 µs,
of which 206 µs was heightmap recomputation — it was scanning every column down the full 384-block
height even though everything above the surface was empty air. Skipping empty sections (an O(1)
question for a single-value container) cut it to 21 µs, taking chunk generation to 32.7 µs and a
view-distance-10 area from 98.7 ms to 14.3 ms. A test compares the result against a deliberately
naive scan so the optimisation cannot drift.

Note that AES-CFB8 costs one AES block operation *per byte* and is strictly serial — that is what
the protocol specifies, not an implementation choice — so 47 MiB/s per connection is close to the
ceiling for the mode even with AES-NI. It is far above what a Minecraft connection actually carries.

## Running it

```sh
cargo run --release
```

On first start this writes `rukkit.toml` with the defaults and begins listening on `0.0.0.0:25565`.
Add a 26.2 client's server list entry pointing at it and the ping, MOTD, player count and version
will come back correctly.

```toml
bind = "0.0.0.0:25565"
motd = "A Rukkit server"
max_players = 20
view_distance = 10
compression_threshold = 256   # negative disables
online_mode = false           # not implemented; login is refused when true
tick_rate = 20
flat_world = true
```

Logging follows `RUST_LOG`, e.g. `RUST_LOG=rukkit_server=debug cargo run --release`.

## Testing

```sh
cargo test --workspace     # 231 tests
cargo clippy --workspace --all-targets
```

The test suite deliberately covers the failure modes that matter for a server exposed to the open
internet: truncated and oversized varints, negative length prefixes, zip bombs declaring huge
uncompressed sizes, NBT nested past the depth limit, unpaired surrogates in modified UTF-8, and a
client that opens a socket and says nothing.

## Roadmap

In the order that unblocks the most:

1. **Load the 26.2 data files** — block, registry and packet reports. This unblocks real block state
   ids, registry data during configuration, and verification of the play id table.
2. **Play state** — validate ids, then join, chunk streaming, movement, keep-alive.
3. **Anvil region I/O** — read and write existing worlds.
4. **Online mode** — RSA key exchange and session-server verification.
5. **Entities and world ticking.**
6. **A plugin API.** Bukkit-compatible is not a goal; the JVM API surface does not translate.

## Licence

MIT. This is an independent implementation against the published wire format; it contains no Mojang
or Paper source.
