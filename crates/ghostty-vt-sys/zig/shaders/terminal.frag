#version 450
// Terminal cell — fragment shader (atlas sampling + bg/fg blending)

layout(set = 0, binding = 1) uniform sampler2D atlas;

layout(location = 0) in vec2 in_uv;
layout(location = 1) in vec4 in_fg;
layout(location = 2) in vec4 in_bg;
layout(location = 3) in float in_has_glyph;

layout(location = 0) out vec4 out_color;

void main() {
    if (in_has_glyph > 0.5) {
        float alpha = texture(atlas, in_uv).r;
        out_color = mix(in_bg, vec4(in_fg.rgb, 1.0), alpha);
    } else {
        out_color = in_bg;
    }
}
