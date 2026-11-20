//! The scene parser against the fixtures: byte-exact round trips, and the
//! same blocks as rmapi-js (tests/reference/dump.mjs writes X.rmapi.json).

mod common;

use std::fs;

use common::{diff, fixtures};
use remarkable_annotations::scene::{view, Scene};
use serde_json::Value;

#[test]
fn round_trips_byte_for_byte() {
    for path in fixtures() {
        let bytes = fs::read(&path).unwrap();
        let scene = Scene::parse(&bytes).unwrap();
        assert!(
            scene.serialize() == bytes,
            "{}: round trip differs",
            path.display()
        );
    }
}

#[test]
fn parses_known_blocks() {
    for path in fixtures() {
        let scene = Scene::parse(&fs::read(&path).unwrap()).unwrap();
        let unknown: Vec<u8> = scene
            .blocks
            .iter()
            .filter_map(|b| match &b.body {
                remarkable_annotations::scene::BlockBody::Unknown { block_type, .. } => {
                    Some(*block_type)
                }
                _ => None,
            })
            .collect();
        assert!(
            unknown.is_empty(),
            "{}: unparsed blocks {unknown:?}",
            path.display()
        );
    }
}

#[test]
fn matches_rmapi_js() {
    for path in fixtures() {
        let reference: Value =
            serde_json::from_str(&fs::read_to_string(path.with_extension("rmapi.json")).unwrap())
                .unwrap();
        let scene = Scene::parse(&fs::read(&path).unwrap()).unwrap();
        if let Some(d) = diff(&view::blocks_json(&scene), &reference["blocks"], "blocks") {
            panic!("{}: {d}", path.display());
        }
    }
}

/// A small deterministic generator (xorshift), to avoid a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Corrupted files (bytes changed, inserted, removed, truncated) never make
/// the parser panic, and still round-trip byte for byte: what does not
/// parse is kept raw.
#[test]
fn corrupted_files_round_trip() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let header = remarkable_annotations::scene::HEADER_LENGTH;
    for path in fixtures() {
        let original = fs::read(&path).unwrap();
        for _ in 0..300 {
            let mut bytes = original.clone();
            for _ in 0..1 + rng.below(4) {
                let at = header + rng.below(bytes.len() - header);
                match rng.below(4) {
                    0 => bytes[at] = rng.next() as u8,
                    1 => bytes.insert(at, rng.next() as u8),
                    2 if bytes.len() > header + 1 => {
                        bytes.remove(at);
                    }
                    _ => bytes.truncate(at.max(header + 1)),
                }
            }
            let scene = Scene::parse(&bytes).unwrap();
            assert!(
                scene.serialize() == bytes,
                "{}: corrupted file does not round-trip",
                path.display()
            );
        }
    }
}
