#version 300 es
precision mediump float;

in vec2 v_tex_coords;
out vec4 color;

uniform sampler2D canvas_texture;

void main() {
    color = texture(canvas_texture, v_tex_coords);
}
