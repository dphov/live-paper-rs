use glow::HasContext;

const VERT_SRC: &str = "#version 300 es
layout(location = 0) in vec2 a_position;
layout(location = 1) in vec2 a_texcoord;
out vec2 v_texcoord;
void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
    v_texcoord = a_texcoord;
}";

const FRAG_SRC: &str = "#version 300 es
precision mediump float;
in vec2 v_texcoord;
uniform sampler2D u_current;
uniform sampler2D u_next;
uniform float u_alpha;
out vec4 fragColor;
void main() {
    vec4 current = texture(u_current, v_texcoord);
    vec4 next = texture(u_next, v_texcoord);
    fragColor = mix(current, next, u_alpha);
}";

/// GL resources for rendering two mpv frames into off-screen textures and
/// blending them with a crossfade alpha.
pub struct Transition {
    width: i32,
    height: i32,
    current_fbo: glow::Framebuffer,
    current_tex: glow::Texture,
    next_fbo: glow::Framebuffer,
    next_tex: glow::Texture,
    program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
}

fn compile_program(gl: &glow::Context) -> glow::Program {
    unsafe {
        let program = gl.create_program().expect("create program");
        for (kind, src) in [(glow::VERTEX_SHADER, VERT_SRC), (glow::FRAGMENT_SHADER, FRAG_SRC)] {
            let shader = gl.create_shader(kind).expect("create shader");
            gl.shader_source(shader, src);
            gl.compile_shader(shader);
            assert!(
                gl.get_shader_compile_status(shader),
                "shader compile failed: {}",
                gl.get_shader_info_log(shader)
            );
            gl.attach_shader(program, shader);
            gl.delete_shader(shader);
        }
        gl.link_program(program);
        assert!(
            gl.get_program_link_status(program),
            "program link failed: {}",
            gl.get_program_info_log(program)
        );
        program
    }
}

fn slice_to_u8<T>(data: &[T]) -> &[u8] {
    let ptr = data.as_ptr() as *const u8;
    let len = data.len() * std::mem::size_of::<T>();
    unsafe { std::slice::from_raw_parts(ptr, len) }
}

fn create_fbo(gl: &glow::Context, width: i32, height: i32) -> (glow::Framebuffer, glow::Texture) {
    unsafe {
        let tex = gl.create_texture().expect("create texture");
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            width,
            height,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);

        let fbo = gl.create_framebuffer().expect("create framebuffer");
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(tex),
            0,
        );
        assert_eq!(
            gl.check_framebuffer_status(glow::FRAMEBUFFER),
            glow::FRAMEBUFFER_COMPLETE,
            "framebuffer incomplete"
        );

        // Clear to black so uninitialized textures never flash garbage colors.
        gl.viewport(0, 0, width, height);
        gl.clear_color(0.0, 0.0, 0.0, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);

        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);

        (fbo, tex)
    }
}

fn delete_fbo(gl: &glow::Context, fbo: glow::Framebuffer, tex: glow::Texture) {
    unsafe {
        gl.delete_framebuffer(fbo);
        gl.delete_texture(tex);
    }
}

impl Transition {
    pub fn new(gl: &glow::Context, width: i32, height: i32) -> Self {
        let (current_fbo, current_tex) = create_fbo(gl, width, height);
        let (next_fbo, next_tex) = create_fbo(gl, width, height);
        let program = compile_program(gl);

        unsafe {
            let vao = gl.create_vertex_array().expect("create vao");
            let vbo = gl.create_buffer().expect("create vbo");
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));

            // Fullscreen triangle strip quad: positions + texcoords interleaved
            #[rustfmt::skip]
            let vertices: [f32; 16] = [
                -1.0, -1.0,  0.0, 0.0,
                 1.0, -1.0,  1.0, 0.0,
                -1.0,  1.0,  0.0, 1.0,
                 1.0,  1.0,  1.0, 1.0,
            ];
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                slice_to_u8(&vertices),
                glow::STATIC_DRAW,
            );

            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 16, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 16, 8);

            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);

            Self {
                width,
                height,
                current_fbo,
                current_tex,
                next_fbo,
                next_tex,
                program,
                vao,
                vbo,
            }
        }
    }

    pub fn resize(&mut self, gl: &glow::Context, width: i32, height: i32) {
        if self.width == width && self.height == height {
            return;
        }
        delete_fbo(gl, self.current_fbo, self.current_tex);
        delete_fbo(gl, self.next_fbo, self.next_tex);
        let (current_fbo, current_tex) = create_fbo(gl, width, height);
        let (next_fbo, next_tex) = create_fbo(gl, width, height);
        self.width = width;
        self.height = height;
        self.current_fbo = current_fbo;
        self.current_tex = current_tex;
        self.next_fbo = next_fbo;
        self.next_tex = next_tex;
    }

    pub fn current_fbo(&self) -> i32 {
        self.current_fbo.0.get() as i32
    }

    pub fn next_fbo(&self) -> i32 {
        self.next_fbo.0.get() as i32
    }

    /// Clear the next FBO to black before a new transition, so the previous
    /// transition's contents or uninitialized memory never flashes through.
    pub fn clear_next(&self, gl: &glow::Context) {
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.next_fbo));
            gl.viewport(0, 0, self.width, self.height);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }

    /// Render the two off-screen textures to the default framebuffer with the
    /// given crossfade alpha (0.0 = current only, 1.0 = next only).
    pub fn render_blend(&self, gl: &glow::Context, alpha: f32, width: i32, height: i32) {
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.viewport(0, 0, width, height);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);

            gl.use_program(Some(self.program));
            gl.bind_vertex_array(Some(self.vao));

            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.current_tex));
            let loc_current = gl.get_uniform_location(self.program, "u_current");
            gl.uniform_1_i32(loc_current.as_ref(), 0);

            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.next_tex));
            let loc_next = gl.get_uniform_location(self.program, "u_next");
            gl.uniform_1_i32(loc_next.as_ref(), 1);

            let loc_alpha = gl.get_uniform_location(self.program, "u_alpha");
            gl.uniform_1_f32(loc_alpha.as_ref(), alpha.clamp(0.0, 1.0));

            gl.enable(glow::BLEND);
            gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);

            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

            gl.bind_vertex_array(None);
            gl.use_program(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
    }
}

// Note: Transition intentionally does not implement Drop. glow::Context is not
// Clone/Copy and we do not store it, so we cannot reliably delete resources on
// drop. GPU resources are reclaimed by the OS when the renderer process exits;
// resize() explicitly deletes the old FBO/texture pair when recreating them.
