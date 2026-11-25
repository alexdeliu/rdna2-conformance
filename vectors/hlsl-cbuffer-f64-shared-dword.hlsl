cbuffer UD : register(b0) { uint4 v; uint4 w; };
[RootSignature("RootFlags(CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED), RootConstants(num32BitConstants=8, b0)")]
[numthreads(1, 1, 1)]
void main() {
    RWByteAddressBuffer o = ResourceDescriptorHeap[w.x >> 5];
    double a = asdouble(v.x, v.y);
    double b = asdouble(v.y, v.z);
    uint lo, hi;
    asuint(a + b, lo, hi);
    o.Store(0, lo);
}
