// New file (not from upstream Alacritty or ayosec's graphics patch). See ../../../PROVENANCE.md.
//
// `DIACRITICS` lists the code points of kitty's `gen/rowcolumn-diacritics.txt`
// (https://github.com/kovidgoyal/kitty, commit 7d374531f9adcd0a6df018684e61d9b799ea10e2), in file
// order. Only the code point values are taken (a list of Unicode facts); no kitty code is used.
// The decoding below is written from the protocol specification
// (https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders).

//! Kitty graphics Unicode placeholders (`U+10EEEE` cells): diacritic table and cell decoding.
//!
//! An application draws a virtual placement by writing `U+10EEEE` cells. The cell's foreground
//! colour carries the image id, its underline colour the placement id, and up to three combining
//! diacritics carry the placeholder row, the placeholder column and the most significant byte of
//! the image id. Missing diacritics are inherited from the placeholder cell to the left.

use crate::index::Point;
use crate::vte::ansi::{Color, NamedColor};

/// Character of a placeholder cell.
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// Diacritics encoding the numbers `0..297`: the number is the index in this table.
///
/// The table is sorted by code point, so lookups use a binary search.
#[rustfmt::skip]
pub const DIACRITICS: [char; 297] = [
    '\u{0305}', '\u{030D}', '\u{030E}', '\u{0310}', '\u{0312}', '\u{033D}', '\u{033E}', '\u{033F}',
    '\u{0346}', '\u{034A}', '\u{034B}', '\u{034C}', '\u{0350}', '\u{0351}', '\u{0352}', '\u{0357}',
    '\u{035B}', '\u{0363}', '\u{0364}', '\u{0365}', '\u{0366}', '\u{0367}', '\u{0368}', '\u{0369}',
    '\u{036A}', '\u{036B}', '\u{036C}', '\u{036D}', '\u{036E}', '\u{036F}', '\u{0483}', '\u{0484}',
    '\u{0485}', '\u{0486}', '\u{0487}', '\u{0592}', '\u{0593}', '\u{0594}', '\u{0595}', '\u{0597}',
    '\u{0598}', '\u{0599}', '\u{059C}', '\u{059D}', '\u{059E}', '\u{059F}', '\u{05A0}', '\u{05A1}',
    '\u{05A8}', '\u{05A9}', '\u{05AB}', '\u{05AC}', '\u{05AF}', '\u{05C4}', '\u{0610}', '\u{0611}',
    '\u{0612}', '\u{0613}', '\u{0614}', '\u{0615}', '\u{0616}', '\u{0617}', '\u{0657}', '\u{0658}',
    '\u{0659}', '\u{065A}', '\u{065B}', '\u{065D}', '\u{065E}', '\u{06D6}', '\u{06D7}', '\u{06D8}',
    '\u{06D9}', '\u{06DA}', '\u{06DB}', '\u{06DC}', '\u{06DF}', '\u{06E0}', '\u{06E1}', '\u{06E2}',
    '\u{06E4}', '\u{06E7}', '\u{06E8}', '\u{06EB}', '\u{06EC}', '\u{0730}', '\u{0732}', '\u{0733}',
    '\u{0735}', '\u{0736}', '\u{073A}', '\u{073D}', '\u{073F}', '\u{0740}', '\u{0741}', '\u{0743}',
    '\u{0745}', '\u{0747}', '\u{0749}', '\u{074A}', '\u{07EB}', '\u{07EC}', '\u{07ED}', '\u{07EE}',
    '\u{07EF}', '\u{07F0}', '\u{07F1}', '\u{07F3}', '\u{0816}', '\u{0817}', '\u{0818}', '\u{0819}',
    '\u{081B}', '\u{081C}', '\u{081D}', '\u{081E}', '\u{081F}', '\u{0820}', '\u{0821}', '\u{0822}',
    '\u{0823}', '\u{0825}', '\u{0826}', '\u{0827}', '\u{0829}', '\u{082A}', '\u{082B}', '\u{082C}',
    '\u{082D}', '\u{0951}', '\u{0953}', '\u{0954}', '\u{0F82}', '\u{0F83}', '\u{0F86}', '\u{0F87}',
    '\u{135D}', '\u{135E}', '\u{135F}', '\u{17DD}', '\u{193A}', '\u{1A17}', '\u{1A75}', '\u{1A76}',
    '\u{1A77}', '\u{1A78}', '\u{1A79}', '\u{1A7A}', '\u{1A7B}', '\u{1A7C}', '\u{1B6B}', '\u{1B6D}',
    '\u{1B6E}', '\u{1B6F}', '\u{1B70}', '\u{1B71}', '\u{1B72}', '\u{1B73}', '\u{1CD0}', '\u{1CD1}',
    '\u{1CD2}', '\u{1CDA}', '\u{1CDB}', '\u{1CE0}', '\u{1DC0}', '\u{1DC1}', '\u{1DC3}', '\u{1DC4}',
    '\u{1DC5}', '\u{1DC6}', '\u{1DC7}', '\u{1DC8}', '\u{1DC9}', '\u{1DCB}', '\u{1DCC}', '\u{1DD1}',
    '\u{1DD2}', '\u{1DD3}', '\u{1DD4}', '\u{1DD5}', '\u{1DD6}', '\u{1DD7}', '\u{1DD8}', '\u{1DD9}',
    '\u{1DDA}', '\u{1DDB}', '\u{1DDC}', '\u{1DDD}', '\u{1DDE}', '\u{1DDF}', '\u{1DE0}', '\u{1DE1}',
    '\u{1DE2}', '\u{1DE3}', '\u{1DE4}', '\u{1DE5}', '\u{1DE6}', '\u{1DFE}', '\u{20D0}', '\u{20D1}',
    '\u{20D4}', '\u{20D5}', '\u{20D6}', '\u{20D7}', '\u{20DB}', '\u{20DC}', '\u{20E1}', '\u{20E7}',
    '\u{20E9}', '\u{20F0}', '\u{2CEF}', '\u{2CF0}', '\u{2CF1}', '\u{2DE0}', '\u{2DE1}', '\u{2DE2}',
    '\u{2DE3}', '\u{2DE4}', '\u{2DE5}', '\u{2DE6}', '\u{2DE7}', '\u{2DE8}', '\u{2DE9}', '\u{2DEA}',
    '\u{2DEB}', '\u{2DEC}', '\u{2DED}', '\u{2DEE}', '\u{2DEF}', '\u{2DF0}', '\u{2DF1}', '\u{2DF2}',
    '\u{2DF3}', '\u{2DF4}', '\u{2DF5}', '\u{2DF6}', '\u{2DF7}', '\u{2DF8}', '\u{2DF9}', '\u{2DFA}',
    '\u{2DFB}', '\u{2DFC}', '\u{2DFD}', '\u{2DFE}', '\u{2DFF}', '\u{A66F}', '\u{A67C}', '\u{A67D}',
    '\u{A6F0}', '\u{A6F1}', '\u{A8E0}', '\u{A8E1}', '\u{A8E2}', '\u{A8E3}', '\u{A8E4}', '\u{A8E5}',
    '\u{A8E6}', '\u{A8E7}', '\u{A8E8}', '\u{A8E9}', '\u{A8EA}', '\u{A8EB}', '\u{A8EC}', '\u{A8ED}',
    '\u{A8EE}', '\u{A8EF}', '\u{A8F0}', '\u{A8F1}', '\u{AAB0}', '\u{AAB2}', '\u{AAB3}', '\u{AAB7}',
    '\u{AAB8}', '\u{AABE}', '\u{AABF}', '\u{AAC1}', '\u{FE20}', '\u{FE21}', '\u{FE22}', '\u{FE23}',
    '\u{FE24}', '\u{FE25}', '\u{FE26}', '\u{10A0F}', '\u{10A38}', '\u{1D185}', '\u{1D186}',
    '\u{1D187}', '\u{1D188}', '\u{1D189}', '\u{1D1AA}', '\u{1D1AB}', '\u{1D1AC}', '\u{1D1AD}',
    '\u{1D242}', '\u{1D243}', '\u{1D244}',
];

/// Number encoded by a row, column or image id diacritic.
pub fn diacritic_value(mark: char) -> Option<u32> {
    DIACRITICS.binary_search(&mark).ok().map(|index| index as u32)
}

/// Image tile shown by a placeholder cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaceholderRef {
    /// Image id: the foreground colour, plus the third diacritic as the most significant byte.
    pub image_id: u32,

    /// Placement id from the underline colour; 0 matches any placement of the image.
    pub placement_id: u32,

    /// Row of the tile in the placeholder grid.
    pub row: u32,

    /// Column of the tile in the placeholder grid.
    pub column: u32,
}

/// A decoded placeholder cell, including the raw colours its right neighbour compares against
/// when it inherits missing diacritics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placeholder {
    fg: Color,
    underline: Option<Color>,
    row: u32,
    column: u32,
    image_id_msb: u32,
}

impl Placeholder {
    /// Decode a cell, or `None` when its character is not [`PLACEHOLDER`].
    ///
    /// `fg` and `underline` must be the raw cell colours, before selection, search, inverse or
    /// dim change them. `left` is the decoded placeholder cell directly to the left, if any.
    ///
    /// The leading run of known diacritics in `zerowidth` gives the row, the column and the
    /// image id's most significant byte, in this order; marks after the first unknown one are
    /// ignored. Missing values are inherited from `left` when its colours match, following the
    /// specification:
    ///
    /// - no diacritic: row, column + 1 and most significant byte of `left`;
    /// - row only: column + 1 and most significant byte of `left`, if `left` is on that row;
    /// - row and column: most significant byte of `left`, if `left` is the previous column of that
    ///   row.
    ///
    /// Values that are neither present nor inherited are 0.
    pub fn decode(
        c: char,
        zerowidth: &[char],
        fg: Color,
        underline: Option<Color>,
        left: Option<&Placeholder>,
    ) -> Option<Self> {
        if c != PLACEHOLDER {
            return None;
        }

        // `fuse`: `map_while` resumes after the first unknown mark when polled again.
        let mut values = zerowidth.iter().map_while(|&mark| diacritic_value(mark)).fuse();
        let row = values.next();
        let column = values.next();
        let image_id_msb = values.next();

        let left = left.filter(|left| left.fg == fg && left.underline == underline);
        let inherited = match (left, row, column, image_id_msb) {
            (Some(left), None, ..) => Some((left.row, left.column + 1, left.image_id_msb)),
            (Some(left), Some(row), None, _) if left.row == row => {
                Some((row, left.column + 1, left.image_id_msb))
            },
            (Some(left), Some(row), Some(column), None)
                if left.row == row && left.column + 1 == column =>
            {
                Some((row, column, left.image_id_msb))
            },
            _ => None,
        };

        let (row, column, image_id_msb) =
            inherited.unwrap_or((row.unwrap_or(0), column.unwrap_or(0), image_id_msb.unwrap_or(0)));

        Some(Self { fg, underline, row, column, image_id_msb })
    }

    /// Image tile of this cell, or `None` when the foreground colour encodes no image id.
    pub fn reference(&self) -> Option<PlaceholderRef> {
        let image_id = (self.image_id_msb << 24) | color_id(self.fg)?;
        if image_id == 0 {
            return None;
        }

        let placement_id = self.underline.and_then(color_id).unwrap_or(0);

        Some(PlaceholderRef { image_id, placement_id, row: self.row, column: self.column })
    }
}

/// Number encoded by a colour: 24 bits for a true colour, the index for a 256-colour palette
/// entry (`SGR 30-37`/`90-97` are palette entries 0-15). Default colours encode nothing.
fn color_id(color: Color) -> Option<u32> {
    match color {
        Color::Spec(rgb) => Some(u32::from(rgb.r) << 16 | u32::from(rgb.g) << 8 | u32::from(rgb.b)),
        Color::Indexed(index) => Some(u32::from(index)),
        Color::Named(named) if named <= NamedColor::BrightWhite => Some(named as u32),
        Color::Named(_) => None,
    }
}

/// Decodes the placeholder cells of a frame in grid order, passing each cell the decoded cell
/// directly to its left for inheritance.
#[derive(Debug, Default)]
pub struct PlaceholderDecoder {
    left: Option<(Point, Placeholder)>,
}

impl PlaceholderDecoder {
    /// Decode the cell at `point`. Cells must be fed left to right within a line.
    ///
    /// Returns `None` for cells that are not placeholders and for placeholders without an
    /// image id.
    pub fn decode(
        &mut self,
        point: Point,
        c: char,
        zerowidth: &[char],
        fg: Color,
        underline: Option<Color>,
    ) -> Option<PlaceholderRef> {
        let left = self
            .left
            .as_ref()
            .filter(|(left, _)| left.line == point.line && left.column + 1 == point.column)
            .map(|(_, placeholder)| placeholder);

        let placeholder = Placeholder::decode(c, zerowidth, fg, underline, left);
        self.left = placeholder.map(|placeholder| (point, placeholder));

        placeholder.and_then(|placeholder| placeholder.reference())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{Column, Line};
    use crate::vte::ansi::Rgb;

    const IMAGE: Color = Color::Spec(Rgb { r: 0x12, g: 0x34, b: 0x56 });

    fn decode(zerowidth: &[char], fg: Color, underline: Option<Color>) -> Option<PlaceholderRef> {
        Placeholder::decode(PLACEHOLDER, zerowidth, fg, underline, None)?.reference()
    }

    fn tile(image_id: u32, placement_id: u32, row: u32, column: u32) -> Option<PlaceholderRef> {
        Some(PlaceholderRef { image_id, placement_id, row, column })
    }

    /// Decode one line of cells, as the renderer does.
    fn decode_line(cells: &[(char, &[char], Color)]) -> Vec<Option<PlaceholderRef>> {
        let mut decoder = PlaceholderDecoder::default();
        cells
            .iter()
            .enumerate()
            .map(|(column, &(c, zerowidth, fg))| {
                let point = Point::new(Line(3), Column(column));
                decoder.decode(point, c, zerowidth, fg, None)
            })
            .collect()
    }

    #[test]
    fn table_matches_kitty() {
        assert_eq!(DIACRITICS.len(), 297);
        assert_eq!(DIACRITICS[0], '\u{0305}');
        assert_eq!(DIACRITICS[1], '\u{030D}');
        assert_eq!(DIACRITICS[296], '\u{1D244}');
        assert!(DIACRITICS.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(
            DIACRITICS.iter().enumerate().all(|(i, &mark)| diacritic_value(mark) == Some(i as u32))
        );
    }

    #[test]
    fn row_and_column_at_table_bounds() {
        assert_eq!(
            decode(&[DIACRITICS[0], DIACRITICS[296]], IMAGE, None),
            tile(0x123456, 0, 0, 296)
        );
        assert_eq!(
            decode(&[DIACRITICS[296], DIACRITICS[0]], IMAGE, None),
            tile(0x123456, 0, 296, 0)
        );
    }

    #[test]
    fn unknown_mark_ends_the_diacritics() {
        assert_eq!(diacritic_value('\u{0300}'), None);
        assert_eq!(diacritic_value('a'), None);

        // The column is missing, so it defaults to 0; later marks are ignored.
        let zerowidth = [DIACRITICS[4], '\u{0300}', DIACRITICS[7]];
        assert_eq!(decode(&zerowidth, IMAGE, None), tile(0x123456, 0, 4, 0));
    }

    #[test]
    fn image_id_from_foreground() {
        let marks = [DIACRITICS[1], DIACRITICS[2]];
        assert_eq!(decode(&marks, Color::Spec(Rgb { r: 0, g: 0, b: 9 }), None), tile(9, 0, 1, 2));
        assert_eq!(
            decode(&marks, Color::Spec(Rgb { r: 255, g: 255, b: 255 }), None),
            tile(0xFFFFFF, 0, 1, 2)
        );
        assert_eq!(decode(&marks, Color::Indexed(42), None), tile(42, 0, 1, 2));
        assert_eq!(decode(&marks, Color::Named(NamedColor::Red), None), tile(1, 0, 1, 2));
        assert_eq!(decode(&marks, Color::Named(NamedColor::BrightWhite), None), tile(15, 0, 1, 2));

        // Default colour and id 0 name no image.
        assert_eq!(decode(&marks, Color::Named(NamedColor::Foreground), None), None);
        assert_eq!(decode(&marks, Color::Spec(Rgb { r: 0, g: 0, b: 0 }), None), None);
        assert_eq!(decode(&marks, Color::Indexed(0), None), None);
    }

    #[test]
    fn third_diacritic_is_image_id_high_byte() {
        let marks = [DIACRITICS[0], DIACRITICS[1], DIACRITICS[2]];
        assert_eq!(decode(&marks, Color::Indexed(42), None), tile(42 + (2 << 24), 0, 0, 1));

        let marks = [DIACRITICS[0], DIACRITICS[0], DIACRITICS[255]];
        assert_eq!(decode(&marks, IMAGE, None), tile(0xFF12_3456, 0, 0, 0));

        // An id with only the high byte set is valid.
        let marks = [DIACRITICS[0], DIACRITICS[0], DIACRITICS[1]];
        assert_eq!(decode(&marks, Color::Indexed(0), None), tile(1 << 24, 0, 0, 0));

        // A fourth diacritic is ignored.
        let marks = [DIACRITICS[0], DIACRITICS[0], DIACRITICS[1], DIACRITICS[5]];
        assert_eq!(decode(&marks, Color::Indexed(7), None), tile(7 + (1 << 24), 0, 0, 0));
    }

    #[test]
    fn placement_id_from_underline() {
        let marks = [DIACRITICS[0], DIACRITICS[0]];
        let underline = Color::Spec(Rgb { r: 0, g: 1, b: 2 });
        assert_eq!(decode(&marks, IMAGE, Some(underline)), tile(0x123456, 0x0102, 0, 0));
        assert_eq!(decode(&marks, IMAGE, Some(Color::Indexed(17))), tile(0x123456, 17, 0, 0));
        assert_eq!(
            decode(&marks, IMAGE, Some(Color::Named(NamedColor::Foreground))),
            tile(0x123456, 0, 0, 0)
        );
        assert_eq!(decode(&marks, IMAGE, None), tile(0x123456, 0, 0, 0));
    }

    #[test]
    fn non_placeholder_is_not_decoded() {
        assert_eq!(Placeholder::decode('a', &[DIACRITICS[0]], IMAGE, None, None), None);
    }

    #[test]
    fn inherits_row_and_column_from_left() {
        let row1: &[char] = &[DIACRITICS[1]];
        let id = 0x123456;

        // Spec example: only the first column carries the row diacritic.
        let line = decode_line(&[
            (PLACEHOLDER, row1, IMAGE),
            (PLACEHOLDER, &[], IMAGE),
            (PLACEHOLDER, &[], IMAGE),
        ]);
        assert_eq!(line, vec![tile(id, 0, 1, 0), tile(id, 0, 1, 1), tile(id, 0, 1, 2)]);

        // Row only: the column continues from the left neighbour on the same row.
        let line = decode_line(&[
            (PLACEHOLDER, &[DIACRITICS[1], DIACRITICS[5]], IMAGE),
            (PLACEHOLDER, row1, IMAGE),
        ]);
        assert_eq!(line, vec![tile(id, 0, 1, 5), tile(id, 0, 1, 6)]);

        // Row only, but the left neighbour is on another row: column 0.
        let line = decode_line(&[
            (PLACEHOLDER, &[DIACRITICS[2], DIACRITICS[5]], IMAGE),
            (PLACEHOLDER, row1, IMAGE),
        ]);
        assert_eq!(line, vec![tile(id, 0, 2, 5), tile(id, 0, 1, 0)]);
    }

    #[test]
    fn inherits_image_id_high_byte_from_left() {
        let first: &[char] = &[DIACRITICS[0], DIACRITICS[0], DIACRITICS[3]];
        let fg = Color::Indexed(42);
        let id = 42 + (3 << 24);

        // No diacritics, row only, and row plus the next column all inherit the high byte.
        let line = decode_line(&[
            (PLACEHOLDER, first, fg),
            (PLACEHOLDER, &[], fg),
            (PLACEHOLDER, &[DIACRITICS[0]], fg),
            (PLACEHOLDER, &[DIACRITICS[0], DIACRITICS[3]], fg),
        ]);
        assert_eq!(line, vec![
            tile(id, 0, 0, 0),
            tile(id, 0, 0, 1),
            tile(id, 0, 0, 2),
            tile(id, 0, 0, 3)
        ]);

        // Row and column that do not continue the left neighbour keep high byte 0.
        let line = decode_line(&[
            (PLACEHOLDER, first, fg),
            (PLACEHOLDER, &[DIACRITICS[0], DIACRITICS[5]], fg),
        ]);
        assert_eq!(line, vec![tile(id, 0, 0, 0), tile(42, 0, 0, 5)]);
    }

    #[test]
    fn inheritance_needs_matching_adjacent_placeholder() {
        let first: &[char] = &[DIACRITICS[1], DIACRITICS[4]];
        let other = Color::Indexed(7);

        // A different foreground colour breaks inheritance, even for the same numeric id.
        let line = decode_line(&[
            (PLACEHOLDER, first, Color::Indexed(9)),
            (PLACEHOLDER, &[], Color::Spec(Rgb { r: 0, g: 0, b: 9 })),
        ]);
        assert_eq!(line, vec![tile(9, 0, 1, 4), tile(9, 0, 0, 0)]);

        // A text cell in between breaks inheritance.
        let line = decode_line(&[
            (PLACEHOLDER, first, other),
            ('x', &[], other),
            (PLACEHOLDER, &[], other),
        ]);
        assert_eq!(line, vec![tile(7, 0, 1, 4), None, tile(7, 0, 0, 0)]);

        // A different underline colour breaks inheritance.
        let left = Placeholder::decode(PLACEHOLDER, first, other, None, None).unwrap();
        let cell =
            Placeholder::decode(PLACEHOLDER, &[], other, Some(Color::Indexed(1)), Some(&left));
        assert_eq!(cell.unwrap().reference(), tile(7, 1, 0, 0));

        // The first cell of the next line does not inherit from the end of the previous one.
        let mut decoder = PlaceholderDecoder::default();
        let end = Point::new(Line(0), Column(9));
        assert_eq!(decoder.decode(end, PLACEHOLDER, first, other, None), tile(7, 0, 1, 4));
        let start = Point::new(Line(1), Column(10));
        assert_eq!(decoder.decode(start, PLACEHOLDER, &[], other, None), tile(7, 0, 0, 0));
    }
}
