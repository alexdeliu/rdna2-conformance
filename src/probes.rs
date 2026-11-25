//! The probes. Each one is a small HLSL compute shader for one driver
//! behaviour that a console RDNA2 -> DXIL translator can trip over, the inputs
//! it runs on (64 lanes × 4 dwords), and a CPU check of the outputs.

/// What a probe found on one device.
#[derive(Clone, Debug)]
pub enum Outcome {
    Pass,
    /// `n` lanes wrong; `first` describes the first one.
    Fail {
        n: usize,
        first: String,
    },
    /// Measured, with no right answer to hold it to (precision, out-of-range inputs).
    Info(String),
    Skip(String),
}

pub struct Build {
    pub label: &'static str,
    pub args: &'static [&'static str],
}

pub struct Probe {
    pub id: &'static str,
    /// What a failure means, for the report.
    pub about: &'static str,
    /// Where the behaviour was found.
    pub origin: &'static str,
    pub body: &'static str,
    pub builds: &'static [Build],
    pub needs_16bit: bool,
    pub input: fn() -> Vec<u32>,
    /// Inputs, and each build's output (`LANES * 4` dwords).
    pub check: fn(&[u32], &[Vec<u32>]) -> Outcome,
}

pub const LANES: usize = 64;

/// The shader around a probe's `body`: `x` is the lane's `uint4` input, `r`
/// its `uint4` output, `l` the lane.
pub fn source(p: &Probe) -> String {
    format!(
        "RWByteAddressBuffer In : register(u0);\n\
         RWByteAddressBuffer Out : register(u1);\n\
         [RootSignature(\"UAV(u0), UAV(u1)\")]\n\
         [numthreads({LANES}, 1, 1)]\n\
         void main(uint l : SV_GroupIndex) {{\n\
         uint4 x = In.Load4(l * 16);\n\
         uint4 r = uint4(0, 0, 0, 0);\n\
         {}\n\
         Out.Store4(l * 16, r);\n\
         }}\n",
        p.body
    )
}

const PLAIN: &[Build] = &[Build {
    label: "default",
    args: &[],
}];
const FAST_AND_IEEE: &[Build] = &[
    Build {
        label: "default",
        args: &[],
    },
    Build {
        label: "-Gis",
        args: &["-Gis"],
    },
];
const HALF: &[Build] = &[Build {
    label: "default",
    args: &["-enable-16bit-types"],
}];
const HALF_FAST_AND_IEEE: &[Build] = &[
    Build {
        label: "default",
        args: &["-enable-16bit-types"],
    },
    Build {
        label: "-Gis",
        args: &["-enable-16bit-types", "-Gis"],
    },
];

/// xorshift64*: the same inputs on every run.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }
}

fn lanes(seed: u64, fixed: &[[u32; 4]], mut fill: impl FnMut(&mut Rng) -> [u32; 4]) -> Vec<u32> {
    let mut rng = Rng(seed | 1);
    (0..LANES)
        .flat_map(|l| fixed.get(l).copied().unwrap_or_else(|| fill(&mut rng)))
        .collect()
}

fn lane(v: &[u32], l: usize) -> [u32; 4] {
    [v[l * 4], v[l * 4 + 1], v[l * 4 + 2], v[l * 4 + 3]]
}

/// Compares every lane's output with `want`, over the first build only.
fn per_lane(input: &[u32], outs: &[Vec<u32>], want: impl Fn(usize, [u32; 4]) -> [u32; 4], mask: [u32; 4]) -> Outcome {
    let mut bad = Vec::new();
    for l in 0..LANES {
        let (x, got, w) = (lane(input, l), lane(&outs[0], l), want(l, lane(input, l)));
        if (0..4).any(|k| got[k] & mask[k] != w[k] & mask[k]) {
            bad.push(format!("lane {l}: in {x:08x?}, got {got:08x?}, want {w:08x?}"));
        }
    }
    match bad.first() {
        None => Outcome::Pass,
        Some(f) => Outcome::Fail {
            n: bad.len(),
            first: f.clone(),
        },
    }
}

fn cmp_bits(a: u64, b: u64) -> u32 {
    let (sa, sb) = (a as i64, b as i64);
    [
        a < b,
        a <= b,
        a > b,
        a >= b,
        sa < sb,
        sa <= sb,
        sa > sb,
        sa >= sb,
        a == b,
        a != b,
    ]
    .iter()
    .enumerate()
    .fold(0, |m, (i, &c)| m | (u32::from(c) << i))
}

const CMP_BODY: &str = "\
uint64_t a = ((uint64_t)x.y << 32) | x.x;
uint64_t b = ((uint64_t)x.z << 32) | x.y;
int64_t sa = (int64_t)a, sb = (int64_t)b;
r.x = (a < b ? 1u : 0u) | (a <= b ? 2u : 0u) | (a > b ? 4u : 0u) | (a >= b ? 8u : 0u)
    | (sa < sb ? 16u : 0u) | (sa <= sb ? 32u : 0u) | (sa > sb ? 64u : 0u) | (sa >= sb ? 128u : 0u)
    | (a == b ? 256u : 0u) | (a != b ? 512u : 0u);";

const CMP_DISJOINT_BODY: &str = "\
uint64_t a = ((uint64_t)x.y << 32) | x.x;
uint64_t b = ((uint64_t)x.w << 32) | x.z;
int64_t sa = (int64_t)a, sb = (int64_t)b;
r.x = (a < b ? 1u : 0u) | (a <= b ? 2u : 0u) | (a > b ? 4u : 0u) | (a >= b ? 8u : 0u)
    | (sa < sb ? 16u : 0u) | (sa <= sb ? 32u : 0u) | (sa > sb ? 64u : 0u) | (sa >= sb ? 128u : 0u)
    | (a == b ? 256u : 0u) | (a != b ? 512u : 0u);";

const F64_BODY: &str = "\
double a = asdouble(x.x, x.y);
double b = asdouble(x.y, x.z);
asuint(a + b, r.x, r.y);
asuint(a * b, r.z, r.w);";

const F64_DISJOINT_BODY: &str = "\
double a = asdouble(x.x, x.y);
double b = asdouble(x.z, x.w);
asuint(a + b, r.x, r.y);
asuint(a * b, r.z, r.w);";

/// 1.5 + 2.25 style cases first: a wrong pairing shows as an exact other number.
const F64_FIXED: &[[u32; 4]] = &[
    [0, 0x3FF8_0000, 0, 0x4002_0000],
    [0, 0x3FF0_0000, 0x3FF0_0000, 0x4000_0000],
    [0, 0x4000_0000, 0x4000_0000, 0x4008_0000],
];

/// Dwords that make normal doubles as either half (no denormals, NaNs or infinities).
fn f64_dwords(r: &mut Rng) -> [u32; 4] {
    [
        r.next(),
        0x3F00_0000 | (r.next() & 0x00FF_FFFF),
        0x3F00_0000 | (r.next() & 0x00FF_FFFF),
        0x3F00_0000 | (r.next() & 0x00FF_FFFF),
    ]
}

fn f64_want(x: [u32; 4], disjoint: bool) -> [u32; 4] {
    let d = |lo: u32, hi: u32| f64::from_bits(u64::from(lo) | u64::from(hi) << 32);
    let a = d(x[0], x[1]);
    let b = if disjoint { d(x[2], x[3]) } else { d(x[1], x[2]) };
    let (s, m) = ((a + b).to_bits(), (a * b).to_bits());
    [s as u32, (s >> 32) as u32, m as u32, (m >> 32) as u32]
}

/// Dword triples whose order as (lo, hi) pairs and as swapped pairs disagree.
const CMP_FIXED: &[[u32; 4]] = &[
    [0, 0, 1, 0],
    [0, 1, 0, 0],
    [1, 0, 0, 0],
    [0, 0x8000_0000, 0, 0],
    [0xFFFF_FFFF, 0, 1, 0],
    [0, 0xFFFF_FFFF, 0x7FFF_FFFF, 0],
    [5, 5, 5, 5],
    [0x8000_0000, 0x7FFF_FFFF, 0x8000_0000, 1],
];

fn small_dword(r: &mut Rng) -> u32 {
    match r.next() % 4 {
        0 => r.next() % 3,
        1 => 0x8000_0000 | (r.next() % 2),
        2 => 0xFFFF_FFFF - (r.next() % 2),
        _ => r.next(),
    }
}

fn f16_sign_zero_input(r: &mut Rng) -> [u32; 4] {
    // A zero operand (either sign) times anything finite: the product is a zero
    // whose sign is the xor of the signs.
    const FINITE: [u32; 6] = [0x0001, 0x8001, 0x3C00, 0xBC00, 0x7BFF, 0x0400];
    let zero = if r.next().is_multiple_of(2) { 0x0000 } else { 0x8000 };
    let other = FINITE[r.next() as usize % FINITE.len()];
    if r.next().is_multiple_of(2) {
        [zero, other, 0, 0]
    } else {
        [other, zero, 0, 0]
    }
}

fn f32_sign_zero_input(r: &mut Rng) -> [u32; 4] {
    const FINITE: [u32; 6] = [
        0x0000_0001,
        0x8000_0001,
        0x3F80_0000,
        0xBF80_0000,
        0x7F7F_FFFF,
        0x0080_0000,
    ];
    let zero = if r.next().is_multiple_of(2) { 0 } else { 0x8000_0000 };
    let other = FINITE[r.next() as usize % FINITE.len()];
    if r.next().is_multiple_of(2) {
        [zero, other, 0, 0]
    } else {
        [other, zero, 0, 0]
    }
}

/// Builds' outputs compared lane by lane: `Pass` when all agree.
fn builds_agree(input: &[u32], outs: &[Vec<u32>], mask: u32) -> Outcome {
    let mut bad = Vec::new();
    for l in 0..LANES {
        let vals: Vec<u32> = outs.iter().map(|o| o[l * 4] & mask).collect();
        if vals.iter().any(|&v| v != vals[0]) {
            bad.push(format!("lane {l}: in {:08x?}, builds give {vals:08x?}", lane(input, l)));
        }
    }
    match bad.first() {
        None => Outcome::Pass,
        Some(f) => Outcome::Fail {
            n: bad.len(),
            first: f.clone(),
        },
    }
}

fn zero_sign_check(input: &[u32], outs: &[Vec<u32>], sign: u32) -> Outcome {
    let mut parts = Vec::new();
    let mut failed = None;
    for (b, out) in outs.iter().enumerate() {
        let wrong: Vec<usize> = (0..LANES)
            .filter(|&l| {
                let x = lane(input, l);
                let want = (x[0] ^ x[1]) & sign;
                out[l * 4] != want
            })
            .collect();
        parts.push(format!(
            "{}: {} of {LANES} wrong",
            ["default", "-Gis"][b.min(1)],
            wrong.len()
        ));
        if b == 0 && !wrong.is_empty() {
            let l = wrong[0];
            failed = Some((
                wrong.len(),
                format!(
                    "lane {l}: in {:08x?}, got {:#010x}, want {:#010x}",
                    lane(input, l),
                    out[l * 4],
                    (input[l * 4] ^ input[l * 4 + 1]) & sign
                ),
            ));
        }
    }
    match failed {
        None => Outcome::Pass,
        Some((n, first)) => Outcome::Fail {
            n,
            first: format!("{first} ({})", parts.join("; ")),
        },
    }
}

fn f16_to_f64(h: u32) -> f64 {
    let (s, e, m) = ((h >> 15) & 1, (h >> 10) & 0x1F, h & 0x3FF);
    let v = match e {
        0 => f64::from(m) * 2f64.powi(-24),
        31 => {
            if m == 0 {
                f64::INFINITY
            } else {
                f64::NAN
            }
        }
        _ => (1.0 + f64::from(m) / 1024.0) * 2f64.powi(e as i32 - 15),
    };
    if s == 1 { -v } else { v }
}

pub fn all() -> Vec<Probe> {
    vec![
        Probe {
            id: "i64-compare-shared-dword",
            about: "64-bit integer compares of two values that share a dword (v[1:2] vs v[2:3]) order wrong",
            origin: "Marvisak/VIctory#1599 (RDNA2, RDNA3, RDNA4)",
            body: CMP_BODY,
            builds: PLAIN,
            needs_16bit: false,
            input: || lanes(1599, CMP_FIXED, |r| [small_dword(r), small_dword(r), small_dword(r), 0]),
            check: |i, o| {
                per_lane(
                    i,
                    o,
                    |_, x| {
                        [
                            cmp_bits(
                                u64::from(x[1]) << 32 | u64::from(x[0]),
                                u64::from(x[2]) << 32 | u64::from(x[1]),
                            ),
                            0,
                            0,
                            0,
                        ]
                    },
                    [!0, 0, 0, 0],
                )
            },
        },
        Probe {
            id: "i64-compare-disjoint",
            about: "control for the above: the same compares on disjoint dwords",
            origin: "Marvisak/VIctory#1599",
            body: CMP_DISJOINT_BODY,
            builds: PLAIN,
            needs_16bit: false,
            input: || {
                lanes(1600, &[], |r| {
                    [small_dword(r), small_dword(r), small_dword(r), small_dword(r)]
                })
            },
            check: |i, o| {
                per_lane(
                    i,
                    o,
                    |_, x| {
                        [
                            cmp_bits(
                                u64::from(x[1]) << 32 | u64::from(x[0]),
                                u64::from(x[3]) << 32 | u64::from(x[2]),
                            ),
                            0,
                            0,
                            0,
                        ]
                    },
                    [!0, 0, 0, 0],
                )
            },
        },
        Probe {
            id: "f64-shared-dword",
            about: "f64 add and mul of two doubles that share a dword (asdouble(x, y) and asdouble(y, z)), as v_add_f64 v[1:2], v[2:3] lowers",
            origin: "Belobug/VIctory-Belobug-AMD f64 lowering (RX 9070; gfx1036 with makeDouble of the dwords)",
            body: F64_BODY,
            builds: PLAIN,
            needs_16bit: false,
            input: || lanes(0xF64, F64_FIXED, f64_dwords),
            check: |i, o| per_lane(i, o, |_, x| f64_want(x, false), [!0, !0, !0, !0]),
        },
        Probe {
            id: "f64-disjoint",
            about: "control for the above: the same f64 add and mul on disjoint dwords",
            origin: "Belobug/VIctory-Belobug-AMD",
            body: F64_DISJOINT_BODY,
            builds: PLAIN,
            needs_16bit: false,
            input: || lanes(0xF65, F64_FIXED, f64_dwords),
            check: |i, o| per_lane(i, o, |_, x| f64_want(x, true), [!0, !0, !0, !0]),
        },
        Probe {
            id: "sext-bool-to-i64",
            about: "sext i1 -> i64 leaves the high dword 0 (0x00000000_ffffffff for true)",
            origin: "xcl-dev/xcl fuzz seed 540 (RDNA2)",
            // A compare, not `x & 1`: DXC turns a bit test into shl/ashr and a
            // 32-bit sext, while a compare's select of -1 / 0 stays `sext i1 to i64`
            // (check with --dxil).
            body: "int64_t s = (x.x > 5u) ? (int64_t)-1 : (int64_t)0;\nr.x = (uint)s; r.y = (uint)((uint64_t)s >> 32);",
            builds: PLAIN,
            needs_16bit: false,
            input: || {
                lanes(
                    540,
                    &[[6, 0, 0, 0], [5, 0, 0, 0], [0x7FFF_FFFF, 0, 0, 0], [0, 0, 0, 0]],
                    |r| [r.next() % 12, 0, 0, 0],
                )
            },
            check: |i, o| {
                per_lane(
                    i,
                    o,
                    |_, x| if x[0] > 5 { [!0, !0, 0, 0] } else { [0, 0, 0, 0] },
                    [!0, !0, 0, 0],
                )
            },
        },
        Probe {
            id: "f32-mul-negative-zero",
            about: "a product that is -0 comes out +0 (fast-math nsz; DXC's default build vs -Gis)",
            origin: "xcl-dev/xcl HLSL backend vs direct DXIL (RDNA2, RDNA4)",
            body: "r.x = asuint(asfloat(x.x) * asfloat(x.y));",
            builds: FAST_AND_IEEE,
            needs_16bit: false,
            input: || lanes(32, &[], f32_sign_zero_input),
            check: |i, o| zero_sign_check(i, o, 0x8000_0000),
        },
        Probe {
            id: "f16-mul-negative-zero",
            about: "an f16 product that is -0 comes out +0 (fast-math nsz; DXC's default build vs -Gis)",
            origin: "xcl-dev/xcl HLSL backend vs direct DXIL (RDNA2, RDNA4)",
            body: "float16_t a = asfloat16((uint16_t)x.x), b = asfloat16((uint16_t)x.y);\nr.x = (uint)asuint16(a * b);",
            builds: HALF_FAST_AND_IEEE,
            needs_16bit: true,
            input: || lanes(16, &[], f16_sign_zero_input),
            check: |i, o| zero_sign_check(i, o, 0x8000),
        },
        Probe {
            id: "sin-cos-in-turns",
            about: "sin(x * 2pi) / cos(x * 2pi), as translators lower v_sin_f32 / v_cos_f32, differ between DXC's default build and -Gis",
            origin: "xcl-dev/xcl HLSL backend vs direct DXIL (RDNA2, RDNA4)",
            body: "float t = asfloat(x.x) * 6.2831853f;\nr.x = asuint(x.y == 0 ? sin(t) : cos(t));",
            builds: FAST_AND_IEEE,
            needs_16bit: false,
            input: || {
                let xs = [
                    0x7F7F_FFFF,
                    0xFF7F_FFFF,
                    0x47BC_FC00,
                    0x4974_2400,
                    0x3E80_0000,
                    0x4380_0000,
                ];
                (0..LANES)
                    .flat_map(|l| [xs[(l / 2) % xs.len()], (l % 2) as u32, 0, 0])
                    .collect()
            },
            check: |i, o| match builds_agree(i, o, !0) {
                Outcome::Fail { n, first } => Outcome::Info(format!("builds differ on {n} lanes; e.g. {first}")),
                other => other,
            },
        },
        Probe {
            id: "wave-read-first-in-switch",
            about: "WaveReadLaneFirst taken in a divergent switch case returns the lane's own value",
            origin: "xcl-dev/xcl fuzz seed 722 (RDNA4)",
            body: "uint w = WaveReadLaneFirst(x.x);\nuint v;\nswitch (x.x) {\ncase 1: v = w; break;\ncase 2: v = l; break;\ndefault: v = w; break;\n}\nr.x = v; r.y = WaveGetLaneCount();",
            builds: PLAIN,
            needs_16bit: false,
            input: || {
                let mut fixed = vec![[0x7FFF_FFFF, 0, 0, 0]];
                fixed.extend((1..LANES).map(|l| {
                    [
                        if l == 30 {
                            1
                        } else if l % 7 == 3 {
                            2
                        } else {
                            (l as u32) << 8
                        },
                        0,
                        0,
                        0,
                    ]
                }));
                lanes(722, &fixed, |r| [r.next(), 0, 0, 0])
            },
            check: |i, o| {
                let n = o[0][1].max(1) as usize;
                per_lane(
                    i,
                    o,
                    |l, x| {
                        let first = lane(i, l / n * n)[0];
                        [if x[0] == 2 { l as u32 } else { first }, n as u32, 0, 0]
                    },
                    [!0, !0, 0, 0],
                )
            },
        },
        Probe {
            id: "dot2add-f16-rounding",
            about: "dot2add(half2, half2, float) vs the correctly rounded result (precision is implementation-defined; informational)",
            origin: "xcl-dev/xcl v_dot2_f32_f16, RDNA4 vs RDNA2",
            body: "half2 a = half2(asfloat16((uint16_t)(x.x & 0xFFFF)), asfloat16((uint16_t)(x.x >> 16)));\nhalf2 b = half2(asfloat16((uint16_t)(x.y & 0xFFFF)), asfloat16((uint16_t)(x.y >> 16)));\nr.x = asuint(dot2add(a, b, asfloat(x.z)));",
            builds: HALF,
            needs_16bit: true,
            input: || {
                lanes(2, &[], |r| {
                    let h = |r: &mut Rng| 0x3000 + (r.next() % 0x1800);
                    [
                        h(r) | h(r) << 16,
                        h(r) | h(r) << 16,
                        (0x3F00_0000 + (r.next() % 0x0100_0000)) ^ ((r.next() & 1) << 31),
                        0,
                    ]
                })
            },
            check: |i, o| {
                let mut off = 0;
                let mut maxd = 0u32;
                for l in 0..LANES {
                    let x = lane(i, l);
                    let exact = f16_to_f64(x[0] & 0xFFFF) * f16_to_f64(x[1] & 0xFFFF)
                        + f16_to_f64(x[0] >> 16) * f16_to_f64(x[1] >> 16)
                        + f64::from(f32::from_bits(x[2]));
                    let want = (exact as f32).to_bits();
                    let d = want.abs_diff(o[0][l * 4]);
                    if d != 0 {
                        off += 1;
                        maxd = maxd.max(d);
                    }
                }
                if off == 0 {
                    Outcome::Pass
                } else {
                    Outcome::Info(format!("{off} of {LANES} not correctly rounded, at most {maxd} ulp"))
                }
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_bits_follow_unsigned_and_signed_order() {
        // 0x0_00000001 vs 0x1_00000000: unsigned and signed less-than, not equal.
        let m = cmp_bits(1, 1 << 32);
        assert_eq!(m & 0b11, 0b11);
        assert_eq!(m >> 4 & 0b11, 0b11);
        assert_eq!(m >> 8 & 0b11, 0b10);
        // -1 vs 0: unsigned greater, signed less.
        let m = cmp_bits(u64::MAX, 0);
        assert_eq!(m & 0b1111, 0b1100);
        assert_eq!(m >> 4 & 0b1111, 0b0011);
    }

    #[test]
    fn f16_decodes_normals_denormals_and_signs() {
        assert_eq!(f16_to_f64(0x3C00), 1.0);
        assert_eq!(f16_to_f64(0xBC00), -1.0);
        assert_eq!(f16_to_f64(0x0001), 2f64.powi(-24));
        assert_eq!(f16_to_f64(0x7BFF), 65504.0);
        assert!(f16_to_f64(0x7E00).is_nan());
    }

    #[test]
    fn every_probe_has_64_lanes_of_input_and_a_unique_id() {
        let all = all();
        for p in &all {
            assert_eq!((p.input)().len(), LANES * 4, "{}", p.id);
        }
        let mut ids: Vec<_> = all.iter().map(|p| p.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
    }

    #[test]
    fn a_zero_product_takes_the_xor_of_the_signs() {
        let input: Vec<u32> = (0..LANES)
            .flat_map(|l| [if l % 2 == 0 { 0x8000 } else { 0 }, 0x3C00, 0, 0])
            .collect();
        let right: Vec<u32> = (0..LANES)
            .flat_map(|l| [if l % 2 == 0 { 0x8000 } else { 0 }, 0, 0, 0])
            .collect();
        assert!(matches!(zero_sign_check(&input, &[right], 0x8000), Outcome::Pass));
        let wrong = vec![0u32; LANES * 4];
        assert!(matches!(
            zero_sign_check(&input, &[wrong], 0x8000),
            Outcome::Fail { n: 32, .. }
        ));
    }
}
