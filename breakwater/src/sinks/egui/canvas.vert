#version 300 es

layout(location = 0) in vec2 position;
layout(location = 1) in vec2 tex_coords;

out vec2 v_tex_coords;

void main() {
    gl_Position = vec4(position, 0.0, 1.0);
    v_tex_coords = tex_coords;
}
