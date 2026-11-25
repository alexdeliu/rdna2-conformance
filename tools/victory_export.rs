// How the `victory-*` vectors were made. Paste this at the end of
// Marvisak/VIctory's `crates/victory-shader/src/emit/fuzz_tests.rs` (from PR
// #1614, which has `body` and `build`), on a tree without #1599's fix, and run
//
//   $env:TEMP_OUT = "<this repo>/vectors"
//   cargo test -p victory-shader --lib export_victory_vectors -- --ignored --nocapture
//
// Made on #1614's branch merged with VIctory main 940d32f.
#[test]
#[ignore]
fn export_victory_vectors() {
    let dir = std::path::PathBuf::from(std::env::var("TEMP_OUT").expect("TEMP_OUT"));
    std::fs::create_dir_all(&dir).unwrap();
    // (opcode, name, signed, kind): kind 0 lt, 1 gt, 2 eq.
    let ops = [(0x0a1u16, "v_cmp_lt_i64", true, 0), (0x0e1, "v_cmp_lt_u64", false, 0), (0x0a4, "v_cmp_gt_i64", true, 1), (0x0e4, "v_cmp_gt_u64", false, 1), (0x0a2, "v_cmp_eq_i64", true, 2)];
    let mut cases: Vec<[u32; 4]> = vec![
        [0, 0, 1, 0], [0, 1, 0, 0], [1, 0, 0, 0], [0, 0x8000_0000, 0, 0], [0xFFFF_FFFF, 0, 1, 0],
        [0, 0xFFFF_FFFF, 0x7FFF_FFFF, 0], [5, 5, 5, 0], [0x8000_0000, 0x7FFF_FFFF, 0x8000_0000, 0],
        [0, 0x0000_ffff, 0x8001_03ff, 0x7fc0_0000], [0x83ff_3c01, 0x8080_0000, 0, 0x4b80_0000],
        [0x0000_0000, 0xff7f_ffff, 0x7f80_0000, 0], [0x4b80_0000, 0x3eff_ffff, 0x9de5_03ff, 0],
    ];
    let mut rng = Rng(0x1599);
    while cases.len() < 32 {
        let mut d = || match rng.next() % 4 { 0 => rng.next() % 3, 1 => 0x8000_0000 | (rng.next() % 2), 2 => 0xFFFF_FFFF, _ => rng.next() };
        cases.push([d(), d(), d(), 0]);
    }
    for (op, name, signed, kind) in ops {
        let words = body(op);
        let shader = build(&words).expect("build");
        let ud_dwords = shader.user_data_dwords().max(1);
        let map: Vec<String> = shader
            .user_data
            .iter()
            .filter(|u| matches!((u.root, u.slot), (RootType::Constants, 0)))
            .map(|u| format!("{}:{}", u.index, u.dword))
            .collect();
        let id = format!("victory-{}-shared-dword", name.replace('_', "-"));
        std::fs::write(dir.join(format!("{id}.dxil")), &shader.dxil).unwrap();
        let mut t = format!(
            "id {id}\nabout {name} s[20:21], v[1:2], v[2:3] (src0 and src1 share v2), then v_cndmask: 1 where the compare holds\norigin Marvisak/VIctory lowering before #1599 (VIctory main 940d32f), fails on AMD RDNA2/3/4\nlayout victory\nud_dwords {ud_dwords}\nud_inputs {}\nheap_raw_uav_dwords 1\n",
            map.join(" ")
        );
        for c in &cases {
            let a = u64::from(c[1]) << 32 | u64::from(c[0]);
            let b = u64::from(c[2]) << 32 | u64::from(c[1]);
            let r = match (kind, signed) {
                (0, true) => (a as i64) < (b as i64),
                (0, false) => a < b,
                (1, true) => (a as i64) > (b as i64),
                (1, false) => a > b,
                _ => a == b,
            };
            t.push_str(&format!("case {:08x} {:08x} {:08x} {:08x} {:08x}\n", c[0], c[1], c[2], c[3], u32::from(r)));
        }
        std::fs::write(dir.join(format!("{id}.txt")), t).unwrap();
        eprintln!("{id}: {} bytes, ud {ud_dwords}, inputs {map:?}", shader.dxil.len());
    }
}
