#version 450
// Terminal cell — fragment shader (Phase 2: bg-only, no atlas)

layout(location = 0) in vec2 in_uv;
layout(location = 1) in vec4 in_fg;
layout(location = 2) in vec4 in_bg;
layout(location = 3) in float in_has_glyph;

layout(location = 0) out vec4 out_color;

void main() {
    // Phase 2: render background colors only (atlas sampling in Phase 3)
    out_color = in_bg;
}
