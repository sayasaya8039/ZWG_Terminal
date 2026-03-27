//! Embedded HLSL shader source for DX12 GPU terminal renderer.
//!
//! Vertex shader: generates quad positions per instance (cell) from SV_VertexID.
//! Pixel shader: samples glyph atlas alpha, composites fg over bg.

/// Vertex shader (vs_5_0)
pub const VS_SOURCE: []const u8 =
    \\cbuffer Constants : register(b0) {
    \\    float2 viewport_size;
    \\    float2 cell_size;
    \\    float2 atlas_pitch_inv_size; // 1.0 / atlas slot pitch
    \\    float2 atlas_glyph_inv_size; // 1.0 / glyph bitmap size
    \\    float term_cols;
    \\    float atlas_grid_cols;
    \\    float2 _pad0;
    \\};
    \\
    \\struct CellData {
    \\    uint glyph_idx;
    \\    uint codepoint;
    \\    uint fg_rgba;
    \\    uint bg_rgba;
    \\    uint attrs;
    \\};
    \\
    \\StructuredBuffer<CellData> cells : register(t0);
    \\
    \\struct VSOut {
    \\    float4 position : SV_Position;
    \\    float2 uv       : TEXCOORD0;
    \\    float4 fg       : COLOR0;
    \\    float4 bg       : COLOR1;
    \\    float has_glyph : TEXCOORD1;
    \\    float2 cell_uv  : TEXCOORD2;
    \\    nointerpolation uint codepoint : TEXCOORD3;
    \\};
    \\
    \\// 6 vertices per quad (2 triangles): TL TR BL | TR BR BL
    \\static const float2 QUAD[6] = {
    \\    float2(0,0), float2(1,0), float2(0,1),
    \\    float2(1,0), float2(1,1), float2(0,1),
    \\};
    \\
    \\float4 unpack_rgba(uint c) {
    \\    return float4(
    \\        ((c >> 16) & 255) / 255.0,
    \\        ((c >> 8) & 255) / 255.0,
    \\        (c & 255) / 255.0,
    \\        ((c >> 24) & 255) / 255.0);
    \\}
    \\
    \\VSOut main(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    \\    VSOut o;
    \\    CellData c = cells[iid];
    \\    uint grid_cols = max((uint)term_cols, 1u);
    \\    float2 v = QUAD[vid % 6];
    \\    float2 cell_origin = float2((iid % grid_cols) * cell_size.x, (iid / grid_cols) * cell_size.y);
    \\    float2 px = cell_origin + v * cell_size;
    \\    uint glyph_slot = c.glyph_idx > 0 ? c.glyph_idx - 1 : 0;
    \\    uint atlas_cols = max((uint)atlas_grid_cols, 1u);
    \\    float2 uv_origin = float2(
    \\        (glyph_slot % atlas_cols) * atlas_pitch_inv_size.x,
    \\        (glyph_slot / atlas_cols) * atlas_pitch_inv_size.y);
    \\    o.position = float4(
    \\        px.x / viewport_size.x *  2.0 - 1.0,
    \\        px.y / viewport_size.y * -2.0 + 1.0,
    \\        0, 1);
    \\    o.uv = uv_origin + v * atlas_glyph_inv_size;
    \\    o.fg = unpack_rgba(c.fg_rgba);
    \\    o.bg = unpack_rgba(c.bg_rgba);
    \\    o.has_glyph = c.glyph_idx == 0 ? 0.0 : 1.0;
    \\    o.cell_uv = v;
    \\    o.codepoint = c.codepoint;
    \\    return o;
    \\}
;

/// Pixel shader (ps_5_0)
pub const PS_SOURCE: []const u8 =
    \\Texture2D<float4> atlas : register(t1);
    \\SamplerState samp : register(s0);
    \\
    \\struct PSIn {
    \\    float4 position : SV_Position;
    \\    float2 uv       : TEXCOORD0;
    \\    float4 fg       : COLOR0;
    \\    float4 bg       : COLOR1;
    \\    float has_glyph : TEXCOORD1;
    \\    float2 cell_uv  : TEXCOORD2;
    \\    nointerpolation uint codepoint : TEXCOORD3;
    \\};
    \\
    \\float rect_alpha(float2 p, float2 min_p, float2 max_p) {
    \\    return all(p >= min_p) && all(p <= max_p) ? 1.0 : 0.0;
    \\}
    \\
    \\float hstroke(float2 p, float start, float end, float cell_w, float cell_h, float thickness) {
    \\    float edge = 0.5;
    \\    float min_x = start <= 0.0 ? -edge : start - edge;
    \\    float max_x = end >= cell_w ? cell_w + edge : end + edge;
    \\    float y0 = (cell_h - thickness) * 0.5;
    \\    return rect_alpha(p, float2(min_x, y0), float2(max_x, y0 + thickness));
    \\}
    \\
    \\float vstroke(float2 p, float start, float end, float cell_w, float cell_h, float thickness) {
    \\    float edge = 0.5;
    \\    float min_y = start <= 0.0 ? -edge : start - edge;
    \\    float max_y = end >= cell_h ? cell_h + edge : end + edge;
    \\    float x0 = (cell_w - thickness) * 0.5;
    \\    return rect_alpha(p, float2(x0, min_y), float2(x0 + thickness, max_y));
    \\}
    \\
    \\float double_hstroke(float2 p, float start, float end, float cell_w, float cell_h, float thickness) {
    \\    float lane = max(cell_h * 0.24, 1.0);
    \\    float edge = 0.5;
    \\    float min_x = start <= 0.0 ? -edge : start - edge;
    \\    float max_x = end >= cell_w ? cell_w + edge : end + edge;
    \\    float top = rect_alpha(p, float2(min_x, lane - thickness * 0.5), float2(max_x, lane + thickness * 0.5));
    \\    float bottom = rect_alpha(
    \\        p,
    \\        float2(min_x, cell_h - lane - thickness * 0.5),
    \\        float2(max_x, cell_h - lane + thickness * 0.5)
    \\    );
    \\    return max(top, bottom);
    \\}
    \\
    \\float double_vstroke(float2 p, float start, float end, float cell_w, float cell_h, float thickness) {
    \\    float lane = max(cell_w * 0.24, 1.0);
    \\    float edge = 0.5;
    \\    float min_y = start <= 0.0 ? -edge : start - edge;
    \\    float max_y = end >= cell_h ? cell_h + edge : end + edge;
    \\    float left = rect_alpha(p, float2(lane - thickness * 0.5, min_y), float2(lane + thickness * 0.5, max_y));
    \\    float right = rect_alpha(
    \\        p,
    \\        float2(cell_w - lane - thickness * 0.5, min_y),
    \\        float2(cell_w - lane + thickness * 0.5, max_y)
    \\    );
    \\    return max(left, right);
    \\}
    \\
    \\float geometry_alpha(uint cp, float2 uv) {
    \\    float2 p = uv * cell_size;
    \\    float cell_w = cell_size.x;
    \\    float cell_h = cell_size.y;
    \\    float mid_x = cell_w * 0.5;
    \\    float mid_y = cell_h * 0.5;
    \\    float light = ceil(max(min(cell_w, cell_h) * 0.12, 1.0));
    \\    float heavy = ceil(max(light * 1.8, 2.0));
    \\    float a = 0.0;
    \\    switch (cp) {
    \\    case 0x2588u: return 1.0;
    \\    case 0x2580u: return rect_alpha(p, float2(0.0, 0.0), float2(cell_w, cell_h * 0.5));
    \\    case 0x2584u: return rect_alpha(p, float2(0.0, cell_h * 0.5), float2(cell_w, cell_h));
    \\    case 0x258Cu: return rect_alpha(p, float2(0.0, 0.0), float2(cell_w * 0.5, cell_h));
    \\    case 0x2590u: return rect_alpha(p, float2(cell_w * 0.5, 0.0), float2(cell_w, cell_h));
    \\    case 0x2591u: return 0.25;
    \\    case 0x2592u: return 0.50;
    \\    case 0x2593u: return 0.75;
    \\    case 0x2500u: return hstroke(p, 0.0, cell_w, cell_w, cell_h, light);
    \\    case 0x2501u: return hstroke(p, 0.0, cell_w, cell_w, cell_h, heavy);
    \\    case 0x2502u: return vstroke(p, 0.0, cell_h, cell_w, cell_h, light);
    \\    case 0x2503u: return vstroke(p, 0.0, cell_h, cell_w, cell_h, heavy);
    \\    case 0x2550u: return double_hstroke(p, 0.0, cell_w, cell_w, cell_h, light);
    \\    case 0x2551u: return double_vstroke(p, 0.0, cell_h, cell_w, cell_h, light);
    \\    case 0x250Cu:
    \\    case 0x256Du:
    \\        a = max(hstroke(p, mid_x, cell_w, cell_w, cell_h, light), vstroke(p, mid_y, cell_h, cell_w, cell_h, light));
    \\        return a;
    \\    case 0x2510u:
    \\    case 0x256Eu:
    \\        a = max(hstroke(p, 0.0, mid_x, cell_w, cell_h, light), vstroke(p, mid_y, cell_h, cell_w, cell_h, light));
    \\        return a;
    \\    case 0x2514u:
    \\    case 0x2570u:
    \\        a = max(hstroke(p, mid_x, cell_w, cell_w, cell_h, light), vstroke(p, 0.0, mid_y, cell_w, cell_h, light));
    \\        return a;
    \\    case 0x2518u:
    \\    case 0x256Fu:
    \\        a = max(hstroke(p, 0.0, mid_x, cell_w, cell_h, light), vstroke(p, 0.0, mid_y, cell_w, cell_h, light));
    \\        return a;
    \\    case 0x251Cu: return max(hstroke(p, mid_x, cell_w, cell_w, cell_h, light), vstroke(p, 0.0, cell_h, cell_w, cell_h, light));
    \\    case 0x2524u: return max(hstroke(p, 0.0, mid_x, cell_w, cell_h, light), vstroke(p, 0.0, cell_h, cell_w, cell_h, light));
    \\    case 0x252Cu: return max(hstroke(p, 0.0, cell_w, cell_w, cell_h, light), vstroke(p, mid_y, cell_h, cell_w, cell_h, light));
    \\    case 0x2534u: return max(hstroke(p, 0.0, cell_w, cell_w, cell_h, light), vstroke(p, 0.0, mid_y, cell_w, cell_h, light));
    \\    case 0x253Cu: return max(hstroke(p, 0.0, cell_w, cell_w, cell_h, light), vstroke(p, 0.0, cell_h, cell_w, cell_h, light));
    \\    case 0x250Fu: return max(hstroke(p, mid_x, cell_w, cell_w, cell_h, heavy), vstroke(p, mid_y, cell_h, cell_w, cell_h, heavy));
    \\    case 0x2513u: return max(hstroke(p, 0.0, mid_x, cell_w, cell_h, heavy), vstroke(p, mid_y, cell_h, cell_w, cell_h, heavy));
    \\    case 0x2517u: return max(hstroke(p, mid_x, cell_w, cell_w, cell_h, heavy), vstroke(p, 0.0, mid_y, cell_w, cell_h, heavy));
    \\    case 0x251Bu: return max(hstroke(p, 0.0, mid_x, cell_w, cell_h, heavy), vstroke(p, 0.0, mid_y, cell_w, cell_h, heavy));
    \\    case 0x2523u: return max(hstroke(p, mid_x, cell_w, cell_w, cell_h, heavy), vstroke(p, 0.0, cell_h, cell_w, cell_h, heavy));
    \\    case 0x252Bu: return max(hstroke(p, 0.0, mid_x, cell_w, cell_h, heavy), vstroke(p, 0.0, cell_h, cell_w, cell_h, heavy));
    \\    case 0x2533u: return max(hstroke(p, 0.0, cell_w, cell_w, cell_h, heavy), vstroke(p, mid_y, cell_h, cell_w, cell_h, heavy));
    \\    case 0x253Bu: return max(hstroke(p, 0.0, cell_w, cell_w, cell_h, heavy), vstroke(p, 0.0, mid_y, cell_w, cell_h, heavy));
    \\    case 0x254Bu: return max(hstroke(p, 0.0, cell_w, cell_w, cell_h, heavy), vstroke(p, 0.0, cell_h, cell_w, cell_h, heavy));
    \\    case 0x2554u: return max(double_hstroke(p, mid_x, cell_w, cell_w, cell_h, light), double_vstroke(p, mid_y, cell_h, cell_w, cell_h, light));
    \\    case 0x2557u: return max(double_hstroke(p, 0.0, mid_x, cell_w, cell_h, light), double_vstroke(p, mid_y, cell_h, cell_w, cell_h, light));
    \\    case 0x255Au: return max(double_hstroke(p, mid_x, cell_w, cell_w, cell_h, light), double_vstroke(p, 0.0, mid_y, cell_w, cell_h, light));
    \\    case 0x255Du: return max(double_hstroke(p, 0.0, mid_x, cell_w, cell_h, light), double_vstroke(p, 0.0, mid_y, cell_w, cell_h, light));
    \\    case 0x2560u: return max(double_hstroke(p, mid_x, cell_w, cell_w, cell_h, light), double_vstroke(p, 0.0, cell_h, cell_w, cell_h, light));
    \\    case 0x2563u: return max(double_hstroke(p, 0.0, mid_x, cell_w, cell_h, light), double_vstroke(p, 0.0, cell_h, cell_w, cell_h, light));
    \\    case 0x2566u: return max(double_hstroke(p, 0.0, cell_w, cell_w, cell_h, light), double_vstroke(p, mid_y, cell_h, cell_w, cell_h, light));
    \\    case 0x2569u: return max(double_hstroke(p, 0.0, cell_w, cell_w, cell_h, light), double_vstroke(p, 0.0, mid_y, cell_w, cell_h, light));
    \\    case 0x256Cu: return max(double_hstroke(p, 0.0, cell_w, cell_w, cell_h, light), double_vstroke(p, 0.0, cell_h, cell_w, cell_h, light));
    \\    default: return 0.0;
    \\    }
    \\}
    \\
    \\float4 main(PSIn i) : SV_Target {
    \\    float geometry = geometry_alpha(i.codepoint, i.cell_uv);
    \\    if (geometry > 0.0) {
    \\        return lerp(i.bg, float4(i.fg.rgb, 1), saturate(geometry));
    \\    }
    \\    if (i.has_glyph < 0.5) {
    \\        return i.bg;
    \\    }
    \\    float a = atlas.Sample(samp, i.uv).r;
    \\    return lerp(i.bg, float4(i.fg.rgb, 1), a);
    \\}
;

/// Compute shader (cs_5_0)
/// Applies compact dirty cell payloads into the persistent GPU cell buffer.
pub const CS_SOURCE: []const u8 =
    \\struct CellData {
    \\    uint glyph_idx;
    \\    uint codepoint;
    \\    uint fg_rgba;
    \\    uint bg_rgba;
    \\    uint attrs;
    \\};
    \\
    \\StructuredBuffer<CellData> dirty_values : register(t0);
    \\StructuredBuffer<uint> dirty_indices : register(t1);
    \\RWStructuredBuffer<CellData> dst_cells : register(u0);
    \\
    \\[numthreads(64, 1, 1)]
    \\void main(uint3 dispatch_thread_id : SV_DispatchThreadID) {
    \\    uint index = dispatch_thread_id.x;
    \\    uint dst_index = dirty_indices[index];
    \\    dst_cells[dst_index] = dirty_values[index];
    \\}
;
