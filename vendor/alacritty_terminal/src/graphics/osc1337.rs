//! Inline images of the [iTerm2 image protocol](https://iterm2.com/documentation-images.html).
//!
//! Derived from `alacritty_terminal/src/graphics/osc1337.rs` of ayosec's experimental
//! `graphics-with-osc1337-v1` tag (https://github.com/ayosec/alacritty, commit
//! `ca3633783e7e1160a61a42c3b27ee537c15493b2`, Apache-2.0). Ported to the current graphics
//! module: the sequence arrives through vte-graphics' `Handler::osc_unhandled`, the image is
//! decoded with resource limits, resized to the requested size before the `GraphicData` is
//! built (the old tag resized in the renderer), and inserted with [`insert_graphic`], so the
//! cursor and scrolling behave as for Sixel.
//!
//! Only `ESC ] 1337 ; File = [arguments] : <base64 file contents> ST` with `inline=1` is
//! supported. Other 1337 commands, non-inline file transfers and the multipart form
//! (`MultipartFile`/`FilePart`/`FileEnd`) are not.
//!
//! The image is decoded synchronously while the terminal is locked. The limits below bound
//! that work: the largest accepted images take about 300 ms and 170 MiB.
//!
//! The decoder, the base64 engine and the scaling helpers are shared with the kitty graphics
//! protocol (`graphics::kitty`).

use std::collections::HashMap;
use std::io::Cursor;
use std::{cmp, fmt, str};

use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use base64::{alphabet, Engine};
use image::{DynamicImage, ImageReader, Limits, RgbaImage};

use crate::event::EventListener;
use crate::graphics::{insert_graphic, ColorType, GraphicData, GraphicId, MAX_GRAPHIC_DIMENSIONS};
use crate::grid::Dimensions;
use crate::term::Term;

/// Largest accepted base64 payload, in bytes (a file of about 24 MiB).
const MAX_PAYLOAD_SIZE: usize = 32 * 1024 * 1024;

/// Largest image accepted from the decoder, in pixels per side. Larger images are rejected
/// before their pixels are allocated. Accepted images larger than [`MAX_GRAPHIC_DIMENSIONS`]
/// are scaled down to fit.
pub(crate) const MAX_DECODED_DIMENSION: u32 = 8192;

/// Maximum memory the image decoder may allocate. The decoded image must fit, which allows up
/// to about 44 megapixels for 8-bit RGB JPEG and PNG (e.g. 7952x5304 photos), 33 for RGBA
/// (including GIF), and half of that for TIFF, whose decoder needs a second full-size buffer.
const MAX_DECODER_ALLOC: u64 = 128 * 1024 * 1024;

/// Standard base64 alphabet; padding is optional, as some producers omit it.
pub(crate) const BASE64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Reason for rejecting an inline image.
#[derive(Debug)]
enum Error {
    /// The `File=` sequence has no `:` separating the arguments from the contents.
    MissingContents,

    /// The contents exceed [`MAX_PAYLOAD_SIZE`].
    PayloadTooLarge(usize),

    /// The contents are not valid base64.
    Base64(base64::DecodeError),

    /// The decoded file is not a supported image, or exceeds the decoder limits.
    Image(image::ImageError),

    /// The requested size is zero, or the terminal has no pixel dimensions.
    InvalidSize,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::MissingContents => f.write_str("missing file contents"),
            Error::PayloadTooLarge(len) => write!(f, "payload of {len} bytes is too large"),
            Error::Base64(err) => write!(f, "invalid base64 data: {err}"),
            Error::Image(err) => write!(f, "can't load image: {err}"),
            Error::InvalidSize => f.write_str("invalid image size"),
        }
    }
}

/// Unit of a requested image dimension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dimension {
    /// Computed from the image size.
    Auto,

    /// Number of grid cells.
    Cells(u32),

    /// Number of pixels.
    Pixels(u32),

    /// Percent of the terminal's width or height.
    Percent(u32),
}

impl Dimension {
    /// Parse `N`, `Npx`, `N%` or `auto`. Unknown values are treated as `auto`.
    fn parse(value: Option<&str>) -> Self {
        let value = match value {
            None | Some("auto") => return Dimension::Auto,
            Some(value) => value,
        };

        let digits = value.bytes().position(|b| !b.is_ascii_digit()).unwrap_or(value.len());
        let (number, unit) = value.split_at(digits);
        let dimension = number.parse().ok().and_then(|number| match unit {
            "" => Some(Dimension::Cells(number)),
            "px" => Some(Dimension::Pixels(number)),
            "%" => Some(Dimension::Percent(number)),
            _ => None,
        });

        dimension.unwrap_or_else(|| {
            log::debug!("Ignoring invalid OSC 1337 image dimension {value:?}");
            Dimension::Auto
        })
    }

    /// Size in pixels, or `None` for [`Dimension::Auto`].
    fn pixels(self, cell_size: f32, terminal_size: f64) -> Option<f64> {
        match self {
            Dimension::Auto => None,
            Dimension::Cells(n) => Some(f64::from(n) * f64::from(cell_size)),
            Dimension::Pixels(n) => Some(f64::from(n)),
            Dimension::Percent(n) => Some(f64::from(n) * terminal_size / 100.),
        }
    }
}

/// Requested size of an inline image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Resize {
    width: Dimension,
    height: Dimension,
    preserve_aspect_ratio: bool,
}

impl Resize {
    fn from_args(args: &HashMap<&str, &str>) -> Self {
        Self {
            width: Dimension::parse(args.get("width").copied()),
            height: Dimension::parse(args.get("height").copied()),
            preserve_aspect_ratio: args.get("preserveAspectRatio") != Some(&"0"),
        }
    }

    /// Compute the image size in pixels, clamped to [`MAX_GRAPHIC_DIMENSIONS`].
    ///
    /// `cell` is the cell size and `terminal` the number of columns and lines.
    fn target_size(
        self,
        image: (u32, u32),
        cell: (f32, f32),
        terminal: (usize, usize),
    ) -> Option<(u32, u32)> {
        let (image_width, image_height) = (f64::from(image.0), f64::from(image.1));
        if image.0 == 0 || image.1 == 0 || cell.0 <= 0. || cell.1 <= 0. {
            return None;
        }

        let terminal_width = terminal.0 as f64 * f64::from(cell.0);
        let terminal_height = terminal.1 as f64 * f64::from(cell.1);
        let width = self.width.pixels(cell.0, terminal_width);
        let height = self.height.pixels(cell.1, terminal_height);

        let (width, height) = match (width, height) {
            (None, None) => (image_width, image_height),
            (Some(width), None) => (width, image_height * width / image_width),
            (None, Some(height)) => (image_width * height / image_height, height),
            (Some(width), Some(height)) if self.preserve_aspect_ratio => {
                // Fit the image inside the requested box.
                let scale = f64::min(width / image_width, height / image_height);
                (image_width * scale, image_height * scale)
            },
            (Some(width), Some(height)) => (width, height),
        };

        if !(width >= 1. && height >= 1.) {
            return None;
        }

        // Scale down to the graphic limits, keeping the computed aspect ratio.
        let scale = f64::min(
            1.,
            f64::min(
                MAX_GRAPHIC_DIMENSIONS[0] as f64 / width,
                MAX_GRAPHIC_DIMENSIONS[1] as f64 / height,
            ),
        );

        // Both values are in `1..=MAX_GRAPHIC_DIMENSIONS`, so the casts are lossless.
        let width = (width * scale).round().clamp(1., MAX_GRAPHIC_DIMENSIONS[0] as f64);
        let height = (height * scale).round().clamp(1., MAX_GRAPHIC_DIMENSIONS[1] as f64);
        Some((width as u32, height as u32))
    }
}

/// Handle an OSC 1337 sequence.
///
/// Returns `false` if it is not a `File=` command, so the caller can log it as unhandled.
pub fn handle<L: EventListener>(term: &mut Term<L>, params: &[&[u8]]) -> bool {
    if !params.get(1).is_some_and(|param| param.starts_with(b"File=")) {
        return false;
    }

    let (args, contents) = match file_args(params) {
        Some(parsed) => parsed,
        None => {
            log::warn!("Ignoring OSC 1337 image: {}", Error::MissingContents);
            return true;
        },
    };

    if args.get("inline") != Some(&"1") {
        log::debug!("Ignoring OSC 1337 file transfer without inline=1");
        return true;
    }

    let cell = (term.graphics.cell_width, term.graphics.cell_height);
    let terminal = (term.columns(), term.screen_lines());
    match load_graphic(contents, Resize::from_args(&args), cell, terminal) {
        Ok(graphic) => insert_graphic(term, graphic, None),
        Err(err) => log::warn!("Ignoring OSC 1337 image: {err}"),
    }

    true
}

/// Split the arguments and the file contents.
///
/// iTerm2's format is not expected by vte: arguments are separated by `;`, so vte splits them
/// into parameters. The first one starts with `File=`, and the last one has the contents after
/// the first `:`.
///
/// vte keeps up to 16 parameters; if a sequence has more, the contents are lost and this returns
/// `None`.
fn file_args<'a>(params: &[&'a [u8]]) -> Option<(HashMap<&'a str, &'a str>, &'a [u8])> {
    let args = params.get(1..).filter(|args| !args.is_empty())?;
    let last = args.len() - 1;

    let mut map = HashMap::new();
    let mut contents = None;
    for (index, &arg) in args.iter().enumerate() {
        let mut arg = if index == 0 { arg.strip_prefix(b"File=")? } else { arg };

        if index == last {
            let separator = arg.iter().position(|&b| b == b':')?;
            contents = Some(&arg[separator + 1..]);
            arg = &arg[..separator];
        }

        let Some(separator) = arg.iter().position(|&b| b == b'=') else { continue };
        if let (Ok(key), Ok(value)) =
            (str::from_utf8(&arg[..separator]), str::from_utf8(&arg[separator + 1..]))
        {
            map.insert(key, value);
        }
    }

    contents.map(|contents| (map, contents))
}

/// Decode the file contents and build a graphic of the requested size.
fn load_graphic(
    contents: &[u8],
    resize: Resize,
    cell: (f32, f32),
    terminal: (usize, usize),
) -> Result<GraphicData, Error> {
    if contents.len() > MAX_PAYLOAD_SIZE {
        return Err(Error::PayloadTooLarge(contents.len()));
    }

    let file = BASE64.decode(contents).map_err(Error::Base64)?;
    let image = decode_image(&file).map_err(Error::Image)?;
    drop(file);

    let (width, height) = resize
        .target_size((image.width(), image.height()), cell, terminal)
        .ok_or(Error::InvalidSize)?;

    // Shrink before converting to RGBA, so that conversion never works on more pixels than the
    // target; enlargement allocates only its output.
    let shrunk = (cmp::min(image.width(), width), cmp::min(image.height(), height));
    let image = shrink(image, shrunk.0, shrunk.1);
    let image =
        if image.dimensions() == (width, height) { image } else { enlarge(&image, width, height) };

    let pixels = image.into_raw();
    let is_opaque = pixels.chunks_exact(4).all(|pixel| pixel[3] == u8::MAX);

    Ok(GraphicData {
        id: GraphicId(0),
        width: width as usize,
        height: height as usize,
        color_type: ColorType::Rgba,
        pixels,
        is_opaque,
    })
}

/// Scale `image` down to `width`x`height` (neither may be larger than the image) and convert it
/// to RGBA, releasing the original.
fn shrink(image: DynamicImage, width: u32, height: u32) -> RgbaImage {
    if (image.width(), image.height()) == (width, height) {
        return image.into_rgba8();
    }

    let dimensions = (image.width(), image.height());
    match &image {
        DynamicImage::ImageRgb8(buffer) => {
            box_shrink(buffer.as_raw(), 3, dimensions, width, height)
        },
        DynamicImage::ImageRgba8(buffer) => {
            box_shrink(buffer.as_raw(), 4, dimensions, width, height)
        },
        _ => image.thumbnail_exact(width, height).into_rgba8(),
    }
}

/// Scale 8-bit RGB or RGBA pixels down to `width`x`height` RGBA, averaging the source pixels of
/// each output pixel.
///
/// The common decoder outputs take this path instead of `imageops::thumbnail`, which is about
/// three times slower for RGB, and the input is never converted to RGBA at full size.
pub(crate) fn box_shrink(
    pixels: &[u8],
    channels: usize,
    (source_width, source_height): (u32, u32),
    width: u32,
    height: u32,
) -> RgbaImage {
    /// Source range of every output position.
    fn spans(source: u32, target: u32) -> Vec<(usize, usize)> {
        let span = |index: u32| (u64::from(index) * u64::from(source) / u64::from(target)) as usize;
        (0..target).map(|index| (span(index), span(index + 1))).collect()
    }

    let columns = spans(source_width, width);
    let stride = source_width as usize * channels;
    let mut sums = vec![0u64; width as usize * 4];

    let mut output = RgbaImage::new(width, height);
    for (row, (top, bottom)) in output.rows_mut().zip(spans(source_height, height)) {
        sums.fill(0);
        for source_row in pixels[top * stride..bottom * stride].chunks_exact(stride) {
            for (sum, &(left, right)) in sums.chunks_exact_mut(4).zip(&columns) {
                for pixel in source_row[left * channels..right * channels].chunks_exact(channels) {
                    sum[0] += u64::from(pixel[0]);
                    sum[1] += u64::from(pixel[1]);
                    sum[2] += u64::from(pixel[2]);
                    sum[3] += u64::from(pixel.get(3).copied().unwrap_or(u8::MAX));
                }
            }
        }

        for ((pixel, sum), &(left, right)) in row.zip(sums.chunks_exact(4)).zip(&columns) {
            let count = ((right - left) * (bottom - top)) as u64;
            pixel.0 = [0, 1, 2, 3].map(|channel| ((sum[channel] + count / 2) / count) as u8);
        }
    }

    output
}

/// Scale `image` up to `width`x`height` (neither may be smaller than the image) with bilinear
/// interpolation.
///
/// This is used instead of `imageops::resize`, which allocates an intermediate RGBA `f32`
/// buffer of the source width times the target height (up to 256 MiB here).
pub(crate) fn enlarge(image: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    /// Source pixels, and weight of the second one out of 256, for an output position, aligning
    /// pixel centers.
    fn taps(position: u32, source: u32, target: u32) -> (usize, usize, u32) {
        let scale = source as f32 / target as f32;
        let center = ((position as f32 + 0.5) * scale - 0.5).max(0.);
        let first = cmp::min(center as u32, source - 1);
        let weight = ((center - first as f32) * 256.).round() as u32;
        (first as usize, cmp::min(first + 1, source - 1) as usize, cmp::min(weight, 256))
    }

    let (source_width, source_height) = image.dimensions();
    let columns: Vec<_> = (0..width).map(|x| taps(x, source_width, width)).collect();
    let stride = source_width as usize * 4;
    let source = image.as_raw();

    let mut output = RgbaImage::new(width, height);
    for (y, row) in output.rows_mut().enumerate() {
        let (top, bottom, weight_y) = taps(y as u32, source_height, height);
        let top = &source[top * stride..][..stride];
        let bottom = &source[bottom * stride..][..stride];

        for (pixel, &(left, right, weight_x)) in row.zip(&columns) {
            let weights = [
                (256 - weight_x) * (256 - weight_y),
                weight_x * (256 - weight_y),
                (256 - weight_x) * weight_y,
                weight_x * weight_y,
            ];
            let samples =
                [&top[left * 4..], &top[right * 4..], &bottom[left * 4..], &bottom[right * 4..]];
            for channel in 0..4 {
                let value: u32 = samples
                    .iter()
                    .zip(weights)
                    .map(|(sample, weight)| u32::from(sample[channel]) * weight)
                    .sum();
                pixel.0[channel] = ((value + (1 << 15)) >> 16) as u8;
            }
        }
    }

    output
}

/// Decode an image file with resource limits.
pub(crate) fn decode_image(file: &[u8]) -> image::ImageResult<DynamicImage> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODED_DIMENSION);
    limits.max_image_height = Some(MAX_DECODED_DIMENSION);
    limits.max_alloc = Some(MAX_DECODER_ALLOC);

    let mut reader = ImageReader::new(Cursor::new(file)).with_guessed_format()?;
    reader.limits(limits);
    reader.decode()
}

#[cfg(test)]
mod tests {
    use super::*;

    use image::{ImageFormat, Rgb, RgbImage, Rgba};

    use crate::event::VoidListener;
    use crate::index::{Column, Line};
    use crate::term::Config;
    use crate::vte::ansi;

    const CELL: (f32, f32) = (10., 20.);

    /// Terminal size with pixel dimensions.
    struct PixelSize {
        columns: usize,
        lines: usize,
    }

    impl Dimensions for PixelSize {
        fn total_lines(&self) -> usize {
            self.lines
        }

        fn screen_lines(&self) -> usize {
            self.lines
        }

        fn columns(&self) -> usize {
            self.columns
        }

        fn cell_width(&self) -> f32 {
            CELL.0
        }

        fn cell_height(&self) -> f32 {
            CELL.1
        }
    }

    fn term() -> Term<VoidListener> {
        Term::new(Config::default(), &PixelSize { columns: 80, lines: 24 }, VoidListener)
    }

    fn encode(width: u32, height: u32, format: ImageFormat) -> String {
        let image = RgbaImage::from_fn(width, height, |x, y| Rgba([x as u8, y as u8, 0x80, 0xff]));
        let mut file = Cursor::new(Vec::new());
        image.write_to(&mut file, format).unwrap();
        BASE64.encode(file.into_inner())
    }

    fn feed(term: &mut Term<VoidListener>, bytes: &[u8]) {
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(term, bytes);
    }

    fn graphic_count(term: &Term<VoidListener>) -> usize {
        let grid = term.grid();
        (0..grid.screen_lines() as i32)
            .flat_map(|line| (0..grid.columns()).map(move |column| (line, column)))
            .filter(|&(line, column)| grid[Line(line)][Column(column)].graphics().is_some())
            .count()
    }

    #[test]
    fn parse_parameters() {
        let params = [
            b"1337".as_ref(),
            b"File=name=dGVzdC5wbmc=".as_ref(),
            b"size=3".as_ref(),
            b"inline=1:AAAA".as_ref(),
        ];

        let (args, contents) = file_args(&params).unwrap();
        assert_eq!(args["name"], "dGVzdC5wbmc=");
        assert_eq!(args["size"], "3");
        assert_eq!(args["inline"], "1");
        assert_eq!(contents, b"AAAA");
    }

    #[test]
    fn parse_single_parameter() {
        let params = [b"1337".as_ref(), b"File=inline=1:AAAA".as_ref()];

        let (args, contents) = file_args(&params).unwrap();
        assert_eq!(args["inline"], "1");
        assert_eq!(contents, b"AAAA");
    }

    #[test]
    fn parse_without_contents() {
        assert!(file_args(&[b"1337".as_ref(), b"File=inline=1".as_ref()]).is_none());
        assert!(file_args(&[b"1337".as_ref(), b"inline=1:AAAA".as_ref()]).is_none());
        assert!(file_args(&[b"1337".as_ref()]).is_none());
    }

    #[test]
    fn parse_dimensions() {
        assert_eq!(Dimension::parse(None), Dimension::Auto);
        assert_eq!(Dimension::parse(Some("auto")), Dimension::Auto);
        assert_eq!(Dimension::parse(Some("10")), Dimension::Cells(10));
        assert_eq!(Dimension::parse(Some("50px")), Dimension::Pixels(50));
        assert_eq!(Dimension::parse(Some("25%")), Dimension::Percent(25));
        assert_eq!(Dimension::parse(Some("10em")), Dimension::Auto);
        assert_eq!(Dimension::parse(Some("99999999999")), Dimension::Auto);
        assert_eq!(Dimension::parse(Some("")), Dimension::Auto);

        let args = HashMap::from([("width", "30"), ("height", "10"), ("preserveAspectRatio", "0")]);
        assert_eq!(Resize::from_args(&args), Resize {
            width: Dimension::Cells(30),
            height: Dimension::Cells(10),
            preserve_aspect_ratio: false,
        });
    }

    #[test]
    fn resize_units() {
        use Dimension::{Auto, Cells, Percent, Pixels};

        let size = |width, height, preserve_aspect_ratio| {
            Resize { width, height, preserve_aspect_ratio }.target_size((200, 100), CELL, (80, 24))
        };

        assert_eq!(size(Auto, Auto, true), Some((200, 100)));
        assert_eq!(size(Cells(30), Cells(10), false), Some((300, 200)));
        assert_eq!(size(Cells(30), Cells(10), true), Some((300, 150)));
        assert_eq!(size(Pixels(50), Auto, true), Some((50, 25)));
        assert_eq!(size(Auto, Pixels(50), true), Some((100, 50)));
        assert_eq!(size(Percent(50), Auto, true), Some((400, 200)));
        assert_eq!(size(Auto, Percent(50), false), Some((480, 240)));
        assert_eq!(size(Cells(20), Auto, false), Some((200, 100)));

        // Clamped to the graphic limits, keeping the aspect ratio.
        assert_eq!(size(Pixels(100_000), Auto, true), Some((4096, 2048)));
        assert_eq!(size(Pixels(100_000), Pixels(10), false), Some((4096, 1)));

        // Empty sizes and terminals without pixel dimensions are rejected.
        assert_eq!(size(Pixels(0), Auto, true), None);
        assert_eq!(size(Cells(0), Cells(3), false), None);
        let resize = Resize { width: Auto, height: Auto, preserve_aspect_ratio: true };
        assert_eq!(resize.target_size((200, 100), (0., 0.), (80, 24)), None);
    }

    #[test]
    fn inline_png() {
        let mut term = term();
        let sequence =
            format!("\x1b]1337;File=size=1;inline=1:{}\x07", encode(4, 3, ImageFormat::Png));
        feed(&mut term, sequence.as_bytes());

        let cell = term.grid()[Line(0)][Column(0)].graphics().expect("graphic in first cell");
        assert_eq!(cell[0].offset_x, 0);
        assert_eq!(cell[0].offset_y, 0);
        assert_eq!(graphic_count(&term), 1);
        assert_eq!(term.grid().cursor.point, crate::index::Point::new(Line(1), Column(0)));

        let queues = term.graphics_take_queues().expect("pending graphic");
        assert_eq!(queues.pending.len(), 1);
        let graphic = &queues.pending[0];
        assert_eq!((graphic.width, graphic.height), (4, 3));
        assert_eq!(graphic.color_type, ColorType::Rgba);
        assert_eq!(graphic.pixels.len(), 4 * 3 * 4);
        assert!(graphic.is_opaque);
    }

    #[test]
    fn inline_png_resized_in_cells() {
        let mut term = term();
        let sequence = format!(
            "\x1b]1337;File=inline=1;width=3;height=2;preserveAspectRatio=0:{}\x1b\\",
            encode(4, 3, ImageFormat::Png)
        );
        feed(&mut term, sequence.as_bytes());

        // 3x2 cells.
        assert_eq!(graphic_count(&term), 6);
        let graphic = &term.graphics_take_queues().unwrap().pending[0];
        assert_eq!((graphic.width, graphic.height), (30, 40));
    }

    #[test]
    fn inline_tiff_from_chafa() {
        // Produced by `chafa -f iterm -s 4x2`: an uncompressed TIFF with
        // `width=4;height=2;preserveAspectRatio=0`, preceded by `CSI ? 25 l`.
        let sequence = include_bytes!("../../tests/iterm2/chafa.iterm");
        let mut term = term();
        feed(&mut term, sequence);

        assert_eq!(graphic_count(&term), 4 * 2);
        let graphic = &term.graphics_take_queues().unwrap().pending[0];
        assert_eq!((graphic.width, graphic.height), (40, 40));
        assert_eq!(graphic.pixels.len(), 40 * 40 * 4);
    }

    #[test]
    fn inline_tiff() {
        let mut term = term();
        let sequence = format!("\x1b]1337;File=inline=1:{}\x07", encode(5, 5, ImageFormat::Tiff));
        feed(&mut term, sequence.as_bytes());
        assert_eq!(graphic_count(&term), 1);
    }

    #[test]
    fn rejected_sequences() {
        let png = encode(4, 3, ImageFormat::Png);
        let sequences = [
            // Not inline.
            format!("\x1b]1337;File=name=eA==:{png}\x07"),
            // Other 1337 commands.
            "\x1b]1337;SetMark\x07".into(),
            format!("\x1b]1337;MultipartFile=inline=1;size=4:{png}\x07"),
            // Missing contents.
            "\x1b]1337;File=inline=1\x07".into(),
            // Invalid base64.
            "\x1b]1337;File=inline=1:****\x07".into(),
            // Valid base64, not an image.
            format!("\x1b]1337;File=inline=1:{}\x07", BASE64.encode(b"not an image")),
            // Truncated image.
            format!("\x1b]1337;File=inline=1:{}\x07", &png[..png.len() / 2]),
            // Empty requested size.
            format!("\x1b]1337;File=inline=1;width=0:{png}\x07"),
        ];

        let mut term = term();
        for sequence in &sequences {
            feed(&mut term, sequence.as_bytes());
        }

        assert_eq!(graphic_count(&term), 0);
        assert!(term.graphics_take_queues().is_none());
    }

    #[test]
    fn oversized_images_are_rejected() {
        // A PNG header claiming 100000x100000 pixels must fail on the decoder limits, before
        // any pixel buffer is allocated.
        let png = png_header(100_000, 100_000);

        let err = decode_image(&png).unwrap_err();
        assert!(matches!(err, image::ImageError::Limits(_)), "{err:?}");

        let mut term = term();
        let sequence = format!("\x1b]1337;File=inline=1:{}\x07", BASE64.encode(&png));
        feed(&mut term, sequence.as_bytes());
        assert_eq!(graphic_count(&term), 0);

        // Payload above the size limit is rejected before decoding.
        let payload = vec![b'A'; MAX_PAYLOAD_SIZE + 4];
        let resize = Resize::from_args(&HashMap::new());
        let err = load_graphic(&payload, resize, CELL, (80, 24)).unwrap_err();
        assert!(matches!(err, Error::PayloadTooLarge(_)));
    }

    #[test]
    fn decoder_allocation_limit() {
        // 8192x8192 RGBA is within the per-side limit, but its 256 MiB exceed the allocation
        // limit.
        let png = png_header(8192, 8192);
        let err = decode_image(&png).unwrap_err();
        assert!(matches!(err, image::ImageError::Limits(_)), "{err:?}");
    }

    #[test]
    fn large_images_are_scaled_down() {
        let mut term = term();
        let sequence =
            format!("\x1b]1337;File=inline=1:{}\x07", encode(5000, 100, ImageFormat::Png));
        feed(&mut term, sequence.as_bytes());

        let graphic = &term.graphics_take_queues().unwrap().pending[0];
        assert_eq!((graphic.width, graphic.height), (4096, 82));
    }

    #[test]
    fn shrink_averages() {
        let red = [0, 10, 20, 30, 40, 50, 60, 70];
        let rgb = RgbImage::from_fn(4, 2, |x, y| Rgb([red[(y * 4 + x) as usize], 1, 2]));
        let shrunk = shrink(DynamicImage::ImageRgb8(rgb), 2, 1);
        assert_eq!(shrunk.as_raw(), &[25, 1, 2, 255, 45, 1, 2, 255]);

        let rgba = RgbaImage::from_fn(3, 1, |x, _| Rgba([0, 0, 0, [0, 90, 255][x as usize]]));
        let shrunk = shrink(DynamicImage::ImageRgba8(rgba), 1, 1);
        assert_eq!(shrunk.as_raw(), &[0, 0, 0, 115]);

        // Other color types use `thumbnail`.
        let luma = image::GrayImage::from_pixel(4, 4, image::Luma([7]));
        let shrunk = shrink(DynamicImage::ImageLuma8(luma), 2, 2);
        assert!(shrunk.pixels().all(|pixel| pixel.0 == [7, 7, 7, 255]));
    }

    #[test]
    fn enlarge_interpolates() {
        let image = RgbaImage::from_raw(2, 1, vec![0, 0, 0, 255, 200, 100, 40, 255]).unwrap();

        let enlarged = enlarge(&image, 4, 2);
        let reds: Vec<_> = enlarged.pixels().map(|pixel| pixel[0]).collect();
        assert_eq!(reds, [0, 50, 150, 200, 0, 50, 150, 200]);
        assert!(enlarged.pixels().all(|pixel| pixel[3] == 255));

        let single = RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 4]));
        assert!(enlarge(&single, 3, 5).pixels().all(|pixel| pixel.0 == [1, 2, 3, 4]));
    }

    #[test]
    fn removed_graphics_are_not_kept_pending() {
        // A session that is not drawn never takes the queues. Graphics erased from the grid
        // must not accumulate.
        let png = encode(4, 3, ImageFormat::Png);
        // 10x30 cells, taller than the terminal, so part of it scrolls into the history.
        let iterm = format!(
            "\x1b]1337;File=inline=1;width=100px;height=600px;preserveAspectRatio=0:{png}\x07"
        );
        let sixel = "\x1bPq#0;2;100;0;0#0~~~~-~~~~\x1b\\";

        let mut term = term();
        for _ in 0..50 {
            feed(&mut term, iterm.as_bytes());
            // Inserting prunes the operations on the graphics erased before.
            assert!(term.graphics.texture_operations.lock().is_empty());
            feed(&mut term, sixel.as_bytes());
            feed(&mut term, b"\x1b[2J\x1b[3J");
            assert!(term.graphics.pending.len() <= 2, "{} pending", term.graphics.pending.len());
        }

        assert_eq!(graphic_count(&term), 0);
        assert!(term.graphics_take_queues().is_none());

        // Graphics still on the grid stay pending.
        feed(&mut term, iterm.as_bytes());
        feed(&mut term, iterm.as_bytes());
        let queues = term.graphics_take_queues().unwrap();
        assert_eq!(queues.pending.len(), 2);
    }

    /// A PNG file with an RGBA `IHDR` chunk and no image data.
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let ihdr = [&width.to_be_bytes()[..], &height.to_be_bytes(), &[8, 6, 0, 0, 0]].concat();

        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        for (kind, data) in [(b"IHDR", &ihdr[..]), (b"IDAT", &[]), (b"IEND", &[])] {
            png.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let chunk = [&kind[..], data].concat();
            png.extend_from_slice(&chunk);
            png.extend_from_slice(&crc32(&chunk).to_be_bytes());
        }
        png
    }

    /// CRC-32 (IEEE) for PNG chunks.
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
}
