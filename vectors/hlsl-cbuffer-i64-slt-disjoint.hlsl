cbuffer UD : register(b0) { uint4 v; uint4 w; };
[RootSignature("RootFlags(CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED), RootConstants(num32BitConstants=8, b0)")]
[numthreads(1, 1, 1)]
void main() {
    RWByteAddressBuffer o = ResourceDescriptorHeap[w.x >> 5];
    int64_t a = (int64_t(v.y) << 32) | v.x;
    int64_t b = (int64_t(v.w) << 32) | v.z;
    o.Store(0, a < b ? 1u : 0u);
}
