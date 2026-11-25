struct UD { uint4 v; uint4 w; };
[RootSignature("RootFlags(CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED), RootConstants(num32BitConstants=8, b0)")]
[numthreads(1, 1, 1)]
void main() {
    ConstantBuffer<UD> cb = ResourceDescriptorHeap[1];
    uint4 v = cb.v;
    uint4 w = cb.w;
    RWByteAddressBuffer o = ResourceDescriptorHeap[w.x >> 5];
    uint64_t a = (uint64_t(v.y) << 32) | v.x;
    uint64_t b = (uint64_t(v.w) << 32) | v.z;
    o.Store(0, a < b ? 1u : 0u);
}
