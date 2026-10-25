use qrcodegen::{QrCode, QrCodeEcc};

const BORDER: i32 = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Glyphs {
    Octant,
    Braille,
}

// Octants only in terminals that draw them themselves (a fallback font draws them uneven); braille elsewhere, though a phone may read its dots as text.
pub fn glyphs_in(env: impl Fn(&str) -> Option<String>) -> Glyphs {
    let set = |name: &str| env(name).is_some_and(|value| !value.is_empty());
    let is = |name: &str, wanted: &str| env(name).is_some_and(|value| value.eq_ignore_ascii_case(wanted));
    match env("KEBBIE_QR").map(|value| value.trim().to_ascii_lowercase()).as_deref() {
        Some("octant") => return Glyphs::Octant,
        Some("braille") => return Glyphs::Braille,
        _ => {}
    }
    let ghostty = set("GHOSTTY_RESOURCES_DIR") || is("TERM_PROGRAM", "ghostty");
    let kitty = set("KITTY_WINDOW_ID") || is("TERM", "xterm-kitty");
    let wezterm = set("WEZTERM_PANE") || is("TERM_PROGRAM", "WezTerm");
    if ghostty || kitty || wezterm { Glyphs::Octant } else { Glyphs::Braille }
}

pub fn glyphs() -> Glyphs {
    glyphs_in(|name| std::env::var(name).ok())
}

// Braille numbers its dots down the left column then the right, with 7 and 8 added under them; cells here go 1 2 / 3 4 / 5 6 / 7 8, bit n-1 for cell n.
pub fn braille(mask: u8) -> char {
    const DOT: [u8; 8] = [0x01, 0x08, 0x02, 0x10, 0x04, 0x20, 0x40, 0x80];
    let dots = (0..8).filter(|cell| mask & 1 << cell != 0).fold(0u32, |dots, cell| dots | u32::from(DOT[cell]));
    char::from_u32(0x2800 + dots).unwrap_or(' ')
}

// Indexed by mask, bit n-1 for cell n: generated once from the Unicode 16 names BLOCK OCTANT-n, the patterns that block leaves out filled from the older block and quadrant characters.
const OCTANTS: [char; 256] = [
    '\u{20}', '\u{1CEA8}', '\u{1CEAB}', '\u{1FB82}', '\u{1CD00}', '\u{2598}', '\u{1CD01}', '\u{1CD02}',
    '\u{1CD03}', '\u{1CD04}', '\u{259D}', '\u{1CD05}', '\u{1CD06}', '\u{1CD07}', '\u{1CD08}', '\u{2580}',
    '\u{1CD09}', '\u{1CD0A}', '\u{1CD0B}', '\u{1CD0C}', '\u{1FBE6}', '\u{1CD0D}', '\u{1CD0E}', '\u{1CD0F}',
    '\u{1CD10}', '\u{1CD11}', '\u{1CD12}', '\u{1CD13}', '\u{1CD14}', '\u{1CD15}', '\u{1CD16}', '\u{1CD17}',
    '\u{1CD18}', '\u{1CD19}', '\u{1CD1A}', '\u{1CD1B}', '\u{1CD1C}', '\u{1CD1D}', '\u{1CD1E}', '\u{1CD1F}',
    '\u{1FBE7}', '\u{1CD20}', '\u{1CD21}', '\u{1CD22}', '\u{1CD23}', '\u{1CD24}', '\u{1CD25}', '\u{1CD26}',
    '\u{1CD27}', '\u{1CD28}', '\u{1CD29}', '\u{1CD2A}', '\u{1CD2B}', '\u{1CD2C}', '\u{1CD2D}', '\u{1CD2E}',
    '\u{1CD2F}', '\u{1CD30}', '\u{1CD31}', '\u{1CD32}', '\u{1CD33}', '\u{1CD34}', '\u{1CD35}', '\u{1FB85}',
    '\u{1CEA3}', '\u{1CD36}', '\u{1CD37}', '\u{1CD38}', '\u{1CD39}', '\u{1CD3A}', '\u{1CD3B}', '\u{1CD3C}',
    '\u{1CD3D}', '\u{1CD3E}', '\u{1CD3F}', '\u{1CD40}', '\u{1CD41}', '\u{1CD42}', '\u{1CD43}', '\u{1CD44}',
    '\u{2596}', '\u{1CD45}', '\u{1CD46}', '\u{1CD47}', '\u{1CD48}', '\u{258C}', '\u{1CD49}', '\u{1CD4A}',
    '\u{1CD4B}', '\u{1CD4C}', '\u{259E}', '\u{1CD4D}', '\u{1CD4E}', '\u{1CD4F}', '\u{1CD50}', '\u{259B}',
    '\u{1CD51}', '\u{1CD52}', '\u{1CD53}', '\u{1CD54}', '\u{1CD55}', '\u{1CD56}', '\u{1CD57}', '\u{1CD58}',
    '\u{1CD59}', '\u{1CD5A}', '\u{1CD5B}', '\u{1CD5C}', '\u{1CD5D}', '\u{1CD5E}', '\u{1CD5F}', '\u{1CD60}',
    '\u{1CD61}', '\u{1CD62}', '\u{1CD63}', '\u{1CD64}', '\u{1CD65}', '\u{1CD66}', '\u{1CD67}', '\u{1CD68}',
    '\u{1CD69}', '\u{1CD6A}', '\u{1CD6B}', '\u{1CD6C}', '\u{1CD6D}', '\u{1CD6E}', '\u{1CD6F}', '\u{1CD70}',
    '\u{1CEA0}', '\u{1CD71}', '\u{1CD72}', '\u{1CD73}', '\u{1CD74}', '\u{1CD75}', '\u{1CD76}', '\u{1CD77}',
    '\u{1CD78}', '\u{1CD79}', '\u{1CD7A}', '\u{1CD7B}', '\u{1CD7C}', '\u{1CD7D}', '\u{1CD7E}', '\u{1CD7F}',
    '\u{1CD80}', '\u{1CD81}', '\u{1CD82}', '\u{1CD83}', '\u{1CD84}', '\u{1CD85}', '\u{1CD86}', '\u{1CD87}',
    '\u{1CD88}', '\u{1CD89}', '\u{1CD8A}', '\u{1CD8B}', '\u{1CD8C}', '\u{1CD8D}', '\u{1CD8E}', '\u{1CD8F}',
    '\u{2597}', '\u{1CD90}', '\u{1CD91}', '\u{1CD92}', '\u{1CD93}', '\u{259A}', '\u{1CD94}', '\u{1CD95}',
    '\u{1CD96}', '\u{1CD97}', '\u{2590}', '\u{1CD98}', '\u{1CD99}', '\u{1CD9A}', '\u{1CD9B}', '\u{259C}',
    '\u{1CD9C}', '\u{1CD9D}', '\u{1CD9E}', '\u{1CD9F}', '\u{1CDA0}', '\u{1CDA1}', '\u{1CDA2}', '\u{1CDA3}',
    '\u{1CDA4}', '\u{1CDA5}', '\u{1CDA6}', '\u{1CDA7}', '\u{1CDA8}', '\u{1CDA9}', '\u{1CDAA}', '\u{1CDAB}',
    '\u{2582}', '\u{1CDAC}', '\u{1CDAD}', '\u{1CDAE}', '\u{1CDAF}', '\u{1CDB0}', '\u{1CDB1}', '\u{1CDB2}',
    '\u{1CDB3}', '\u{1CDB4}', '\u{1CDB5}', '\u{1CDB6}', '\u{1CDB7}', '\u{1CDB8}', '\u{1CDB9}', '\u{1CDBA}',
    '\u{1CDBB}', '\u{1CDBC}', '\u{1CDBD}', '\u{1CDBE}', '\u{1CDBF}', '\u{1CDC0}', '\u{1CDC1}', '\u{1CDC2}',
    '\u{1CDC3}', '\u{1CDC4}', '\u{1CDC5}', '\u{1CDC6}', '\u{1CDC7}', '\u{1CDC8}', '\u{1CDC9}', '\u{1CDCA}',
    '\u{1CDCB}', '\u{1CDCC}', '\u{1CDCD}', '\u{1CDCE}', '\u{1CDCF}', '\u{1CDD0}', '\u{1CDD1}', '\u{1CDD2}',
    '\u{1CDD3}', '\u{1CDD4}', '\u{1CDD5}', '\u{1CDD6}', '\u{1CDD7}', '\u{1CDD8}', '\u{1CDD9}', '\u{1CDDA}',
    '\u{2584}', '\u{1CDDB}', '\u{1CDDC}', '\u{1CDDD}', '\u{1CDDE}', '\u{2599}', '\u{1CDDF}', '\u{1CDE0}',
    '\u{1CDE1}', '\u{1CDE2}', '\u{259F}', '\u{1CDE3}', '\u{2586}', '\u{1CDE4}', '\u{1CDE5}', '\u{2588}',
];

pub fn octant(mask: u8) -> char {
    OCTANTS[usize::from(mask)]
}

// Two modules across and four down a character: on a cell twice as tall as wide every module is square.
pub fn lines(text: &str, glyphs: Glyphs) -> Option<Vec<String>> {
    let code = QrCode::encode_text(text, QrCodeEcc::Low).ok()?;
    let span = -BORDER..code.size() + BORDER;
    let glyph = match glyphs {
        Glyphs::Octant => octant,
        Glyphs::Braille => braille,
    };
    Some(span.clone().step_by(4).map(|y| span.clone().step_by(2).map(|x| {
        let mask = (0..8u8).filter(|cell| code.get_module(x + i32::from(cell % 2), y + i32::from(cell / 2))).fold(0u8, |mask, cell| mask | 1 << cell);
        glyph(mask)
    }).collect()).collect())
}

pub fn truecolor() -> bool {
    std::env::var("COLORTERM").is_ok_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "truecolor" | "24bit"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "https://kebbie.example.com/connect?code=K7QF-M2XD#k=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&m=BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";

    fn mask_of(cells: &str) -> u8 {
        cells.bytes().fold(0, |mask, cell| mask | 1 << (cell - b'1'))
    }

    fn modules(lines: &[String], glyph: fn(u8) -> char) -> Vec<Vec<bool>> {
        let mask = |cell: char| (0..=255u8).find(|mask| glyph(*mask) == cell).unwrap_or_else(|| panic!("{cell:?} is no glyph of the set"));
        lines.iter().flat_map(|line| {
            let masks = line.chars().map(mask).collect::<Vec<_>>();
            (0..4).map(move |row| masks.iter().flat_map(|mask| [mask & 1 << (row * 2) != 0, mask & 1 << (row * 2 + 1) != 0]).collect::<Vec<_>>())
        }).collect()
    }

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs = pairs.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect::<Vec<_>>();
        move |name| pairs.iter().find(|(set, _)| set == name).map(|(_, value)| value.clone())
    }

    #[test]
    fn braille_numbers_its_dots_down_each_column() {
        assert_eq!(braille(0), '\u{2800}');
        assert_eq!(braille(mask_of("1")), '⠁');
        assert_eq!(braille(mask_of("2")), '⠈');
        assert_eq!(braille(mask_of("3")), '⠂');
        assert_eq!(braille(mask_of("78")), '⣀');
        assert_eq!(braille(255), '⣿');
        assert_eq!((0..=255u8).map(braille).collect::<std::collections::HashSet<_>>().len(), 256);
    }

    #[test]
    fn every_pattern_of_eight_modules_has_its_own_octant_and_the_older_blocks_fill_the_ones_the_octant_block_leaves_out() {
        assert_eq!((0..=255u8).map(octant).collect::<std::collections::HashSet<_>>().len(), 256);
        assert!((0..=255u8).all(|mask| octant(mask) != '\0'));
        assert_eq!(octant(mask_of("3")), '\u{1CD00}', "BLOCK OCTANT-3");
        assert_eq!(octant(mask_of("23")), '\u{1CD01}', "BLOCK OCTANT-23");
        assert_eq!(octant(mask_of("1234567")), '\u{1CD70}', "BLOCK OCTANT-1234567");
        assert_eq!(octant(mask_of("1345678")), '\u{1CDE4}', "BLOCK OCTANT-1345678");
        assert_eq!(octant(mask_of("2345678")), '\u{1CDE5}', "BLOCK OCTANT-2345678");
        for (cells, glyph) in [("", ' '), ("12345678", '█'), ("1", '\u{1CEA8}'), ("2", '\u{1CEAB}'), ("7", '\u{1CEA3}'), ("8", '\u{1CEA0}'), ("12", '\u{1FB82}'), ("78", '▂'), ("13", '▘'), ("24", '▝'), ("57", '▖'), ("68", '▗'), ("35", '\u{1FBE6}'), ("46", '\u{1FBE7}'), ("1234", '▀'), ("5678", '▄'), ("1357", '▌'), ("2468", '▐'), ("123456", '\u{1FB85}'), ("345678", '▆'), ("1368", '▚'), ("2457", '▞'), ("123457", '▛'), ("123468", '▜'), ("135678", '▙'), ("245678", '▟')] {
            assert_eq!(octant(mask_of(cells)), glyph, "cells {cells}");
        }
        assert_eq!((0..=255u8).filter(|mask| (0x1CD00..=0x1CDE5).contains(&u32::from(octant(*mask)))).count(), 230, "the rest from the octant block");
    }

    #[test]
    fn octants_where_the_terminal_draws_them_itself_and_braille_everywhere_else_unless_kebbie_qr_says() {
        for (pairs, glyphs) in [
            (&[("TERM_PROGRAM", "ghostty")][..], Glyphs::Octant),
            (&[("TERM_PROGRAM", "herdr"), ("GHOSTTY_RESOURCES_DIR", "/Applications/Ghostty.app/Contents/Resources/ghostty")], Glyphs::Octant),
            (&[("KITTY_WINDOW_ID", "1")], Glyphs::Octant),
            (&[("TERM", "xterm-kitty")], Glyphs::Octant),
            (&[("WEZTERM_PANE", "0")], Glyphs::Octant),
            (&[("TERM_PROGRAM", "WezTerm")], Glyphs::Octant),
            (&[("TERM_PROGRAM", "Apple_Terminal"), ("TERM", "xterm-256color")], Glyphs::Braille),
            (&[("TERM_PROGRAM", "iTerm.app")], Glyphs::Braille),
            (&[("TERM_PROGRAM", "herdr")], Glyphs::Braille),
            (&[("GHOSTTY_RESOURCES_DIR", "")], Glyphs::Braille),
            (&[], Glyphs::Braille),
            (&[("TERM_PROGRAM", "ghostty"), ("KEBBIE_QR", "braille")], Glyphs::Braille),
            (&[("TERM_PROGRAM", "Apple_Terminal"), ("KEBBIE_QR", "Octant")], Glyphs::Octant),
            (&[("TERM_PROGRAM", "ghostty"), ("KEBBIE_QR", "squares")], Glyphs::Octant),
        ] {
            assert_eq!(glyphs_in(env(pairs)), glyphs, "{pairs:?}");
        }
    }

    #[test]
    fn a_link_draws_two_modules_across_and_four_down_with_four_light_modules_around_and_every_module_in_place() {
        let code = QrCode::encode_text(LINK, QrCodeEcc::Low).unwrap();
        let size = code.size() as usize;
        for (glyphs, glyph) in [(Glyphs::Octant, octant as fn(u8) -> char), (Glyphs::Braille, braille)] {
            let drawn = lines(LINK, glyphs).unwrap();
            assert!(drawn.iter().all(|line| line.chars().count() == (size + 8).div_ceil(2)), "two modules a column");
            assert_eq!(drawn.len(), (size + 8).div_ceil(4), "four modules a row");
            assert!(!drawn.concat().contains('\x1b'), "no colour in the glyphs");
            let grid = modules(&drawn, glyph);
            for (y, row) in grid.iter().enumerate() {
                for (x, dark) in row.iter().enumerate() {
                    let inside = (4..size + 4).contains(&x) && (4..size + 4).contains(&y);
                    assert_eq!(*dark, inside && code.get_module(x as i32 - 4, y as i32 - 4), "{glyphs:?} module {x},{y}");
                }
            }
        }
    }
}
