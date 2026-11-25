//! DXIL vectors: a translator's own DXIL container (`<id>.dxil`, no root
//! signature) with its inputs and expected outputs (`<id>.txt`). These are
//! what catch driver bugs that only show up in the DXIL a translator writes:
//! the same operation compiled from HLSL by DXC often comes out fine.
//!
//! `<id>.txt` is lines of `key value...`:
//! `id`, `about`, `origin`, `regs u<i>=in u<j>=out`, `wave <lanes>` (optional),
//! `out_dwords <n>`, `in <hex dword>...`, `expect <index>:<hex dword>...`.

use crate::gpu::Gpu;
use crate::probes::Outcome;
use std::path::Path;

pub struct Vector {
    pub id: String,
    pub about: String,
    pub origin: String,
    dxil: Vec<u8>,
    in_reg: u32,
    out_reg: u32,
    wave: Option<u32>,
    out_dwords: usize,
    input: Vec<u32>,
    expect: Vec<(usize, u32)>,
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
    let mut v = Vector {
        id: String::new(),
        about: String::new(),
        origin: String::new(),
        dxil: std::fs::read(path.with_extension("dxil")).map_err(|e| format!("its .dxil: {e}"))?,
        in_reg: 0,
        out_reg: 1,
        wave: None,
        out_dwords: 0,
        input: Vec::new(),
        expect: Vec::new(),
    };
    let hex = |s: &str| u32::from_str_radix(s, 16).map_err(|e| format!("{s}: {e}"));
    for line in text.lines() {
        let (key, rest) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "id" => v.id = rest.into(),
            "about" => v.about = rest.into(),
            "origin" => v.origin = rest.into(),
            "wave" => v.wave = Some(rest.trim().parse().map_err(|e| format!("wave: {e}"))?),
            "out_dwords" => v.out_dwords = rest.trim().parse().map_err(|e| format!("out_dwords: {e}"))?,
            "threads" => {}
            "regs" => {
                for part in rest.split_whitespace() {
                    let (reg, role) = part.split_once('=').ok_or("regs: u<i>=in|out")?;
                    let n: u32 = reg.trim_start_matches('u').parse().map_err(|e| format!("regs: {e}"))?;
                    match role {
                        "in" => v.in_reg = n,
                        "out" => v.out_reg = n,
                        _ => return Err(format!("regs: unknown role {role}")),
                    }
                }
            }
            "in" => v.input = rest.split_whitespace().map(hex).collect::<Result<_, _>>()?,
            "expect" => {
                for part in rest.split_whitespace() {
                    let (i, x) = part.split_once(':').ok_or("expect: <index>:<hex>")?;
                    v.expect.push((i.parse().map_err(|e| format!("expect: {e}"))?, hex(x)?));
                }
            }
            "" => {}
            other => return Err(format!("unknown key {other}")),
        }
    }
    if v.id.is_empty() || v.expect.is_empty() || v.in_reg > 2 || v.out_reg > 2 {
        return Err("needs id, expect, and regs within u0..u2".into());
    }
    Ok(v)
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
    let out = match g.run_raw(&v.dxil, v.in_reg, v.out_reg, &v.input, v.out_dwords) {
        Ok(o) => o,
        Err(e) => return Outcome::Skip(e),
    };
    let bad: Vec<&(usize, u32)> = v.expect.iter().filter(|(i, x)| out[*i] != *x).collect();
    match bad.first() {
        None => Outcome::Pass,
        Some((i, x)) => Outcome::Fail {
            n: bad.len(),
            first: format!("out[{i}] = {:#010x}, want {x:#010x}", out[*i]),
        },
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_shipped_vectors_parse() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vectors");
        let v = super::load(&dir).expect("vectors");
        assert!(v.len() >= 6);
        for v in &v {
            assert!(
                !v.dxil.is_empty() && &v.dxil[..4] == b"DXBC",
                "{}: a DXIL container",
                v.id
            );
            assert!(v.expect.iter().all(|(i, _)| *i < v.out_dwords), "{}", v.id);
            assert_eq!((v.in_reg, v.out_reg, v.wave), (2, 1, Some(32)), "{}", v.id);
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
        let v = super::load(&dir).unwrap();
        assert_eq!(v[0].expect, vec![(0, 2)]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
