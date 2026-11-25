# Driver Experiments (Radeon Developer Panel), 2026-11-25

Asked for by AMD in https://devcommunity.amd.com/t/1518. Run with `RadeonDeveloperPanelCLI` from Radeon Developer Tool Suite 2026-10-02-1872, filtered to this runner's process. The CLI log shows each override sent ("Successfully sent 1 user overrides"). Drivers: 32.0.32029.2010 (RX 9070) and 32.0.21045.29003 (Ryzen 7 7700 iGPU).

| run | RX 9070 (RDNA4) | iGPU (RDNA2) |
|---|---|---|
| default | bugs 1, 2, 4, 5 | bugs 1, 2, 3 |
| `dx12/Disable shader compiler optimizations=true` | same vectors fail, same counts | same |
| `dx12/Disable floating-point optimizations=true` | same | same |

Control, showing the overrides reach both GPUs: `dx12/Compute shader wave size` flips a wave-width-dependent test (Belobug's `scalar_counted_in_a_divergent_loop_read_after_it` without its fix).
- iGPU: fails by default, passes with `=2` (force 64).
- RX 9070: passes by default, fails with `=3` (force 32).

Narrowing bug 2 the same day (`2026-11-25-f64-cbuffer.md`):
- The f64 add of two doubles that share a dword fails from plain HLSL too, when the dwords come from a constant buffer (`hlsl-cbuffer-*`), on both GPUs, with DXC's default build and with `-Gis`.
- From a UAV it passes, per lane or uniform.
