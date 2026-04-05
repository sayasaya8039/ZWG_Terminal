#version 450
// Terminal cell instanced quad — vertex shader
// Each instance = 1 terminal cell, 6 vertices = 2 triangles

layout(push_constant) uniform Constants {
    float viewport_w;       // [0]
    float viewport_h;       // [1]
    float cell_w;           // [2]
    float cell_h;           // [3]
    float atlas_pitch_u;    // [4] slot_pitch / ATLAS_SIZE
    float atlas_pitch_v;    // [5] slot_pitch / ATLAS_SIZE
    float atlas_glyph_u;   // [6] glyph_size / ATLAS_SIZE
    float atlas_glyph_v;   // [7] glyph_size / ATLAS_SIZE
    float term_cols;        // [8]
    float atlas_grid_cols;  // [9]
    float _pad0;            // [10]
    float _pad1;            // [11]
};

// GpuCellData: 5 × uint = 20 bytes (matches Zig GpuCellData layout)
// [0] = col(u16) | row(u16)
// [1] = glyph_index (0 = no glyph, 1+ = atlas index)
// [2] = fg_rgba(u32)
// [3] = bg_rgba(u32)
// [4] = flags(u16) | pad(u16)
layout(std430, set = 0, binding = 0) readonly buffer CellBuffer {
    uint cell_data[];
};

layout(location = 0) out vec2 out_uv;
layout(location = 1) out vec4 out_fg;
layout(location = 2) out vec4 out_bg;
layout(location = 3) out float out_has_glyph;

const vec2 QUAD[6] = vec2[](
    vec2(0,0), vec2(1,0), vec2(0,1),
    vec2(1,0), vec2(1,1), vec2(0,1)
);

vec4 unpack_rgba(uint c) {
    return vec4(
        float((c >> 16) & 0xFFu) / 255.0,
        float((c >> 8)  & 0xFFu) / 255.0,
        float( c        & 0xFFu) / 255.0,
        float((c >> 24) & 0xFFu) / 255.0
    );
}

void main() {
    uint vid = gl_VertexIndex % 6;
    uint iid = gl_InstanceIndex;

    // Read packed cell data (5 uints per cell)
    uint base = iid * 5;
    uint glyph_index = cell_data[base + 1];
    uint fg_rgba     = cell_data[base + 2];
    uint bg_rgba     = cell_data[base + 3];

    uint grid_cols = max(uint(term_cols), 1u);
    vec2 v = QUAD[vid];
    vec2 cell_origin = vec2(
        float(iid % grid_cols) * cell_w,
        float(iid / grid_cols) * cell_h
    );
    vec2 px = cell_origin + v * vec2(cell_w, cell_h);

    gl_Position = vec4(
        px.x / viewport_w *  2.0 - 1.0,
        px.y / viewport_h * -2.0 + 1.0,
        0.0, 1.0
    );

    // Atlas UV computation
    if (glyph_index > 0u) {
        uint atlas_idx = glyph_index - 1u;
        uint atlas_col = atlas_idx % uint(atlas_grid_cols);
        uint atlas_row = atlas_idx / uint(atlas_grid_cols);
        vec2 atlas_origin = vec2(
            float(atlas_col) * atlas_pitch_u,
            float(atlas_row) * atlas_pitch_v
        );
        vec2 local_uv = v; // 0..1 range within cell quad
        out_uv = atlas_origin + local_uv * vec2(atlas_glyph_u, atlas_glyph_v);
        out_has_glyph = 1.0;
    } else {
        out_uv = vec2(0.0);
        out_has_glyph = 0.0;
    }

    out_fg = unpack_rgba(fg_rgba);
    out_bg = unpack_rgba(bg_rgba);
}
