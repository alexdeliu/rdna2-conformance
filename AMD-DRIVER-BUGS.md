# AMD D3D12 shader compiler: miscompiles found with this kit

Each bug below is a **validator-passing DXIL container** that runs right on WARP and gives wrong results on AMD hardware. Each one comes with its inputs and the expected outputs, computed on the CPU.

They were found while testing GPU translators for Xbox Series (RDNA2) code: VIctory, Belobug's VIctory fork and XCL. Each translator works around them.

Machine: Windows 11 with:
- an RX 9070 (RDNA4, driver 32.0.32029.2010);
- a Ryzen 7 7700 iGPU (gfx1036, RDNA2, driver 32.0.21045.29003);
- WARP.

A second RX 9070 XT gives the same results ([#1](https://github.com/alexdeliu/rdna2-conformance/issues/1)), and RDNA3 (RX 7900 XT) reproduces bug 1 ([VIctory#1599](https://github.com/Marvisak/VIctory/pull/1599)).

Reproduce: `cargo run --release` runs every probe and vector on every adapter; `--out report.md` saves the matrix.

| # | what goes wrong | vectors | RDNA4 | RDNA2 | WARP |
|---|---|---|---|---|---|
| 1 | Ordered `icmp` (`slt`/`ult`/`sgt`/`ugt`) of two i64 values built from dwords where one dword is the high half of one operand and the low half of the other: ordered as if each operand's dwords were swapped. `eq` is right. | `victory-v-cmp-{lt,gt}-{i64,u64}-shared-dword` | wrong | wrong | right |
| 2 | f64 `fadd`/`fmul` of two doubles made with `makeDouble` from dwords that one source shares with the other (`v[1:2]`, `v[2:3]`): computed as if both were the first pair (`1.5 + x` gives `3.0`). Disjoint pairs are right. | `belobug-v-{add,mul}-f64-shared-dword-{lo,hi}`, `belobug-direct-*`; from plain HLSL: `hlsl-cbuffer-v-add-f64-shared-dword-lo-{fast,gis}` ([source](vectors/hlsl-cbuffer-f64-shared-dword.hlsl)); controls `belobug-v-*-f64-disjoint-*`, `hlsl-cbuffer-v-add-f64-disjoint-lo-*` | wrong (both shapes) | wrong with `makeDouble` of the dwords; right through i64 | right |
| 3 | `sext i1 → i64` leaves the high dword 0 instead of all ones. | `xcl-sext-i1-to-i64`, `xcl-seed-540` | right | wrong | – (needs 32 lanes) |
| 4 | `WaveReadLaneFirst` inside a divergent `switch` case returns another lane's value. | `xcl-seed-722` | wrong | right | – |
| 5 | `Bfrev(Countbits(0x80000000))` (a constant fold) gives `0x80000001`; `Bfrev` of a runtime value is right. | `xcl-seed-1599` | wrong | right | – |

**The same operations written in HLSL and compiled by DXC pass everywhere, with one exception.** These are the probes `i64-compare-shared-dword`, `sext-bool-to-i64`, `f64-shared-dword` and `wave-read-first-in-switch`. The exception is bug 2: plain HLSL reproduces it when the doubles' dwords come from a constant buffer (`hlsl-cbuffer-*`, with or without `-Gis`). The same add with the dwords from a UAV passes, whether they are per lane (`f64-shared-dword`) or uniform (`f64-shared-dword-uniform`), and so does the same add with fast-math off (`*-gis`). Each bug needs the exact DXIL shape a translator emits; that shape is valid DXIL, and the validator accepts it. So the bugs are in the driver's compiler, not in the HLSL front end.

Each vector's `.txt` names its instruction, origin, root layout, inputs and expected outputs. The `.dxil` files are translator output, licensed under their projects' licenses (see the README).

**Reported to AMD** on 2026-11-25, with all five bugs re-checked on the drivers above that day: [AMD Developer Community, Drivers](https://devcommunity.amd.com/t/d3d12-shader-compiler-5-miscompiles-of-validator-passing-dxil-on-rx-9070-rdna4-and-rdna2-with-repro-vectors/1518). Results on other GPUs and drivers are welcome there or in this repository's issues.
