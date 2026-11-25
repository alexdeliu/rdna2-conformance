// How the `xcl-*` vectors were made. Paste this at the end of xcl-dev/xcl's
// `crates/xcl/src/gpu/dxil/fuzz_tests.rs` (it uses that file's private fuzz IR,
// emitter `emit` and CPU interpreter `interpret`), then run
//
//   $env:TEMP_OUT = "<this repo>\vectors"
//   cargo test -p xcl --lib export_conformance_vectors -- --ignored --nocapture
//
// It writes `<id>.dxil` (XCL's direct-DXIL output, no root signature; root UAV
// u1 = results, u2 = inputs) and `<id>.txt` (inputs and the interpreter's
// expected results), and reports how many results differ on the adapter it ran
// on (`XCL_ADAPTER` picks it). Made at xcl-dev/xcl 6020a33.

#[test]
#[ignore]
fn export_conformance_vectors() {
    use Opnd::{C, V};
    let dir = std::path::PathBuf::from(std::env::var("TEMP_OUT").expect("TEMP_OUT"));
    std::fs::create_dir_all(&dir).unwrap();
    let ins = |dst, op| Inst { dst, op };
    // Seed 540 (RDNA2): b = trunc x to i1; s = sext b to i64 [; ashr(s, 63) - (-7)].
    let s540 = |shift: bool| {
        let mut insts = vec![ins(2, Op::Cast(CastOp::Trunc, V(1), T::I1)), ins(3, Op::Cast(CastOp::SExt, V(2), T::I64))];
        if shift {
            insts.push(ins(4, Op::Bin(BinOp::AShr, V(3), C(T::I64, 63))));
            insts.push(ins(5, Op::Bin(BinOp::Sub, V(4), C(T::I64, (-7i64) as u64))));
        }
        Prog { tys: vec![T::I32, T::I32, T::I1, T::I64, T::I64, T::I64], segs: vec![Seg::Load { slot: 0, dsts: vec![1] }, Seg::Insts(insts)], lane: 0, outs: vec![if shift { 5 } else { 3 }], gpu_safe: true }
    };
    // Seed 722 (RDNA4): w = WaveReadLaneFirst(x); a divergent switch on x whose case 1 takes w.
    let s722 = || {
        let cases = vec![(1u64, vec![], V(3)), (2, vec![], V(0))];
        Prog { tys: vec![T::I32; 5], segs: vec![Seg::Load { slot: 0, dsts: vec![1, 2] }, Seg::Wave { v: V(1), lane: None, dst: 3 }, Seg::Switch { sel: V(1), cases, default: (vec![], V(3)), phi: 4 }], lane: 0, outs: vec![4], gpu_safe: true }
    };
    // Seed 1599 (RDNA4): c = Countbits.i64(2); ((c >> (lane & 31)) | Bfrev(c)) & 31.
    let s1599 = || {
        let insts = vec![
            ins(3, Op::DxUn(DxUn::Count, C(T::I64, 2))),
            ins(8, Op::Bin(BinOp::And, V(0), C(T::I32, 31))),
            ins(9, Op::Bin(BinOp::LShr, V(3), V(8))),
            ins(4, Op::DxUn(DxUn::Rev, V(3))),
            ins(10, Op::Bin(BinOp::Or, V(9), V(4))),
            ins(5, Op::Bin(BinOp::And, V(10), C(T::I32, 31))),
        ];
        let tys = vec![T::I32, T::I32, T::I64, T::I32, T::I32, T::I32, T::I32, T::I32, T::I32, T::I32, T::I32];
        Prog { tys, segs: vec![Seg::Load { slot: 0, dsts: vec![1] }, Seg::Insts(insts)], lane: 0, outs: vec![5, 3, 9, 4], gpu_safe: true }
    };
    // a = x1:x0 against b = x2:x1 (shared x1) or x3:x2 (disjoint): ult, slt, ugt, sgt.
    let cmp64 = |shared: bool| {
        let mut insts = vec![
            ins(5, Op::Cast(CastOp::ZExt, V(1), T::I64)),
            ins(6, Op::Cast(CastOp::ZExt, V(2), T::I64)),
            ins(7, Op::Bin(BinOp::Shl, V(6), C(T::I64, 32))),
            ins(8, Op::Bin(BinOp::Or, V(5), V(7))),
        ];
        let (lo, hi) = if shared { (2, 3) } else { (3, 4) };
        insts.push(ins(9, Op::Cast(CastOp::ZExt, V(lo), T::I64)));
        insts.push(ins(10, Op::Cast(CastOp::ZExt, V(hi), T::I64)));
        insts.push(ins(11, Op::Bin(BinOp::Shl, V(10), C(T::I64, 32))));
        insts.push(ins(12, Op::Bin(BinOp::Or, V(9), V(11))));
        let mut outs = vec![];
        for (k, pr) in [Pred::ULt, Pred::SLt, Pred::UGt, Pred::SGt].into_iter().enumerate() {
            insts.push(ins(13 + 2 * k, Op::Cmp(pr, V(8), V(12))));
            insts.push(ins(14 + 2 * k, Op::Cast(CastOp::ZExt, V(13 + 2 * k), T::I32)));
            outs.push(14 + 2 * k);
        }
        let mut tys = vec![T::I32, T::I32, T::I32, T::I32, T::I32, T::I64, T::I64, T::I64, T::I64, T::I64, T::I64, T::I64, T::I64];
        for _ in 0..4 {
            tys.push(T::I1);
            tys.push(T::I32);
        }
        Prog { tys, segs: vec![Seg::Load { slot: 0, dsts: vec![1, 2, 3, 4] }, Seg::Insts(insts)], lane: 0, outs, gpu_safe: true }
    };
    // Small dwords whose (lo, hi) and swapped orders disagree.
    let small: Vec<u32> = {
        let mut r = Rng::new(99);
        (0..64 * IN_SLOTS as usize).map(|i| match r.below(4) { 0 => (r.bits(2) as u32) % 3, 1 => 0x8000_0000 | (i as u32 & 1), 2 => 0xFFFF_FFFF, _ => r.bits(32) as u32 }).collect()
    };
    let cases: Vec<(&str, &str, Prog, Vec<u32>)> = vec![
        ("xcl-sext-i1-to-i64", "sext i1 -> i64 (trunc of the input to i1): the high dword must be all ones for a set bit", s540(false), inputs(540)),
        ("xcl-seed-540", "seed 540's shape: ashr(sext i1 -> i64, 63) - (-7)", s540(true), inputs(540)),
        ("xcl-seed-722", "seed 722's shape: WaveReadLaneFirst, then a divergent switch whose case 1 reads it", s722(), inputs(722)),
        ("xcl-seed-1599", "seed 1599's shape: Countbits.i64(2), shifted by the lane and or'd with its Bfrev", s1599(), inputs(722)),
        ("xcl-i64-compare-shared-dword", "unsigned and signed i64 compares of x1:x0 against x2:x1 (one shared dword)", cmp64(true), small.clone()),
        ("xcl-i64-compare-disjoint", "control: the same compares of x1:x0 against x3:x2", cmp64(false), small.clone()),
    ];
    for (id, about, p, mem) in cases {
        let c = emit(&p);
        let want = interpret(&p, &mem);
        let out_dwords = want.iter().map(|w| w.0 + 1).max().unwrap();
        std::fs::write(dir.join(format!("{id}.dxil")), &c).unwrap();
        let found = if id.contains("540") || id.contains("sext") {
            "fails on RDNA2"
        } else if id.contains("722") || id.contains("1599") {
            "fails on RDNA4"
        } else {
            "shape of Marvisak/VIctory#1599"
        };
        let mut t = format!("id {id}\nabout {about}\norigin xcl-dev/xcl direct-DXIL emitter (gpu/dxil/fuzz_tests.rs), {found}\nthreads 32\nwave 32\nregs u1=out u2=in\nout_dwords {out_dwords}\nin");
        for v in &mem {
            t.push_str(&format!(" {v:08x}"));
        }
        t.push_str("\nexpect");
        for (i, v) in &want {
            t.push_str(&format!(" {i}:{v:08x}"));
        }
        t.push('\n');
        std::fs::write(dir.join(format!("{id}.txt")), t).unwrap();
        let r = runner::run_with(&c, &runner::KernelState::default(), &mem, &[], &[]).unwrap();
        let bad = want.iter().filter(|(i, v)| r.raw[*i] != *v).count();
        eprintln!("{id}: {} bytes, {bad} mismatching dwords here ({:?})", c.len(), runner::adapter());
    }
}
