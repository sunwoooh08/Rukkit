//! World storage for Rukkit.
//!
//! The performance of a Minecraft server is decided here more than anywhere
//! else: chunk storage sets the memory footprint, and chunk serialization is
//! the single largest thing the network layer ever sends. Everything in this
//! crate is built around that — paletted containers that cost nothing for
//! uniform sections, packed bit storage with the division turned into a
//! multiply, and bulk operations that work a cell at a time.

pub mod bitstorage;
pub mod block;
pub mod chunk;
pub mod generator;
pub mod palette;

pub use bitstorage::BitStorage;
pub use block::BlockStates;
pub use chunk::{Chunk, ChunkSection, HeightLimits, Heightmaps};
pub use generator::{ChunkGenerator, FlatGenerator, VoidGenerator};
pub use palette::{ContainerKind, PalettedContainer};
pub use rukkit_protocol::{BlockPos, ChunkPos};
