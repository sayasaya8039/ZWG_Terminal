//! Embedded HLSL shader source for DX12 GPU terminal renderer.
//!
//! Vertex shader: generates quad positions per instance (cell) from SV_VertexID.
//! Pixel shader: samples glyph atlas alpha, composites fg over bg.

/// Vertex shader (vs_5_0)
pub const VS_SOURCE: []const u8 =
    \\cbuffer Constants : register(b0) {
    \\    float2 viewport_size;
    \\    float2 cell_size;
    \\    float2 atlas_inv_size; // 1.0 / atlas_dims
    \\    float2 _pad0;
    \\};
    \\
    \\struct CellData {
    \\    float2 pos;      // pixel position (top-left of cell)
    \\    float2 uv_origin; // atlas UV origin for this glyph
    \\    float2 uv_size;  // atlas UV extent for this glyph
    \\    float4 fg;
    \\    float4 bg;
    \\};
    \\
    \\StructuredBuffer<CellData> cells : register(t0);
    \\
    \\struct VSOut {
    \\    float4 position : SV_Position;
    \\    float2 uv       : TEXCOORD0;
    \\    float4 fg       : COLOR0;
    \\    float4 bg       : COLOR1;
    \\};
    \\
    \\// 6 vertices per quad (2 triangles): TL TR BL | TR BR BL
    \\static const float2 QUAD[6] = {
    \\    float2(0,0), float2(1,0), float2(0,1),
    \\    float2(1,0), float2(1,1), float2(0,1),
    \\};
    \\
    \\VSOut main(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    \\    VSOut o;
    \\    CellData c = cells[iid];
    \\    float2 v = QUAD[vid % 6];
    \\    float2 px = c.pos + v * cell_size;
    \\    o.position = float4(
    \\        px.x / viewport_size.x *  2.0 - 1.0,
    \\        px.y / viewport_size.y * -2.0 + 1.0,
    \\        0, 1);
    \\    o.uv = c.uv_origin + v * c.uv_size;
    \\    o.fg = c.fg;
    \\    o.bg = c.bg;
    \\    return o;
    \\}
;

/// Pixel shader (ps_5_0)
pub const PS_SOURCE: []const u8 =
    \\Texture2D<float4> atlas : register(t0);
    \\SamplerState samp : register(s0);
    \\
    \\struct PSIn {
    \\    float4 position : SV_Position;
    \\    float2 uv       : TEXCOORD0;
    \\    float4 fg       : COLOR0;
    \\    float4 bg       : COLOR1;
    \\};
    \\
    \\float4 main(PSIn i) : SV_Target {
    \\    float a = atlas.Sample(samp, i.uv).r;
    \\    return lerp(i.bg, float4(i.fg.rgb, 1), a);
    \\}
;
