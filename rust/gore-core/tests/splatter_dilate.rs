//! Output check for `SplatterRt::dilate`: a fixed sequence of stamps at the corners, mid-edges and centre of a
//! unit quad, at several target sizes, with every blend mode. The hash of the final `disp` and `dif` bytes was
//! recorded from the code before the 2026-10-10 bounds fix (release build) and must not change.

use glam::Vec3;
use gore_core::splatter::{Stamp, APPLY_MAX, APPLY_MIN};
use gore_core::splatter_rt::{SplatterRt, TexelMap};

fn quad_map(res: usize) -> TexelMap {
    let pos = [Vec3::new(-0.5, -0.5, 0.0), Vec3::new(0.5, -0.5, 0.0), Vec3::new(0.5, 0.5, 0.0), Vec3::new(-0.5, 0.5, 0.0)];
    let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    TexelMap::build(res, &pos, &[Vec3::Z; 4], &[0.0; 4], &uv, &[0, 1, 2, 0, 2, 3])
}

fn stamp(origin: Vec3, half_size: f32, flags: u32, scale: f32) -> Stamp {
    Stamp {
        origin, ax: -Vec3::Z, ay: Vec3::X, az: Vec3::Y, half_size, depth: 0.1, fade_gain: 1.0, angle_wrap: 1.0,
        displacement_tex: 0, diffuse_tex: 1, flags, cloth_depth: 1.0, cloth_displacement: 1.0,
        wound: [0.6 * scale, 0.4 * scale, 0.3 * scale], fx: [0.5 * scale, 0.35 * scale, 0.25 * scale, 0.15 * scale],
        range: [0.0, 1.0], displacement: 1.0, persistent: false, id: 0,
    }
}

fn fnv1a(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= *b as u64;
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// Runs the fixed sequence and returns (hash, raw bytes) of every target's final `disp` then `dif`.
fn run() -> (u64, Vec<u8>) {
    // Texture 0: displacement, texture 1: diffuse. Smooth, deterministic, no randomness.
    let sample = |tex: i32, u: f32, v: f32| -> [f32; 4] {
        if tex == 0 { [u, v, 0.5 * u + 0.25, 1.0] } else { [1.0 - u, 0.5 + 0.5 * v, u * v, 0.75] }
    };
    let e = 0.49f32;
    let spots = [
        Vec3::new(-e, -e, 0.0), Vec3::new(e, -e, 0.0), Vec3::new(-e, e, 0.0), Vec3::new(e, e, 0.0),
        Vec3::new(0.0, -e, 0.0), Vec3::new(0.0, e, 0.0), Vec3::new(-e, 0.0, 0.0), Vec3::new(e, 0.0, 0.0),
        Vec3::ZERO,
    ];
    let modes = [0, APPLY_MAX, APPLY_MIN, 0, APPLY_MAX];
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut raw = Vec::new();
    for res in [1usize, 2, 3, 8, 17, 64] {
        let map = quad_map(res);
        let mut rt = SplatterRt::new(res);
        let mut step = 0usize;
        for (si, spot) in spots.iter().enumerate() {
            for (mi, &flags) in modes.iter().enumerate() {
                let half = [0.03, 0.12, 0.3][(si + mi) % 3];
                let scale = if flags == APPLY_MIN { -0.8 } else { 1.0 - 0.1 * mi as f32 };
                let s = stamp(*spot, half, flags, scale);
                if let Some(r) = rt.apply(&map, &s, false, &sample) {
                    rt.dilate(r);
                }
                step += 1;
            }
        }
        assert_eq!(step, spots.len() * modes.len());
        for px in rt.disp.iter().chain(rt.dif.iter()) {
            raw.extend_from_slice(px);
        }
    }
    fnv1a(&mut h, &raw);
    (h, raw)
}

#[test]
fn dilate_output_matches_the_pre_fix_hash() {
    let (h, raw) = run();
    let nonzero = raw.iter().filter(|b| **b != 0).count();
    eprintln!("splatter dilate harness: fnv1a64={h:016x} bytes={} nonzero={nonzero}", raw.len());
    if let Some(path) = std::env::var_os("SPLATTER_DILATE_DUMP") {
        std::fs::write(path, &raw).unwrap();
    }
    assert!(nonzero > raw.len() / 10, "the harness must actually write the targets");
    // Recorded from the pre-fix code in a release build (debug builds panicked at x == 0 / y == 0).
    assert_eq!(format!("{h:016x}"), "7e38b43b9bbcc045");
}
