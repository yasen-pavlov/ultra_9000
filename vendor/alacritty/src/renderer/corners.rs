// embed: Native left-corner coverage mask, new code (Apache-2.0).
// Two scissored quads multiply the completed premultiplied framebuffer by coverage.
use std::{mem, ptr};

use glutin::surface::Rect;

use crate::display::SizeInfo;
use crate::gl;
use crate::renderer::shader::{ShaderProgram, ShaderVersion};
use crate::renderer::{cstr, Error};

const VERTEX: &str = r#"
#ifdef GLES2_RENDERER
attribute vec2 position;
#else
in vec2 position;
#endif
void main() { gl_Position = vec4(position, 0.0, 1.0); }
"#;
const FRAGMENT: &str = r#"
#ifdef GLES2_RENDERER
precision highp float;
#define MASK_COLOR gl_FragColor
#else
out vec4 maskColor;
#define MASK_COLOR maskColor
#endif
uniform vec4 circle;
void main() {
    // Only the outer quadrant curves; the straight edges past the centre do not.
    vec2 d = max(vec2(circle.x - gl_FragCoord.x,
                      (gl_FragCoord.y - circle.y) * circle.w), 0.0);
    float coverage = clamp(circle.z - length(d) + 0.5, 0.0, 1.0);
    MASK_COLOR = vec4(0.0, 0.0, 0.0, 1.0 - coverage);
}
"#;

#[derive(Debug)]
pub struct CornerRenderer {
    program: ShaderProgram,
    circle: i32,
    vao: u32,
    vbo: u32,
}

/// GL-origin rectangles cover the whole antialias fringe, including subpixel radii.
pub fn damage_rects(size: &SizeInfo, radii: [f32; 2]) -> [Rect; 2] {
    let height = size.height() as i32;
    let side = |radius: f32| (radius + 0.5).ceil() as i32;
    let top = if radii[0] > 0. { side(radii[0]) } else { 0 };
    let bottom = if radii[1] > 0. { side(radii[1]) } else { 0 };
    [Rect::new(0, height - top, top, top), Rect::new(0, 0, bottom, bottom)]
}

impl CornerRenderer {
    pub fn new(version: ShaderVersion) -> Result<Self, Error> {
        let program = ShaderProgram::new(version, None, VERTEX, FRAGMENT)?;
        let circle = program.get_uniform_location(cstr!("circle"))?;
        let vertices: [f32; 8] = [-1., -1., 1., -1., -1., 1., 1., 1.];
        let (mut vao, mut vbo) = (0, 0);
        unsafe {
            gl::GenVertexArrays(1, &mut vao);
            gl::GenBuffers(1, &mut vbo);
            gl::BindVertexArray(vao);
            gl::BindBuffer(gl::ARRAY_BUFFER, vbo);
            gl::BufferData(gl::ARRAY_BUFFER, mem::size_of_val(&vertices) as isize,
                vertices.as_ptr().cast(), gl::STATIC_DRAW);
            let position = gl::GetAttribLocation(program.id(), cstr!("position").as_ptr()) as u32;
            gl::VertexAttribPointer(position, 2, gl::FLOAT, gl::FALSE, 0, ptr::null());
            gl::EnableVertexAttribArray(position);
            gl::BindBuffer(gl::ARRAY_BUFFER, 0);
            gl::BindVertexArray(0);
        }
        Ok(Self { program, circle, vao, vbo })
    }

    /// Called after every other draw. No uploads or allocations in the frame path.
    pub fn draw(&self, size: &SizeInfo, radii: [f32; 2]) {
        if radii == [0.; 2] {
            return;
        }
        unsafe {
            gl::Viewport(0, 0, size.width() as i32, size.height() as i32);
            gl::Enable(gl::SCISSOR_TEST);
            // Both RGB and alpha are attenuated, never just alpha (Wayland is premultiplied).
            gl::BlendFunc(gl::ZERO, gl::ONE_MINUS_SRC_ALPHA);
            gl::UseProgram(self.program.id());
            gl::BindVertexArray(self.vao);
            for (index, rect) in damage_rects(size, radii).into_iter().enumerate() {
                if rect.width == 0 {
                    continue;
                }
                let radius = radii[index];
                let center_y = if index == 0 { size.height() - radius } else { radius };
                gl::Scissor(rect.x, rect.y, rect.width, rect.height);
                gl::Uniform4f(self.circle, radius, center_y, radius, if index == 0 { 1. } else { -1. });
                gl::DrawArrays(gl::TRIANGLE_STRIP, 0, 4);
            }
            gl::BindVertexArray(0);
            gl::UseProgram(0);
            gl::Disable(gl::SCISSOR_TEST);
            gl::BlendFunc(gl::SRC1_COLOR, gl::ONE_MINUS_SRC1_COLOR);
        }
    }
}

impl Drop for CornerRenderer {
    fn drop(&mut self) {
        unsafe {
            gl::DeleteBuffers(1, &self.vbo);
            gl::DeleteVertexArrays(1, &self.vao);
        }
    }
}
