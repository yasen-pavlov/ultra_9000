// From ayosec's graphics patch (https://github.com/ayosec/alacritty, tag v0.16.0-graphics),
// Apache-2.0. Modified: each vertex carries its destination rectangle in pixels and its texture
// coordinates, computed on the CPU, so a quad can draw a sub-rectangle of a texture (Kitty
// placeholder tiles) as well as a whole graphic; the side bits follow `VertexSide`. See
// vendor/PROVENANCE.md.

#if defined(GLES2_RENDERER) ///////////////////////////////////////////

precision highp float;

#define LAYOUT_LOCATION(N)      attribute
#define IN
#define FLAT
#define OUT                     varying

#define INT                     float

#define IS_RIGHT_SIDE           (int(sides) == 2 || int(sides) == 3)
#define IS_BOTTOM_SIDE          (int(sides) == 1 || int(sides) == 3)

#else // GLSL3_RENDERER ///////////////////////////////////////////////

#define LAYOUT_LOCATION(N)      layout(location = N)
#define IN                      in
#define FLAT                    flat
#define OUT                     out

#define INT                     int

#define IS_RIGHT_SIDE           ((sides & 2) == 2)
#define IS_BOTTOM_SIDE          ((sides & 1) == 1)

#endif ////////////////////////////////////////////////////////////////

// ------
// INPUTS

// Texture associated to the graphic.
LAYOUT_LOCATION(0) IN INT textureId;

// Sides where the vertex is located.
//
// Bit 0 (LSB) is 0 for top and 1 for bottom.
// Bit 1 is 0 for left and 1 for right.
LAYOUT_LOCATION(1) IN INT sides;

// Destination rectangle, in pixels from the top-left corner of the view.
LAYOUT_LOCATION(2) IN float x;
LAYOUT_LOCATION(3) IN float y;
LAYOUT_LOCATION(4) IN float width;
LAYOUT_LOCATION(5) IN float height;

// Texture coordinates of the left, top, right and bottom edges.
LAYOUT_LOCATION(6) IN float texLeft;
LAYOUT_LOCATION(7) IN float texTop;
LAYOUT_LOCATION(8) IN float texRight;
LAYOUT_LOCATION(9) IN float texBottom;

// -------
// OUTPUTS

// Texture sent to the fragment shader.
FLAT OUT INT texId;

// Coordinates sent to the fragment shader.
OUT vec2 texCoords;

// --------
// UNIFORMS

// Width and height of the view.
uniform vec2 viewDimensions;

void main() {
    vec2 pixel = vec2(x, y);
    vec2 coords = vec2(texLeft, texTop);

    if(IS_RIGHT_SIDE) {
        pixel.x += width;
        coords.x = texRight;
    }

    if(IS_BOTTOM_SIDE) {
        pixel.y += height;
        coords.y = texBottom;
    }

    gl_Position = vec4(
        pixel.x / (viewDimensions.x / 2.) - 1.,
        -pixel.y / (viewDimensions.y / 2.) + 1.,
        0.,
        1.
    );
    texCoords = coords;
    texId = textureId;
}
