//! DXIL vectors: a translator's own DXIL container (`<id>.dxil`) with its
//! inputs and expected outputs (`<id>.txt`). These catch driver bugs that only
//! show up in the DXIL a translator writes: the same operation compiled from
//! HLSL by DXC often comes out fine.
//!
//! `<id>.txt` is lines of `key value...`: `id`, `about`, `origin`, then one of
//! two layouts.
//!
//! - `layout root-uavs` (the default; XCL's kernels): root UAVs `u0..u2`.
//!   Keys: `regs u<i>=in u<j>=out`, `wave <lanes>` (optional),
//!   `out_dwords <n>`, `in <hex dword>...`, `expect <index>:<hex dword>...`.
//!   One dispatch.
//! - `layout victory` (VIctory's one-thread kernels): user data in root
//!   constants `b0`, the result in a raw UAV at index 0 of a directly indexed
//!   heap. Keys: `ud_dwords <n>`, `ud_inputs <ud index>:<input>...`,
//!   `heap_raw_uav_dwords <n>`, and one `case <in0> <in1> <in2> <in3> <expected dword 0>`
//!   line per dispatch (hex).

use crate::gpu::Gpu;
use crate::probes::Outcome;
use std::path::Path;

enum Layout {
    RootUavs {
        in_reg: u32,
        out_reg: u32,
        out_dwords: usize,
        input: Vec<u32>,
        expect: Vec<(usize, u32)>,
    },
    Victory {
        ud_dwords: u32,
        inputs: Vec<(usize, usize)>,
        uav_dwords: u32,
        cases: Vec<([u32; 4], u32)>,
    },
}

pub struct Vector {
    pub id: String,
    pub about: String,
    pub origin: String,
    dxil: Vec<u8>,
    wave: Option<u32>,
    layout: Layout,
}

pub fn load(dir: &Path) -> Result<Vec<Vector>, String> {
    let mut out = Vec::new();
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "txt"))
        .collect();
    paths.sort();
    for p in paths {
        out.push(parse(&p).map_err(|e| format!("{}: {e}", p.display()))?);
    }
    Ok(out)
}

fn parse(path: &Path) -> Result<Vector, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let dxil = std::fs::read(path.with_extension("dxil")).map_err(|e| format!("its .dxil: {e}"))?;
    let hex = |s: &str| u32::from_str_radix(s, 16).map_err(|e| format!("{s}: {e}"));
    let num = |k: &str, s: &str| s.trim().parse::<usize>().map_err(|e| format!("{k}: {e}"));
    let (mut id, mut about, mut origin, mut layout, mut wave) = (
        String::new(),
        String::new(),
        String::new(),
        "root-uavs".to_string(),
        None,
    );
    let (mut in_reg, mut out_reg, mut out_dwords, mut input, mut expect) = (0u32, 1u32, 0usize, Vec::new(), Vec::new());
    let (mut ud_dwords, mut inputs, mut uav_dwords, mut cases) = (0usize, Vec::new(), 1usize, Vec::new());
    for line in text.lines() {
        let (key, rest) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "id" => id = rest.into(),
            "about" => about = rest.into(),
            "origin" => origin = rest.into(),
            "layout" => layout = rest.trim().into(),
            "wave" => wave = Some(num("wave", rest)? as u32),
            "threads" | "" => {}
            "out_dwords" => out_dwords = num("out_dwords", rest)?,
            "regs" => {
                for part in rest.split_whitespace() {
                    let (reg, role) = part.split_once('=').ok_or("regs: u<i>=in|out")?;
                    let n = num("regs", reg.trim_start_matches('u'))? as u32;
                    match role {
                        "in" => in_reg = n,
                        "out" => out_reg = n,
                        _ => return Err(format!("regs: unknown role {role}")),
                    }
                }
            }
            "in" => input = rest.split_whitespace().map(hex).collect::<Result<_, _>>()?,
            "expect" => {
                for part in rest.split_whitespace() {
                    let (i, x) = part.split_once(':').ok_or("expect: <index>:<hex>")?;
                    expect.push((num("expect", i)?, hex(x)?));
                }
            }
            "ud_dwords" => ud_dwords = num("ud_dwords", rest)?,
            "heap_raw_uav_dwords" => uav_dwords = num("heap_raw_uav_dwords", rest)?,
            "ud_inputs" => {
                for part in rest.split_whitespace() {
                    let (i, k) = part.split_once(':').ok_or("ud_inputs: <ud index>:<input>")?;
                    inputs.push((num("ud_inputs", i)?, num("ud_inputs", k)?));
                }
            }
            "case" => {
                let w: Vec<u32> = rest.split_whitespace().map(hex).collect::<Result<_, _>>()?;
                let [a, b, c, d, want] = w[..] else {
                    return Err("case: 4 inputs and the expected dword".into());
                };
                cases.push(([a, b, c, d], want));
            }
            other => return Err(format!("unknown key {other}")),
        }
    }
    if id.is_empty() {
        return Err("needs an id".into());
    }
    let layout = match layout.as_str() {
        "root-uavs" => {
            if expect.is_empty() || in_reg > 2 || out_reg > 2 || expect.iter().any(|e| e.0 >= out_dwords) {
                return Err("root-uavs needs expect within out_dwords, and regs within u0..u2".into());
            }
            Layout::RootUavs {
                in_reg,
                out_reg,
                out_dwords,
                input,
                expect,
            }
        }
        "victory" => {
            if cases.is_empty() || ud_dwords == 0 || inputs.iter().any(|&(i, k)| i >= ud_dwords || k > 3) {
                return Err("victory needs cases, ud_dwords, and ud_inputs within them".into());
            }
            Layout::Victory {
                ud_dwords: ud_dwords as u32,
                inputs,
                uav_dwords: uav_dwords as u32,
                cases,
            }
        }
        other => return Err(format!("unknown layout {other}")),
    };
    Ok(Vector {
        id,
        about,
        origin,
        dxil,
        wave,
        layout,
    })
}

pub fn run(g: &Gpu, v: &Vector) -> Outcome {
    if g.shader_model() < 0x66 {
        return Outcome::Skip("below SM 6.6".into());
    }
    if let Some(w) = v.wave {
        let (lo, hi) = g.wave_lanes();
        if w < lo || w > hi {
            return Outcome::Skip(format!("needs {w}-lane waves (device: {lo}..{hi})"));
        }
    }
    match &v.layout {
        Layout::RootUavs {
            in_reg,
            out_reg,
            out_dwords,
            input,
            expect,
        } => {
            let out = match g.run_raw(&v.dxil, *in_reg, *out_reg, input, *out_dwords) {
                Ok(o) => o,
                Err(e) => return Outcome::Skip(e),
            };
            let bad: Vec<&(usize, u32)> = expect.iter().filter(|(i, x)| out[*i] != *x).collect();
            match bad.first() {
                None => Outcome::Pass,
                Some((i, x)) => Outcome::Fail {
                    n: bad.len(),
                    first: format!("out[{i}] = {:#010x}, want {x:#010x}", out[*i]),
                },
            }
        }
        Layout::Victory {
            ud_dwords,
            inputs,
            uav_dwords,
            cases,
        } => {
            let ins: Vec<[u32; 4]> = cases.iter().map(|c| c.0).collect();
            let out = match g.run_victory(&v.dxil, *ud_dwords, inputs, *uav_dwords, &ins) {
                Ok(o) => o,
                Err(e) => return Outcome::Skip(e),
            };
            let bad: Vec<usize> = (0..cases.len()).filter(|&i| out[i] != cases[i].1).collect();
            match bad.first() {
                None => Outcome::Pass,
                Some(&i) => Outcome::Fail {
                    n: bad.len(),
                    first: format!("case {:08x?}: got {:#x}, want {:#x}", cases[i].0, out[i], cases[i].1),
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Layout;

    #[test]
    fn the_shipped_vectors_parse() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vectors");
        let v = super::load(&dir).expect("vectors");
        assert!(v.len() >= 11);
        for v in &v {
            assert!(
                !v.dxil.is_empty() && &v.dxil[..4] == b"DXBC",
                "{}: a DXIL container",
                v.id
            );
            match &v.layout {
                Layout::RootUavs { in_reg, out_reg, .. } => {
                    assert_eq!((*in_reg, *out_reg, v.wave), (2, 1, Some(32)), "{}", v.id)
                }
                Layout::Victory { ud_dwords, cases, .. } => assert!(*ud_dwords >= 4 && cases.len() >= 12, "{}", v.id),
            }
        }
    }

    #[test]
    fn a_vector_file_needs_an_id_and_expectations() {
        let dir = std::env::temp_dir().join(format!("rdna2-conformance-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.dxil"), b"DXBC").unwrap();
        std::fs::write(dir.join("a.txt"), "id a\nregs u2=in u1=out\nout_dwords 1\n").unwrap();
        assert!(super::load(&dir).is_err());
        std::fs::write(
            dir.join("a.txt"),
            "id a\nregs u2=in u1=out\nout_dwords 1\nin 00000001\nexpect 0:00000002\n",
        )
        .unwrap();
        assert!(super::load(&dir).is_ok());
        std::fs::write(
            dir.join("a.txt"),
            "id a\nlayout victory\nud_dwords 8\nud_inputs 0:0 9:1\ncase 1 2 3 4 1\n",
        )
        .unwrap();
        assert!(super::load(&dir).is_err(), "a ud index outside ud_dwords");
        std::fs::write(
            dir.join("a.txt"),
            "id a\nlayout victory\nud_dwords 8\nud_inputs 0:0 1:1\ncase 1 2 3 4 1\n",
        )
        .unwrap();
        assert!(super::load(&dir).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
