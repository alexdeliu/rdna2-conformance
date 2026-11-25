// How the `belobug-*` vectors were made. Paste this at the end of the fuzzer file of
// Marvisak/VIctory#1614 (`fuzz_tests.rs`, which has `build` and `Rng`) added to
// Belobug/VIctory-Belobug-AMD 71652a2, then run
//
//   $env:TEMP_OUT = "<this repo>/vectors"
//   cargo test -p victory-shader --lib export_f64_vectors -- --ignored --nocapture
//
// TAG=-direct names the vectors made with src_f64 passing the dwords to makeDouble directly.
#[test]
#[ignore = "export"]
fn export_f64_vectors() {
    let dir = std::path::PathBuf::from(std::env::var("TEMP_OUT").expect("TEMP_OUT"));
    let tag = std::env::var("TAG").unwrap_or_default();
    std::fs::create_dir_all(&dir).unwrap();
    let d = |lo: u32, hi: u32| f64::from_bits(u64::from(lo) | u64::from(hi) << 32);
    let mut cases: Vec<[u32; 4]> = vec![
        [0x0000_0000, 0x3FF8_0000, 0x0000_0000, 0x4002_0000],
        [0x0000_0000, 0x3FF0_0000, 0x3FF0_0000, 0x4000_0000],
        [0x0000_0000, 0x4000_0000, 0x4000_0000, 0x4008_0000],
        [0x0000_0000, 0x3EFF_FFFF, 0x7F80_0001, 0x3F80_0000],
        [0x4F00_0000, 0x0000_001F, 0x37A7_7C00, 0x0000_0000],
        [0xDEAD_BEEF, 0xAFB5_28F6, 0x1400_BC00, 0x0000_0000],
    ];
    let mut rng = Rng(0xF64);
    while cases.len() < 32 {
        // Normal doubles: exponents near 1.0 so no case is a denormal.
        let a = rng.next();
        let b = 0x3F00_0000 | (rng.next() & 0x00FF_FFFF);
        let c = 0x3F00_0000 | (rng.next() & 0x00FF_FFFF);
        let e = 0x3F00_0000 | (rng.next() & 0x00FF_FFFF);
        cases.push([a, b, c, e]);
    }
    for (opname, op) in [("add", 0x164u32), ("mul", 0x165)] {
        for (shape, src1) in [("shared-dword", 0x102u32), ("disjoint", 0x103)] {
            for (half, extra) in [("lo", None), ("hi", Some(0x7E00_0301u32))] {
                let mut words = vec![0xD400_0000 | op << 16, 0x101 | src1 << 9];
                words.extend(extra);
                let shader = build(&words).expect("build");
                let map: Vec<String> = shader
                    .user_data
                    .iter()
                    .filter(|u| matches!((u.root, u.slot), (RootType::Constants, 0)))
                    .map(|u| format!("{}:{}", u.index, u.dword))
                    .collect();
                let id = format!("belobug{tag}-v-{opname}-f64-{shape}-{half}");
                let (s1, ex) = if src1 == 0x102 { ("v[2:3] (shares v2 with src0)", "") } else { ("v[3:4]", "") };
                let _ = ex;
                let mut t = format!(
                    "id {id}\nabout v_{opname}_f64 v[0:1], v[1:2], {s1}; stores the result's {half} dword\norigin Belobug/VIctory-Belobug-AMD 71652a2 lowering{}\nlayout victory\nud_dwords {}\nud_inputs {}\nheap_raw_uav_dwords 1\n",
                    if tag.is_empty() { " (i64 join/split, then makeDouble)" } else { " with the dwords passed to makeDouble directly" },
                    shader.user_data_dwords().max(1),
                    map.join(" ")
                );
                for c in &cases {
                    let a = d(c[0], c[1]);
                    let b = if src1 == 0x102 { d(c[1], c[2]) } else { d(c[2], c[3]) };
                    let r = if op == 0x164 { a + b } else { a * b }.to_bits();
                    let want = if half == "lo" { r as u32 } else { (r >> 32) as u32 };
                    t.push_str(&format!("case {:08x} {:08x} {:08x} {:08x} {:08x}\n", c[0], c[1], c[2], c[3], want));
                }
                std::fs::write(dir.join(format!("{id}.dxil")), &shader.dxil).unwrap();
                std::fs::write(dir.join(format!("{id}.txt")), t).unwrap();
                eprintln!("EXPORT {id}");
            }
        }
    }
}
