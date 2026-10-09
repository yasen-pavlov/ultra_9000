// From ayosec's graphics patch (https://github.com/ayosec/alacritty, tag v0.16.0-graphics),
// Apache-2.0. Modified: also draws Kitty placeholder tiles (one quad per cell sampling a
// sub-rectangle of the placement texture, scaled to the current cell size); vertices carry
// pixel rectangles and texture coordinates computed here; a batch is cut only when the texture
// slots run out, not every 16 quads. See vendor/PROVENANCE.md.

//! This module implements the functionality to render graphic textures
//! in the display.
//!
//! [`RenderList`] is used to track graphics in the visible cells. When all
//! cells in the grid are read, graphics are rendered using the positions
//! found in those cells.

use std::collections::{BTreeMap, HashMap};
use std::mem;

use crate::display::SizeInfo;
use crate::display::content::RenderableCell;
use crate::gl::types::*;
use crate::gl::{self};
use crate::renderer::graphics::{GraphicTexture, GraphicsRenderer, shader};
use crate::renderer::{RenderRect, Rgb};

use alacritty_terminal::graphics::GraphicId;
use alacritty_terminal::index::Column;

use crossfont::Metrics;

use log::trace;

/// Position to render each texture in the grid.
struct RenderPosition {
    column: Column,
    line: usize,
    offset_x: u16,
    offset_y: u16,
    cell_color: Rgb,
    show_hint: bool,
}

/// Kitty placeholder tile to render in a single cell.
struct RenderTile {
    id: GraphicId,
    column: Column,
    line: usize,

    /// Source rectangle in texture pixels.
    source_x: f32,
    source_y: f32,
    width: f32,
    height: f32,
}

/// Track textures to be rendered in the display.
#[derive(Default)]
pub struct RenderList {
    items: BTreeMap<GraphicId, RenderPosition>,
    tiles: Vec<RenderTile>,
}

impl RenderList {
    /// Detects if the cell contains a graphic, then add it to the render list.
    ///
    /// A graphic is added only the first time it is found in a cell. A Kitty placeholder tile is
    /// added for every cell that shows one.
    #[inline]
    pub fn update(&mut self, cell: &RenderableCell, show_hint: bool) {
        let extra = match cell.extra.as_ref() {
            Some(extra) => extra,
            None => return,
        };

        if let Some(tile) = &extra.kitty_tile {
            self.tiles.push(RenderTile {
                id: tile.id,
                column: cell.point.column,
                line: cell.point.line,
                source_x: tile.source_x,
                source_y: tile.source_y,
                width: tile.width,
                height: tile.height,
            });
        }

        let graphics = match extra.graphics.as_ref() {
            Some(graphics) => graphics,
            _ => return,
        };

        for graphic in graphics {
            if self.items.contains_key(&graphic.id) {
                continue;
            }

            let render_item = RenderPosition {
                column: cell.point.column,
                line: cell.point.line,
                offset_x: graphic.offset_x,
                offset_y: graphic.offset_y,
                cell_color: cell.fg,
                show_hint,
            };

            self.items.insert(graphic.id, render_item);
        }
    }

    /// Returns `true` if there are no items to render.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.tiles.is_empty()
    }

    /// Builds a list of vertex for the shader program.
    ///
    /// Whole graphics come first, in id order, then the tiles grouped by texture, so consecutive
    /// quads share texture slots.
    fn build_vertices(
        self,
        textures: &HashMap<GraphicId, GraphicTexture>,
        size_info: &SizeInfo,
        rects: &mut Vec<RenderRect>,
        line_thickness: f32,
    ) -> Vec<shader::Vertex> {
        let mut vertices = Vec::with_capacity((self.items.len() + self.tiles.len()) * 6);

        for (graphics_id, render_item) in self.items {
            let graphic_texture = match textures.get(&graphics_id) {
                Some(tex) => tex,
                None => continue,
            };

            let scale = size_info.cell_height() / graphic_texture.cell_height;

            let x = render_item.column.0 as f32 * size_info.cell_width()
                - render_item.offset_x as f32 * scale;
            let y = render_item.line as f32 * size_info.cell_height()
                - render_item.offset_y as f32 * scale;

            let tex_width = graphic_texture.width as f32 * scale;
            let tex_height = graphic_texture.height as f32 * scale;

            push_quad(&mut vertices, shader::Vertex {
                texture_id: graphic_texture.texture.0,
                sides: shader::VertexSide::TopLeft,
                x,
                y,
                width: tex_width,
                height: tex_height,
                tex_left: 0.,
                tex_top: 0.,
                tex_right: 1.,
                tex_bottom: 1.,
            });

            if render_item.show_hint {
                let right = x + tex_width - line_thickness;
                let bottom = y + tex_height - line_thickness;

                let template = RenderRect {
                    x,
                    y,
                    width: tex_width,
                    height: line_thickness,
                    color: render_item.cell_color,
                    alpha: 1.,
                    kind: crate::renderer::rects::RectKind::Normal,
                };

                rects.push(template);
                rects.push(RenderRect { y: bottom, ..template });

                let template = RenderRect { width: line_thickness, height: tex_height, ..template };
                rects.push(template);
                rects.push(RenderRect { x: right, ..template });
            }
        }

        let mut tiles = self.tiles;
        tiles.sort_by_key(|tile| tile.id);

        for tile in tiles {
            let graphic_texture = match textures.get(&tile.id) {
                Some(tex) => tex,
                None => continue,
            };

            // Rejects empty and NaN sizes.
            if !(tile.width > 0. && tile.height > 0.) {
                continue;
            }

            // Clip the source rectangle to the texture: the part outside it is transparent.
            let texture_width = graphic_texture.width as f32;
            let texture_height = graphic_texture.height as f32;
            let left = tile.source_x.max(0.);
            let top = tile.source_y.max(0.);
            let right = (tile.source_x + tile.width).min(texture_width);
            let bottom = (tile.source_y + tile.height).min(texture_height);
            if !(right > left && bottom > top) {
                continue;
            }

            // The whole source rectangle maps onto the cell, whatever the cell size is now.
            let scale_x = size_info.cell_width() / tile.width;
            let scale_y = size_info.cell_height() / tile.height;
            let cell_x = tile.column.0 as f32 * size_info.cell_width();
            let cell_y = tile.line as f32 * size_info.cell_height();

            push_quad(&mut vertices, shader::Vertex {
                texture_id: graphic_texture.texture.0,
                sides: shader::VertexSide::TopLeft,
                x: cell_x + (left - tile.source_x) * scale_x,
                y: cell_y + (top - tile.source_y) * scale_y,
                width: (right - left) * scale_x,
                height: (bottom - top) * scale_y,
                tex_left: left / texture_width,
                tex_top: top / texture_height,
                tex_right: right / texture_width,
                tex_bottom: bottom / texture_height,
            });
        }

        vertices
    }

    /// Draw graphics in the display, using the graphics rendering shader
    /// program.
    pub fn draw(
        self,
        renderer: &GraphicsRenderer,
        size_info: &SizeInfo,
        rects: &mut Vec<RenderRect>,
        metrics: &Metrics,
    ) {
        let mut vertices = self.build_vertices(
            &renderer.graphic_textures,
            size_info,
            rects,
            metrics.underline_thickness.max(3.0),
        );

        // Initialize the rendering program.
        unsafe {
            gl::BindBuffer(gl::ARRAY_BUFFER, renderer.program.vbo);
            gl::BindVertexArray(renderer.program.vao);

            gl::UseProgram(renderer.program.shader.id());

            gl::Uniform2f(
                renderer.program.u_view_dimensions,
                size_info.width() - 2.0 * size_info.padding_x(),
                size_info.height() - 2.0 * size_info.padding_y(),
            );

            gl::BlendFuncSeparate(gl::SRC_ALPHA, gl::ONE_MINUS_SRC_ALPHA, gl::SRC_ALPHA, gl::ONE);
        }

        // Draw the vertices in `batch` with a single call to the shader program.
        let send_batch = |batch: &[shader::Vertex]| {
            if batch.is_empty() {
                return;
            }

            trace!("Call glDrawArrays with {} items", batch.len());

            unsafe {
                gl::BufferData(
                    gl::ARRAY_BUFFER,
                    mem::size_of_val(batch) as isize,
                    batch.as_ptr().cast(),
                    gl::STREAM_DRAW,
                );

                gl::DrawArrays(gl::TRIANGLES, 0, batch.len() as GLint);
            }
        };

        // In order to send textures to the shader program we need to get a _slot_
        // for every texture associated to a graphic.
        //
        // We have `u_textures.len()` slots available in each execution of the
        // shader.
        //
        // For each slot we need three values:
        //
        // - The texture unit for `glActiveTexture` (`GL_TEXTUREi`).
        // - The uniform location for `textures[i]`.
        // - The index `i`, used to set the value of the uniform.
        //
        // These values are generated using the `tex_slots_generator` iterator.
        //
        // A single quad has 6 vertices. All vertices will use the same texture
        // slot. To detect if a texture has already a slot, we only need to compare
        // with the last texture (`last_tex_slot`) because all the vertices of a
        // single quad are consecutive, and so are the tiles of one texture.
        //
        // When all slots are occupied, the vertices before the current one are
        // sent as a batch and the iterator is reset. A texture changes only between
        // quads, so a batch always holds whole quads.
        //
        // This logic could be simplified using the [Bindless Texture extension],
        // but it is not a core feature of any OpenGL version, so hardware support
        // is uncertain.
        //
        // [Bindless Texture extension]: https://www.khronos.org/opengl/wiki/Bindless_Texture

        let tex_slots_generator = (gl::TEXTURE0..=gl::TEXTURE31)
            .zip(renderer.program.u_textures.iter())
            .zip(0_u32..)
            .map(|((tex_enum, &u_texture), index)| (tex_enum, u_texture, index));

        let mut tex_slots = tex_slots_generator.clone();

        // Keep the last allocated slot in a `(texture id, index)` tuple.
        let mut last_tex_slot = (0, 0);

        // First vertex of the batch not sent yet.
        let mut batch_start = 0;

        for index in 0..vertices.len() {
            let texture_id = vertices[index].texture_id;

            // Check if we can reuse the last texture slot.
            if last_tex_slot.0 != texture_id {
                last_tex_slot = loop {
                    match tex_slots.next() {
                        None => {
                            // No more slots. Send the batch and reset the iterator.
                            send_batch(&vertices[batch_start..index]);
                            batch_start = index;
                            tex_slots = tex_slots_generator.clone();
                        },

                        Some((tex_enum, u_texture, slot)) => {
                            unsafe {
                                gl::ActiveTexture(tex_enum);
                                gl::BindTexture(gl::TEXTURE_2D, texture_id);
                                gl::Uniform1i(u_texture, slot as GLint);
                            }

                            break (texture_id, slot);
                        },
                    }
                };
            }

            vertices[index].texture_id = last_tex_slot.1;
        }

        send_batch(&vertices[batch_start..]);

        // Reset state.
        unsafe {
            gl::BlendFunc(gl::SRC1_COLOR, gl::ONE_MINUS_SRC1_COLOR);

            gl::ActiveTexture(gl::TEXTURE0);
            gl::BindTexture(gl::TEXTURE_2D, 0);

            gl::UseProgram(0);
            gl::BindVertexArray(0);
            gl::BindBuffer(gl::ARRAY_BUFFER, 0);
        }
    }
}

/// Push the six vertices (two triangles) of the quad described by `vertex`.
fn push_quad(vertices: &mut Vec<shader::Vertex>, vertex: shader::Vertex) {
    use shader::VertexSide::{BottomLeft, BottomRight, TopLeft, TopRight};

    for sides in [TopLeft, TopRight, BottomLeft, TopRight, BottomRight, BottomLeft] {
        vertices.push(shader::Vertex { sides, ..vertex });
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::graphics::Graphics;
    use alacritty_terminal::index::Point;
    use alacritty_terminal::term::cell::Flags;
    use shader::VertexSide::{BottomLeft, BottomRight, TopLeft, TopRight};

    use super::*;
    use crate::display::content::{
        RenderableCellExtra, RenderableGraphicCell, RenderableKittyTile,
    };
    use crate::renderer::graphics::TextureName;

    /// Textures for `build_vertices`. GL names are fake and reset before the GL-deleting drop.
    #[derive(Default)]
    struct Textures(HashMap<GraphicId, GraphicTexture>);

    impl Textures {
        fn add(&mut self, id: GraphicId, name: GLuint, width: u16, height: u16, cell_height: f32) {
            let texture = GraphicTexture {
                texture: TextureName(name),
                cell_height,
                cell_width: cell_height / 2.,
                width,
                height,
                retained: false,
            };
            self.0.insert(id, texture);
        }
    }

    impl Drop for Textures {
        fn drop(&mut self) {
            for texture in self.0.values_mut() {
                texture.texture.0 = 0;
            }
        }
    }

    /// Display with a grid of 100x50 cells of `cell_width`x`cell_height` pixels.
    fn size_info(cell_width: f32, cell_height: f32) -> SizeInfo {
        SizeInfo::new(cell_width * 100., cell_height * 50., cell_width, cell_height, 0., 0., false)
    }

    fn cell(column: usize, line: usize, extra: RenderableCellExtra) -> RenderableCell {
        RenderableCell {
            character: ' ',
            point: Point::new(line, Column(column)),
            fg: Rgb::default(),
            bg: Rgb::default(),
            bg_alpha: 0.,
            underline: Rgb::default(),
            flags: Flags::empty(),
            extra: Some(Box::new(extra)),
        }
    }

    fn tile_cell(column: usize, line: usize, tile: RenderableKittyTile) -> RenderableCell {
        let extra = RenderableCellExtra {
            zerowidth: None,
            hyperlink: None,
            graphics: None,
            kitty_tile: Some(tile),
        };
        cell(column, line, extra)
    }

    fn tile(id: GraphicId, row: u32, column: u32, size: (f32, f32)) -> RenderableKittyTile {
        RenderableKittyTile {
            id,
            source_x: column as f32 * size.0,
            source_y: row as f32 * size.1,
            width: size.0,
            height: size.1,
        }
    }

    fn build(list: RenderList, textures: &Textures, size: &SizeInfo) -> Vec<shader::Vertex> {
        list.build_vertices(&textures.0, size, &mut Vec::new(), 3.)
    }

    /// The single quad in `vertices`, as its top-left vertex.
    fn single_quad(vertices: &[shader::Vertex]) -> shader::Vertex {
        assert_eq!(vertices.len(), 6);
        let sides: Vec<_> = vertices.iter().map(|vertex| vertex.sides).collect();
        assert_eq!(sides, [TopLeft, TopRight, BottomLeft, TopRight, BottomRight, BottomLeft]);
        assert!(
            vertices
                .iter()
                .all(|vertex| shader::Vertex { sides: TopLeft, ..*vertex } == vertices[0])
        );
        vertices[0]
    }

    #[test]
    fn tile_samples_its_sub_rectangle_scaled_to_the_cell() {
        let mut graphics = Graphics::default();
        let id = graphics.next_id();
        let mut textures = Textures::default();

        // An 8x5 placement made at 10x20 pixel cells, drawn at 12x24 pixel cells (font zoom).
        textures.add(id, 7, 80, 100, 20.);
        let mut list = RenderList::default();
        list.update(&tile_cell(5, 1, tile(id, 2, 3, (10., 20.))), false);

        let size = size_info(12., 24.);
        let quad = single_quad(&build(list, &textures, &size));
        assert_eq!(quad.texture_id, 7);
        assert_eq!((quad.x, quad.y, quad.width, quad.height), (60., 24., 12., 24.));
        assert_eq!(
            (quad.tex_left, quad.tex_top, quad.tex_right, quad.tex_bottom),
            (30. / 80., 40. / 100., 40. / 80., 60. / 100.)
        );
    }

    #[test]
    fn every_tile_cell_is_drawn() {
        let mut graphics = Graphics::default();
        let first = graphics.next_id();
        let second = graphics.next_id();
        let mut textures = Textures::default();
        textures.add(first, 1, 20, 20, 10.);
        textures.add(second, 2, 20, 20, 10.);

        // Tiles of one texture are not de-duplicated, and are grouped by texture.
        let mut list = RenderList::default();
        list.update(&tile_cell(0, 0, tile(second, 0, 0, (10., 10.))), false);
        list.update(&tile_cell(1, 0, tile(first, 0, 0, (10., 10.))), false);
        list.update(&tile_cell(2, 0, tile(second, 0, 1, (10., 10.))), false);
        list.update(&tile_cell(0, 1, tile(first, 1, 1, (10., 10.))), false);
        assert!(!list.is_empty());

        let vertices = build(list, &textures, &size_info(10., 10.));
        let quads: Vec<_> = vertices
            .chunks(6)
            .map(|quad| (single_quad(quad).texture_id, quad[0].x, quad[0].y))
            .collect();
        assert_eq!(quads, [(1, 10., 0.), (1, 0., 10.), (2, 0., 0.), (2, 20., 0.)]);
    }

    #[test]
    fn tile_is_clipped_to_the_texture() {
        let mut graphics = Graphics::default();
        let id = graphics.next_id();
        let mut textures = Textures::default();
        textures.add(id, 3, 75, 40, 20.);

        // The last column covers texture pixels 70..80 of a 75 pixel wide texture.
        let mut list = RenderList::default();
        list.update(&tile_cell(4, 2, tile(id, 1, 7, (10., 20.))), false);

        let quad = single_quad(&build(list, &textures, &size_info(20., 40.)));
        assert_eq!((quad.x, quad.y, quad.width, quad.height), (80., 80., 10., 40.));
        assert_eq!(
            (quad.tex_left, quad.tex_top, quad.tex_right, quad.tex_bottom),
            (70. / 75., 0.5, 1., 1.)
        );
    }

    #[test]
    fn tile_without_pixels_or_texture_draws_nothing() {
        let mut graphics = Graphics::default();
        let id = graphics.next_id();
        let missing = graphics.next_id();
        let mut textures = Textures::default();
        textures.add(id, 3, 80, 40, 20.);

        let mut list = RenderList::default();
        list.update(&tile_cell(0, 0, tile(id, 2, 0, (10., 20.))), false);
        list.update(&tile_cell(1, 0, tile(id, 0, 8, (10., 20.))), false);
        list.update(&tile_cell(2, 0, tile(id, 0, 0, (0., 20.))), false);
        list.update(&tile_cell(3, 0, tile(id, 0, 0, (f32::NAN, 20.))), false);
        list.update(&tile_cell(4, 0, tile(missing, 0, 0, (10., 20.))), false);

        assert!(build(list, &textures, &size_info(10., 20.)).is_empty());
    }

    #[test]
    fn whole_graphic_keeps_its_offset_and_scale() {
        let mut graphics = Graphics::default();
        let id = graphics.next_id();
        let mut textures = Textures::default();

        // A graphic made at 20 pixel high cells, drawn at 30 pixel high cells.
        textures.add(id, 9, 100, 40, 20.);

        let graphic = |offset_x, offset_y| RenderableCellExtra {
            zerowidth: Some(Vec::new()),
            hyperlink: None,
            graphics: Some(
                [RenderableGraphicCell { id, offset_x, offset_y }].into_iter().collect(),
            ),
            kitty_tile: None,
        };

        // Only the first cell of a graphic places it.
        let mut list = RenderList::default();
        list.update(&cell(2, 3, graphic(10, 20)), false);
        list.update(&cell(3, 3, graphic(20, 20)), false);

        let quad = single_quad(&build(list, &textures, &size_info(15., 30.)));
        assert_eq!(quad.texture_id, 9);
        assert_eq!((quad.x, quad.y, quad.width, quad.height), (15., 60., 150., 60.));
        assert_eq!(
            (quad.tex_left, quad.tex_top, quad.tex_right, quad.tex_bottom),
            (0., 0., 1., 1.)
        );
    }
}
