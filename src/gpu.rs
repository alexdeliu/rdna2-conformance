//! A minimal D3D12 compute runner: one device per adapter, a compute shader
//! with an embedded root signature (`UAV(u0), UAV(u1)`: input, output), one
//! dispatch, and the output read back.

use std::mem::ManuallyDrop;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::Interface;

/// An adapter the probes can run on.
#[derive(Clone, Debug)]
pub struct AdapterInfo {
    /// DXGI index, or `None` for WARP.
    pub index: Option<u32>,
    pub name: String,
    pub vendor: u32,
    pub device: u32,
    pub driver: String,
}

fn err(e: windows::core::Error) -> String {
    format!("{e}")
}

/// The hardware adapters (not software, with memory of their own), then WARP.
pub fn adapters() -> Vec<AdapterInfo> {
    let mut out = Vec::new();
    // SAFETY: plain DXGI enumeration with valid out pointers.
    unsafe {
        let Ok(factory) = CreateDXGIFactory2::<IDXGIFactory4>(DXGI_CREATE_FACTORY_FLAGS(0)) else {
            return out;
        };
        let mut i = 0;
        while let Ok(a) = factory.EnumAdapters1(i) {
            if let Ok(d) = a.GetDesc1()
                && d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0
                && d.DedicatedVideoMemory > 0
            {
                out.push(AdapterInfo {
                    index: Some(i),
                    name: name(&d.Description),
                    vendor: d.VendorId,
                    device: d.DeviceId,
                    driver: driver_version(&a).unwrap_or_else(|| "unknown".into()),
                });
            }
            i += 1;
        }
        if let Ok(w) = factory.EnumWarpAdapter::<IDXGIAdapter1>()
            && let Ok(d) = w.GetDesc1()
        {
            out.push(AdapterInfo {
                index: None,
                name: "WARP".into(),
                vendor: d.VendorId,
                device: d.DeviceId,
                driver: driver_version(&w).unwrap_or_else(|| "unknown".into()),
            });
        }
    }
    out
}

fn name(desc: &[u16]) -> String {
    let n = desc.iter().take_while(|&&c| c != 0).count();
    String::from_utf16_lossy(&desc[..n])
}

/// The user-mode driver version (`a.b.c.d`), as Device Manager shows it.
fn driver_version(a: &IDXGIAdapter1) -> Option<String> {
    // SAFETY: CheckInterfaceSupport only writes the version.
    let v = unsafe { a.CheckInterfaceSupport(&IDXGIDevice::IID) }.ok()?;
    let v = v as u64;
    Some(format!(
        "{}.{}.{}.{}",
        v >> 48,
        (v >> 32) & 0xFFFF,
        (v >> 16) & 0xFFFF,
        v & 0xFFFF
    ))
}

pub struct Gpu {
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
}

impl Gpu {
    pub fn new(info: &AdapterInfo) -> Result<Gpu, String> {
        // SAFETY: plain D3D12/DXGI calls with valid out pointers.
        unsafe {
            let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).map_err(err)?;
            let adapter: IDXGIAdapter1 = match info.index {
                Some(i) => factory.EnumAdapters1(i).map_err(err)?,
                None => factory.EnumWarpAdapter().map_err(err)?,
            };
            let mut device: Option<ID3D12Device> = None;
            D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_12_0, &mut device).map_err(err)?;
            let device = device.ok_or("no device")?;
            let queue: ID3D12CommandQueue = device
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_COMPUTE,
                    ..Default::default()
                })
                .map_err(err)?;
            Ok(Gpu { device, queue })
        }
    }

    /// The highest shader model the device runs (e.g. 0x66).
    pub fn shader_model(&self) -> u32 {
        let mut sm = D3D12_FEATURE_DATA_SHADER_MODEL {
            HighestShaderModel: D3D_SHADER_MODEL_6_6,
        };
        // SAFETY: the struct matches the feature queried.
        let ok = unsafe {
            self.device.CheckFeatureSupport(
                D3D12_FEATURE_SHADER_MODEL,
                (&raw mut sm).cast(),
                size_of_val(&sm) as u32,
            )
        };
        if ok.is_ok() { sm.HighestShaderModel.0 as u32 } else { 0 }
    }

    /// Whether the device runs 16-bit shader types.
    pub fn native_16bit(&self) -> bool {
        let mut o4 = D3D12_FEATURE_DATA_D3D12_OPTIONS4::default();
        // SAFETY: the struct matches the feature queried.
        let ok = unsafe {
            self.device.CheckFeatureSupport(
                D3D12_FEATURE_D3D12_OPTIONS4,
                (&raw mut o4).cast(),
                size_of_val(&o4) as u32,
            )
        };
        ok.is_ok() && o4.Native16BitShaderOpsSupported.as_bool()
    }

    /// The device's (min, max) wave lane counts.
    pub fn wave_lanes(&self) -> (u32, u32) {
        let mut o1 = D3D12_FEATURE_DATA_D3D12_OPTIONS1::default();
        // SAFETY: the struct matches the feature queried.
        let ok = unsafe {
            self.device.CheckFeatureSupport(
                D3D12_FEATURE_D3D12_OPTIONS1,
                (&raw mut o1).cast(),
                size_of_val(&o1) as u32,
            )
        };
        if ok.is_ok() {
            (o1.WaveLaneCountMin, o1.WaveLaneCountMax)
        } else {
            (0, 0)
        }
    }

    /// Runs `dxil` (one group of `threads`) on `input`; returns `out_dwords` of output.
    /// The shader carries its own root signature: `UAV(u0)` input, `UAV(u1)` output.
    pub fn run(&self, dxil: &[u8], threads_groups: u32, input: &[u32], out_dwords: usize) -> Result<Vec<u32>, String> {
        // SAFETY: the blob is a DXIL container with an embedded root signature.
        let rs: ID3D12RootSignature =
            unsafe { self.device.CreateRootSignature(0, dxil) }.map_err(|e| format!("root signature: {e}"))?;
        self.dispatch(dxil, &rs, &[Slot::In, Slot::Out], threads_groups, input, out_dwords)
    }

    /// Runs a translator's `dxil`, which has no root signature: root UAVs
    /// `u0..u2`, with `input` at `in_reg` and the output at `out_reg`.
    pub fn run_raw(
        &self,
        dxil: &[u8],
        in_reg: u32,
        out_reg: u32,
        input: &[u32],
        out_dwords: usize,
    ) -> Result<Vec<u32>, String> {
        let params: Vec<D3D12_ROOT_PARAMETER> = (0..3)
            .map(|r| D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    Descriptor: D3D12_ROOT_DESCRIPTOR {
                        ShaderRegister: r,
                        RegisterSpace: 0,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            })
            .collect();
        let desc = D3D12_ROOT_SIGNATURE_DESC {
            NumParameters: params.len() as u32,
            pParameters: params.as_ptr(),
            ..Default::default()
        };
        let mut blob: Option<ID3DBlob> = None;
        let mut errs: Option<ID3DBlob> = None;
        // SAFETY: valid descriptors; the blob's bytes are read while it lives.
        let rs: ID3D12RootSignature = unsafe {
            D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, Some(&mut errs))
                .map_err(|e| format!("serialize root signature: {e}"))?;
            let blob = blob.ok_or("no root signature blob")?;
            let bytes = std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize());
            self.device
                .CreateRootSignature(0, bytes)
                .map_err(|e| format!("root signature: {e}"))?
        };
        let slots: Vec<Slot> = (0..3)
            .map(|r| {
                if r == in_reg {
                    Slot::In
                } else if r == out_reg {
                    Slot::Out
                } else {
                    Slot::Dummy
                }
            })
            .collect();
        self.dispatch(dxil, &rs, &slots, 1, input, out_dwords)
    }

    /// Runs VIctory-style `dxil` once per case: user data in root constants
    /// (`b0`, `ud_dwords` of them, the case's four inputs at `inputs`' (index,
    /// input) pairs), the result in a raw UAV at heap index 0 of a directly
    /// indexed heap. Returns each case's dword 0.
    pub fn run_victory(
        &self,
        dxil: &[u8],
        ud_dwords: u32,
        inputs: &[(usize, usize)],
        uav_dwords: u32,
        cases: &[[u32; 4]],
    ) -> Result<Vec<u32>, String> {
        let param = D3D12_ROOT_PARAMETER1 {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER1_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                    Num32BitValues: ud_dwords,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        };
        let desc = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
            Version: D3D_ROOT_SIGNATURE_VERSION_1_1,
            Anonymous: D3D12_VERSIONED_ROOT_SIGNATURE_DESC_0 {
                Desc_1_1: D3D12_ROOT_SIGNATURE_DESC1 {
                    NumParameters: 1,
                    pParameters: &param,
                    Flags: D3D12_ROOT_SIGNATURE_FLAG_CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED,
                    ..Default::default()
                },
            },
        };
        // SAFETY: D3D12 objects are used on this thread only; each case's GPU
        // work is waited for before its read-back, and every resource outlives
        // the command list that uses it.
        unsafe {
            let d = &self.device;
            let mut blob: Option<ID3DBlob> = None;
            D3D12SerializeVersionedRootSignature(&desc, &mut blob, None)
                .map_err(|e| format!("serialize root signature: {e}"))?;
            let blob = blob.ok_or("no root signature blob")?;
            let bytes = std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize());
            let rs: ID3D12RootSignature = d
                .CreateRootSignature(0, bytes)
                .map_err(|e| format!("root signature: {e}"))?;
            let pso: ID3D12PipelineState = d
                .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: ManuallyDrop::new(Some(rs.clone())),
                    CS: D3D12_SHADER_BYTECODE {
                        pShaderBytecode: dxil.as_ptr().cast(),
                        BytecodeLength: dxil.len(),
                    },
                    ..Default::default()
                })
                .map_err(|e| format!("pipeline: {e}"))?;
            let bytes = u64::from(uav_dwords.max(1)) * 4;
            let out = buffer(
                d,
                bytes.max(256),
                D3D12_HEAP_TYPE_DEFAULT,
                D3D12_RESOURCE_STATE_COMMON,
                true,
            )?;
            let readback = buffer(d, 256, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST, false)?;
            let heap: ID3D12DescriptorHeap = d
                .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: 1,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })
                .map_err(err)?;
            d.CreateUnorderedAccessView(
                &out,
                None,
                Some(&D3D12_UNORDERED_ACCESS_VIEW_DESC {
                    Format: DXGI_FORMAT_R32_TYPELESS,
                    ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
                    Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                        Buffer: D3D12_BUFFER_UAV {
                            FirstElement: 0,
                            NumElements: uav_dwords.max(1),
                            StructureByteStride: 0,
                            CounterOffsetInBytes: 0,
                            Flags: D3D12_BUFFER_UAV_FLAG_RAW,
                        },
                    },
                }),
                heap.GetCPUDescriptorHandleForHeapStart(),
            );
            let alloc: ID3D12CommandAllocator =
                d.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE).map_err(err)?;
            let fence: ID3D12Fence = d.CreateFence(0, D3D12_FENCE_FLAG_NONE).map_err(err)?;
            let ev: HANDLE = CreateEventW(None, false, false, None).map_err(err)?;
            let mut results = Vec::with_capacity(cases.len());
            let mut first = true;
            for (n, case) in cases.iter().enumerate() {
                let mut ud = vec![0u32; ud_dwords as usize];
                for &(index, input) in inputs {
                    ud[index] = case[input];
                }
                alloc.Reset().map_err(err)?;
                let list: ID3D12GraphicsCommandList = d
                    .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COMPUTE, &alloc, &pso)
                    .map_err(err)?;
                if first {
                    list.ResourceBarrier(&[transition(
                        &out,
                        D3D12_RESOURCE_STATE_COMMON,
                        D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    )]);
                    first = false;
                }
                // A directly indexed heap is set before the root signature.
                list.SetDescriptorHeaps(&[Some(heap.clone())]);
                list.SetComputeRootSignature(&rs);
                list.SetComputeRoot32BitConstants(0, ud_dwords, ud.as_ptr().cast(), 0);
                list.Dispatch(1, 1, 1);
                list.ResourceBarrier(&[transition(
                    &out,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                )]);
                list.CopyBufferRegion(&readback, 0, &out, 0, 4);
                list.ResourceBarrier(&[transition(
                    &out,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                )]);
                list.Close().map_err(err)?;
                self.queue.ExecuteCommandLists(&[Some(list.cast().map_err(err)?)]);
                let v = n as u64 + 1;
                self.queue.Signal(&fence, v).map_err(err)?;
                fence.SetEventOnCompletion(v, ev).map_err(err)?;
                WaitForSingleObject(ev, INFINITE);
                let removed = d.GetDeviceRemovedReason();
                if removed.is_err() {
                    let _ = CloseHandle(ev);
                    return Err(format!("device removed: {removed:?}"));
                }
                let mut p = std::ptr::null_mut();
                readback.Map(0, None, Some(&mut p)).map_err(err)?;
                results.push(*p.cast::<u32>());
                readback.Unmap(0, None);
            }
            let _ = CloseHandle(ev);
            Ok(results)
        }
    }

    fn dispatch(
        &self,
        dxil: &[u8],
        rs: &ID3D12RootSignature,
        slots: &[Slot],
        threads_groups: u32,
        input: &[u32],
        out_dwords: usize,
    ) -> Result<Vec<u32>, String> {
        // SAFETY: D3D12 objects are created and used on this thread only, every
        // buffer outlives the command list that uses it, and the fence wait
        // finishes the GPU work before the read-back.
        unsafe {
            let d = &self.device;
            let pso: ID3D12PipelineState = d
                .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: ManuallyDrop::new(Some(rs.clone())),
                    CS: D3D12_SHADER_BYTECODE {
                        pShaderBytecode: dxil.as_ptr().cast(),
                        BytecodeLength: dxil.len(),
                    },
                    ..Default::default()
                })
                .map_err(|e| format!("pipeline: {e}"))?;
            let in_bytes = (input.len() * 4).max(256) as u64;
            let out_bytes = (out_dwords * 4).max(256) as u64;
            let upload = buffer(
                d,
                in_bytes,
                D3D12_HEAP_TYPE_UPLOAD,
                D3D12_RESOURCE_STATE_GENERIC_READ,
                false,
            )?;
            let inbuf = buffer(d, in_bytes, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COMMON, true)?;
            let outbuf = buffer(d, out_bytes, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COMMON, true)?;
            let readback = buffer(
                d,
                out_bytes,
                D3D12_HEAP_TYPE_READBACK,
                D3D12_RESOURCE_STATE_COPY_DEST,
                false,
            )?;
            let mut p = std::ptr::null_mut();
            upload.Map(0, None, Some(&mut p)).map_err(err)?;
            std::ptr::copy_nonoverlapping(input.as_ptr(), p.cast::<u32>(), input.len());
            upload.Unmap(0, None);

            let alloc: ID3D12CommandAllocator =
                d.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE).map_err(err)?;
            let list: ID3D12GraphicsCommandList = d
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COMPUTE, &alloc, &pso)
                .map_err(err)?;
            list.ResourceBarrier(&[transition(
                &inbuf,
                D3D12_RESOURCE_STATE_COMMON,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )]);
            list.CopyBufferRegion(&inbuf, 0, &upload, 0, (input.len() * 4) as u64);
            list.ResourceBarrier(&[
                transition(
                    &inbuf,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                ),
                transition(
                    &outbuf,
                    D3D12_RESOURCE_STATE_COMMON,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                ),
            ]);
            let dummy = buffer(d, 256, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COMMON, true)?;
            list.ResourceBarrier(&[transition(
                &dummy,
                D3D12_RESOURCE_STATE_COMMON,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            )]);
            list.SetComputeRootSignature(rs);
            for (i, s) in slots.iter().enumerate() {
                let b = match s {
                    Slot::In => &inbuf,
                    Slot::Out => &outbuf,
                    Slot::Dummy => &dummy,
                };
                list.SetComputeRootUnorderedAccessView(i as u32, b.GetGPUVirtualAddress());
            }
            list.Dispatch(threads_groups, 1, 1);
            list.ResourceBarrier(&[transition(
                &outbuf,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            )]);
            list.CopyBufferRegion(&readback, 0, &outbuf, 0, (out_dwords * 4) as u64);
            list.Close().map_err(err)?;
            self.queue.ExecuteCommandLists(&[Some(list.cast().map_err(err)?)]);
            let fence: ID3D12Fence = d.CreateFence(0, D3D12_FENCE_FLAG_NONE).map_err(err)?;
            self.queue.Signal(&fence, 1).map_err(err)?;
            let ev: HANDLE = CreateEventW(None, false, false, None).map_err(err)?;
            fence.SetEventOnCompletion(1, ev).map_err(err)?;
            WaitForSingleObject(ev, INFINITE);
            let _ = CloseHandle(ev);
            let removed = d.GetDeviceRemovedReason();
            if removed.is_err() {
                return Err(format!("device removed: {removed:?}"));
            }
            let mut p = std::ptr::null_mut();
            readback.Map(0, None, Some(&mut p)).map_err(err)?;
            let out = std::slice::from_raw_parts(p.cast::<u32>(), out_dwords).to_vec();
            readback.Unmap(0, None);
            Ok(out)
        }
    }
}

/// What a root UAV parameter is bound to.
enum Slot {
    In,
    Out,
    Dummy,
}

fn buffer(
    d: &ID3D12Device,
    size: u64,
    heap: D3D12_HEAP_TYPE,
    state: D3D12_RESOURCE_STATES,
    uav: bool,
) -> Result<ID3D12Resource, String> {
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: size,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: if uav {
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS
        } else {
            D3D12_RESOURCE_FLAG_NONE
        },
        ..Default::default()
    };
    let mut r: Option<ID3D12Resource> = None;
    // SAFETY: valid descriptors and out pointer.
    unsafe {
        d.CreateCommittedResource(
            &D3D12_HEAP_PROPERTIES {
                Type: heap,
                ..Default::default()
            },
            D3D12_HEAP_FLAG_NONE,
            &desc,
            state,
            None,
            &mut r,
        )
        .map_err(err)?;
    }
    r.ok_or_else(|| "buffer".into())
}

fn transition(
    r: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                // SAFETY: a borrowed COM pointer, not released by the barrier.
                pResource: unsafe { std::mem::transmute_copy(r) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}
