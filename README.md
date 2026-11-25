# rdna2-conformance

Driver probes for console RDNA2 → DXIL shader translators: [VIctory](https://github.com/Marvisak/VIctory) and its forks, [XCL](https://github.com/xcl-dev/xcl) and anything else that recompiles console GPU code for D3D12.

These translators lean on the PC driver to run their DXIL the way the console runs its own code, and some AMD driver versions don't. Each probe is one such behaviour, run on every D3D12 adapter in the machine and checked against a CPU reference. The result is a per-GPU matrix of which bugs a translator has to work around.

## The main finding so far

The same operation passes when it is written in HLSL and compiled by DXC, and fails when it is the DXIL a translator emits:

| behaviour | HLSL (DXC) | translator's DXIL | fails on |
|---|---|---|---|
| `sext i1 → i64` keeps the high dword 0 | pass everywhere | `xcl-sext-i1-to-i64`, `xcl-seed-540` | RDNA2 (gfx1036, 32.0.21045.29003) |
| `WaveReadLaneFirst` read in a divergent `switch` case | pass everywhere | `xcl-seed-722` | RDNA4 (RX 9070, 32.0.32029.2010) |
| `Countbits.i64` result shifted by the lane, or'd with its `Bfrev` | – | `xcl-seed-1599` | RDNA4 |
| ordered i64 compare (`lt`/`gt`, signed or not) of values sharing a dword | pass everywhere | `victory-v-cmp-*-shared-dword` (VIctory before [#1599](https://github.com/Marvisak/VIctory/pull/1599)); XCL's IR shape passes | RDNA2 and RDNA4 here; RDNA3 in #1599 |

So a driver bug is a property of the DXIL's exact shape, and a translator has to test the DXIL it writes. That is why this kit runs **DXIL vectors**: a translator's own containers with their inputs and expected outputs. It also runs HLSL probes, which catch what DXC itself does (fast-math, precision).

Two HLSL probes do show differences:
- **`sin-cos-in-turns`:** `sin(x·2π)` / `cos(x·2π)`, as translators lower `v_sin_f32` / `v_cos_f32`, give different results on AMD for out-of-range inputs depending on whether DXC builds with its default fast-math or with `-Gis`, e.g. `0x00000000` vs NaN for `sin(FLT_MAX·2π)`. WARP gives the same either way.
- **`dot2add-f16-rounding`:** on the RX 9070, 3 of 64 results are not correctly rounded (1 ulp), and on WARP 7 of 64 (up to 2 ulp). The RDNA2 iGPU rounds all of them correctly. Precision here is implementation-defined, so this is reported as info.

## Results

[`results/2026-11-25-rx9070-gfx1036-warp.md`](results/2026-11-25-rx9070-gfx1036-warp.md): one machine with an RX 9070 (RDNA4) and a Ryzen 7 7700 iGPU (gfx1036, RDNA2, the console's architecture), plus WARP.

| probe | kind | RX 9070 | gfx1036 | WARP |
|---|---|---|---|---|
| `i64-compare-shared-dword` | HLSL | pass | pass | pass |
| `i64-compare-disjoint` | HLSL | pass | pass | pass |
| `sext-bool-to-i64` | HLSL | pass | pass | pass |
| `f32-mul-negative-zero` | HLSL | pass | pass | pass |
| `f16-mul-negative-zero` | HLSL | pass | pass | pass |
| `sin-cos-in-turns` | HLSL | info | info | pass |
| `wave-read-first-in-switch` | HLSL | pass | pass | pass |
| `dot2add-f16-rounding` | HLSL | info | pass | info |
| `victory-v-cmp-eq-i64-shared-dword` | DXIL | pass | pass | pass |
| `victory-v-cmp-gt-i64-shared-dword` | DXIL | **FAIL** | **FAIL** | pass |
| `victory-v-cmp-gt-u64-shared-dword` | DXIL | **FAIL** | **FAIL** | pass |
| `victory-v-cmp-lt-i64-shared-dword` | DXIL | **FAIL** | **FAIL** | pass |
| `victory-v-cmp-lt-u64-shared-dword` | DXIL | **FAIL** | **FAIL** | pass |
| `xcl-i64-compare-disjoint` | DXIL | pass | pass | skip |
| `xcl-i64-compare-shared-dword` | DXIL | pass | pass | skip |
| `xcl-seed-1599` | DXIL | **FAIL** | pass | skip |
| `xcl-seed-540` | DXIL | pass | **FAIL** | skip |
| `xcl-seed-722` | DXIL | **FAIL** | pass | skip |
| `xcl-sext-i1-to-i64` | DXIL | pass | **FAIL** | skip |

WARP skips the `xcl-*` vectors because they need 32-lane waves. The `victory-*` ones run there, which also makes them a CI check.

**More GPUs make this useful:** NVIDIA, Intel, other AMD generations and other drivers. Run it and add your report (see below).

## Run it

Windows 10/11, a D3D12 GPU with shader model 6.6, Rust, and DXC: the Windows SDK's `dxc.exe` is found by itself, or set `DXC` to a `dxc.exe`.

```powershell
cargo run --release                                      # every probe, every adapter
cargo run --release -- --out results\<date>-<gpus>.md     # also save the report
cargo run --release -- --only xcl-seed-540,sin-cos-in-turns
cargo run --release -- --dxil sext-bool-to-i64             # the DXIL DXC makes of an HLSL probe
```

No game files are involved. The probes are synthetic.

## Add your results

Run `cargo run --release -- --out results/<date>-<your gpus>.md`, then open a pull request with that file, or paste it into an issue. The report names the adapters, vendor and device ids, and driver versions.

## Add a vector

A vector is `vectors/<id>.dxil` (a DXIL container without a root signature) plus `vectors/<id>.txt` in one of two layouts.

XCL's kernels (`layout root-uavs`, the default) use root UAVs `u0..u2` and one dispatch:

```text
id <id>
about <what the right result is>
origin <where it was found>
regs u2=in u1=out          # which root UAV is the input, which the output
wave 32                    # optional: the wave size it needs
out_dwords <n>
in <hex dword> ...
expect <index>:<hex dword> ...
```

VIctory's one-thread kernels (`layout victory`) take user data in root constants `b0` and write a raw UAV at index 0 of a directly indexed heap, with one dispatch per case:

```text
id <id>
layout victory
ud_dwords 8                # root constants
ud_inputs 0:0 1:1 2:2 3:3  # user-data dword : which of the case's 4 inputs
heap_raw_uav_dwords 1
case <in0> <in1> <in2> <in3> <expected dword 0>    # hex, one line per dispatch
```

The `xcl-*` vectors were written by XCL's direct-DXIL emitter from small programs in its fuzz IR ([`tools/xcl_export.rs`](tools/xcl_export.rs), which goes into XCL's `gpu/dxil/fuzz_tests.rs`). The `victory-*` vectors come from VIctory's lowering of single `v_cmp_*_i64` instructions, before #1599's fix ([`tools/victory_export.rs`](tools/victory_export.rs), which goes into the fuzzer file of VIctory#1614).

## Where these came from

- [Marvisak/VIctory#1599](https://github.com/Marvisak/VIctory/pull/1599): AMD orders a shared-dword i64 compare wrong (RDNA2, RDNA3, RDNA4).
- [Marvisak/VIctory#1614](https://github.com/Marvisak/VIctory/pull/1614): the cross-device VALU fuzzer that found it.
- [xcl-dev/xcl#42](https://github.com/xcl-dev/xcl/issues/42#issuecomment-7153665094): XCL's suite on RDNA4 and RDNA2, where seeds 540 / 722 / 1599 split by GPU generation.

## License

MIT (see `LICENSE`). The `.dxil` files in `vectors/` are translator output: `xcl-*` from XCL's emitter (GPL-2.0-only), for the programs in `tools/xcl_export.rs`; `victory-*` from VIctory's lowering (GPL-3.0), for the instructions in `tools/victory_export.rs`. The `tools/*.rs` snippets are meant to be pasted into those projects' test files; they are offered under MIT, and they fall under each project's license once added there.

Written with Claude Code (Claude Opus 5.5). Every result in this README comes from a run on the hardware named.
