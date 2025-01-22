use std::{num::NonZero, sync::Arc};

use breakwater_parser::FrameBuffer;
use eframe::glow::{self, HasContext};

const VERTEX: Vertex = Vertex {
    position: [0.0; 2],
    tex_coords: [0.0; 2],
};

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 2],
    pub tex_coords: [f32; 2],
}

/// Handles opengl related stuff to instruct a gpu to draw the framebuffer into a an egui widget.
#[derive(Debug)]
pub struct CanvasRenderer<FB: FrameBuffer> {
    framebuffer: Arc<FB>,
    vertex_array: glow::VertexArray,
    vertex_buffer: glow::Buffer,
    canvas_texture: glow::Texture,
    canvas_shaders: glow::Program,
}

impl<FB: FrameBuffer> CanvasRenderer<FB> {
    pub fn new(
        gl: &eframe::glow::Context,
        framebuffer: Arc<FB>,
        view_ports: NonZero<usize>,
    ) -> Self {
        let (vertex_array, vertex_buffer) = unsafe { init_vertex_data(gl, view_ports.get()) };
        let canvas_texture = unsafe {
            init_canvas_texture(
                gl,
                framebuffer.get_width() as i32,
                framebuffer.get_height() as i32,
            )
        };
        let canvas_shaders = unsafe { init_shaders(gl) };

        Self {
            framebuffer,
            vertex_array,
            vertex_buffer,
            canvas_texture,
            canvas_shaders,
        }
    }

    pub fn prepare(
        &self,
        gl: &glow::Context,
        view_port_index: usize,
        new_vertices: Option<[Vertex; 4]>,
    ) {
        // This function gets called once per frame for every viewport.
        // We only want to upload the framebuffer to the gpu once per frame,
        // so we do it on the first viewport only.
        // This saves bandwidth to the gpu and ensures a consistent pixelflut canvas across
        // all viewports.
        if view_port_index == 0 {
            unsafe {
                gl.bind_texture(glow::TEXTURE_2D, Some(self.canvas_texture));

                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    self.framebuffer.get_width() as i32,
                    self.framebuffer.get_height() as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(self.framebuffer.as_bytes()),
                );

                gl.bind_texture(glow::TEXTURE_2D, None);
            }
        }

        if let Some(new_vertices) = new_vertices {
            unsafe {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vertex_buffer));
                gl.buffer_sub_data_u8_slice(
                    glow::ARRAY_BUFFER,
                    (std::mem::size_of::<Vertex>() * 4 * view_port_index) as i32,
                    bytemuck::cast_slice(&new_vertices),
                );
            }
        }
    }

    pub fn paint(&self, gl: &glow::Context, view_port_index: usize) {
        unsafe {
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.use_program(Some(self.canvas_shaders));

            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.canvas_texture));
            let texture_location = gl.get_uniform_location(self.canvas_shaders, "canvas_texture");
            gl.uniform_1_i32(texture_location.as_ref(), 0);

            gl.bind_vertex_array(Some(self.vertex_array));

            let offset = (4 * view_port_index) as i32;
            gl.draw_arrays(glow::TRIANGLE_STRIP, offset, 4);

            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
        }
    }
}

unsafe fn init_vertex_data(
    gl: &glow::Context,
    view_port_count: usize,
) -> (glow::VertexArray, glow::Buffer) {
    let vao = gl.create_vertex_array().unwrap();
    gl.bind_vertex_array(Some(vao));

    let vbo = gl.create_buffer().unwrap();
    gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
    gl.buffer_data_size(
        glow::ARRAY_BUFFER,
        (std::mem::size_of::<Vertex>() * 4 * view_port_count) as i32,
        glow::STATIC_DRAW,
    );

    gl.enable_vertex_attrib_array(0);
    gl.vertex_attrib_pointer_f32(
        0,
        2,
        glow::FLOAT,
        false,
        std::mem::size_of_val(&VERTEX) as i32,
        0,
    );
    gl.enable_vertex_attrib_array(1);
    gl.vertex_attrib_pointer_f32(
        1,
        2,
        glow::FLOAT,
        false,
        std::mem::size_of_val(&VERTEX) as i32,
        std::mem::size_of_val(&VERTEX.position) as i32,
    );

    // Unbind for safety
    gl.bind_vertex_array(None);

    (vao, vbo)
}

unsafe fn init_canvas_texture(gl: &glow::Context, width: i32, height: i32) -> glow::Texture {
    let texture = gl.create_texture().unwrap();
    gl.bind_texture(glow::TEXTURE_2D, Some(texture));

    gl.tex_image_2d(
        glow::TEXTURE_2D,
        0,
        glow::RGBA as i32,
        width,
        height,
        0,
        glow::RGBA,
        glow::UNSIGNED_BYTE,
        None,
    );

    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::REPEAT as i32);
    gl.tex_parameter_i32(
        glow::TEXTURE_2D,
        glow::TEXTURE_MIN_FILTER,
        glow::LINEAR as i32,
    );
    gl.tex_parameter_i32(
        glow::TEXTURE_2D,
        glow::TEXTURE_MAG_FILTER,
        glow::LINEAR as i32,
    );
    gl.bind_texture(glow::TEXTURE_2D, None);

    texture
}

unsafe fn init_shaders(gl: &glow::Context) -> glow::Program {
    let vertex_shader = gl.create_shader(glow::VERTEX_SHADER).unwrap();
    gl.shader_source(vertex_shader, include_str!("./canvas.vert"));
    gl.compile_shader(vertex_shader);

    if !gl.get_shader_compile_status(vertex_shader) {
        panic!(
            "vertex_shader compilation failed: {}",
            gl.get_shader_info_log(vertex_shader)
        );
    }

    let fragment_shader = gl.create_shader(glow::FRAGMENT_SHADER).unwrap();
    gl.shader_source(fragment_shader, include_str!("./canvas.frag"));
    gl.compile_shader(fragment_shader);

    if !gl.get_shader_compile_status(fragment_shader) {
        panic!(
            "fragment_shader compilation failed: {}",
            gl.get_shader_info_log(fragment_shader)
        );
    }

    let program = gl.create_program().unwrap();
    gl.attach_shader(program, vertex_shader);
    gl.attach_shader(program, fragment_shader);
    gl.link_program(program);

    if !gl.get_program_link_status(program) {
        panic!(
            "Shader program linking failed: {}",
            gl.get_program_info_log(program)
        );
    }

    gl.delete_shader(vertex_shader);
    gl.delete_shader(fragment_shader);

    program
}
