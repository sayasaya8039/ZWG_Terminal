#version 450
// Terminal cell instanced quad — vertex shader
// Each instance = 1 terminal cell, 6 vertices = 2 triangles

layout(push_constant) uniform Constants {
    vec2 viewport_size;
    vec2 cell_size;
    vec2 atlas_pitch_inv;   // 1.0 / atlas slot pitch
    vec2 atlas_glyph_inv;   // 1.0 / glyph bitmap size
    float term_cols;
    float atlas_grid_cols;
    vec2 _pad0;
};

// GpuCellData: 5 × uint = 20 bytes (matches Zig GpuCellData layout)
// [0] = col(u16) | row(u16)
// [1] = codepoint(u32)
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
    uint fg_rgba   = cell_data[base + 2];
    uint bg_rgba   = cell_data[base + 3];

    uint grid_cols = max(uint(term_cols), 1u);
    vec2 v = QUAD[vid];
    vec2 cell_origin = vec2(
        float(iid % grid_cols) * cell_size.x,
        float(iid / grid_cols) * cell_size.y
    );
    vec2 px = cell_origin + v * cell_size;

    gl_Position = vec4(
        px.x / viewport_size.x *  2.0 - 1.0,
        px.y / viewport_size.y * -2.0 + 1.0,
        0.0, 1.0
    );

    // Atlas UV (placeholder — glyph atlas not yet wired)
    out_uv = vec2(0.0);
    out_fg = unpack_rgba(fg_rgba);
    out_bg = unpack_rgba(bg_rgba);
    out_has_glyph = 0.0; // no atlas yet
}
