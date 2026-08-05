//! Unicode Mathematical Alphanumeric Symbols mapping (U+1D400–U+1D7FF).
//!
//! Maps ASCII Latin letters (and digits) to their styled variants in the
//! Unicode Mathematical Alphanumeric Symbols block, keyed by `MathFontKind`.

use rust_latex_parser::MathFontKind;

/// Convert a character to its mathematical font variant.
/// Returns the original character if no mapping exists.
pub fn map_char(kind: &MathFontKind, ch: char) -> char {
    match kind {
        MathFontKind::Bold => to_bold(ch),
        MathFontKind::Blackboard => to_double_struck(ch),
        MathFontKind::Calligraphic => to_script(ch),
        MathFontKind::Fraktur => to_fraktur(ch),
        MathFontKind::Roman => ch, // upright, no transformation
        MathFontKind::SansSerif => to_sans_serif(ch),
        MathFontKind::Monospace => to_monospace(ch),
    }
}

/// Convert a string by mapping each character through the font transform.
pub fn map_str(kind: &MathFontKind, s: &str) -> String {
    s.chars().map(|c| map_char(kind, c)).collect()
}

// U+1D400 MATHEMATICAL BOLD CAPITAL A .. U+1D419 MATHEMATICAL BOLD CAPITAL Z
// U+1D41A MATHEMATICAL BOLD SMALL A .. U+1D433 MATHEMATICAL BOLD SMALL Z
// U+1D7CE MATHEMATICAL BOLD DIGIT ZERO .. U+1D7D7 MATHEMATICAL BOLD DIGIT NINE
fn to_bold(ch: char) -> char {
    match ch {
        'A'..='Z' => char::from_u32(0x1D400 + (ch as u32 - 'A' as u32)).unwrap_or(ch),
        'a'..='z' => char::from_u32(0x1D41A + (ch as u32 - 'a' as u32)).unwrap_or(ch),
        '0'..='9' => char::from_u32(0x1D7CE + (ch as u32 - '0' as u32)).unwrap_or(ch),
        // Bold Greek uppercase: U+1D6A8–U+1D6C0
        'Α'..='Ω' => char::from_u32(0x1D6A8 + (ch as u32 - 'Α' as u32)).unwrap_or(ch),
        // Bold Greek lowercase: U+1D6C2–U+1D6DA
        'α'..='ω' => char::from_u32(0x1D6C2 + (ch as u32 - 'α' as u32)).unwrap_or(ch),
        _ => ch,
    }
}

// U+1D538 MATHEMATICAL DOUBLE-STRUCK CAPITAL A .. U+1D551
// Exceptions: C=ℂ, H=ℍ, N=ℕ, P=ℙ, Q=ℚ, R=ℝ, Z=ℤ (in Letterlike Symbols block)
// U+1D552 MATHEMATICAL DOUBLE-STRUCK SMALL A .. U+1D56B
// U+1D7D8 MATHEMATICAL DOUBLE-STRUCK DIGIT ZERO .. U+1D7E1
fn to_double_struck(ch: char) -> char {
    match ch {
        'C' => 'ℂ',
        'H' => 'ℍ',
        'N' => 'ℕ',
        'P' => 'ℙ',
        'Q' => 'ℚ',
        'R' => 'ℝ',
        'Z' => 'ℤ',
        'A' | 'B' | 'D'..='G' | 'I'..='M' | 'O' | 'S'..='Y' => {
            char::from_u32(0x1D538 + (ch as u32 - 'A' as u32)).unwrap_or(ch)
        }
        'a'..='z' => char::from_u32(0x1D552 + (ch as u32 - 'a' as u32)).unwrap_or(ch),
        '0'..='9' => char::from_u32(0x1D7D8 + (ch as u32 - '0' as u32)).unwrap_or(ch),
        _ => ch,
    }
}

// U+1D49C MATHEMATICAL SCRIPT CAPITAL A .. U+1D4B5
// Exceptions: B=ℬ, E=ℰ, F=ℱ, H=ℋ, I=ℐ, L=ℒ, M=ℳ, R=ℛ (Letterlike Symbols)
// U+1D4B6 MATHEMATICAL SCRIPT SMALL A .. U+1D4CF
// Exceptions: e=ℯ, g=ℊ, o=ℴ
fn to_script(ch: char) -> char {
    match ch {
        'B' => 'ℬ',
        'E' => 'ℰ',
        'F' => 'ℱ',
        'H' => 'ℋ',
        'I' => 'ℐ',
        'L' => 'ℒ',
        'M' => 'ℳ',
        'R' => 'ℛ',
        'e' => 'ℯ',
        'g' => 'ℊ',
        'o' => 'ℴ',
        'A' | 'C' | 'D' | 'G' | 'J' | 'K' | 'N'..='Q' | 'S'..='Z' => {
            char::from_u32(0x1D49C + (ch as u32 - 'A' as u32)).unwrap_or(ch)
        }
        'a'..='d' | 'f' | 'h'..='n' | 'p'..='z' => {
            char::from_u32(0x1D4B6 + (ch as u32 - 'a' as u32)).unwrap_or(ch)
        }
        _ => ch,
    }
}

// U+1D504 MATHEMATICAL FRAKTUR CAPITAL A .. U+1D51C
// Exceptions: C=ℭ, H=ℌ, I=ℑ, R=ℜ, Z=ℨ
// U+1D51E MATHEMATICAL FRAKTUR SMALL A .. U+1D537
fn to_fraktur(ch: char) -> char {
    match ch {
        'C' => 'ℭ',
        'H' => 'ℌ',
        'I' => 'ℑ',
        'R' => 'ℜ',
        'Z' => 'ℨ',
        'A' | 'B' | 'D'..='G' | 'J'..='Q' | 'S'..='Y' => {
            char::from_u32(0x1D504 + (ch as u32 - 'A' as u32)).unwrap_or(ch)
        }
        'a'..='z' => char::from_u32(0x1D51E + (ch as u32 - 'a' as u32)).unwrap_or(ch),
        _ => ch,
    }
}

// U+1D5A0 MATHEMATICAL SANS-SERIF CAPITAL A .. U+1D5B9
// U+1D5BA MATHEMATICAL SANS-SERIF SMALL A .. U+1D5D3
// U+1D7E2 MATHEMATICAL SANS-SERIF DIGIT ZERO .. U+1D7EB
fn to_sans_serif(ch: char) -> char {
    match ch {
        'A'..='Z' => char::from_u32(0x1D5A0 + (ch as u32 - 'A' as u32)).unwrap_or(ch),
        'a'..='z' => char::from_u32(0x1D5BA + (ch as u32 - 'a' as u32)).unwrap_or(ch),
        '0'..='9' => char::from_u32(0x1D7E2 + (ch as u32 - '0' as u32)).unwrap_or(ch),
        _ => ch,
    }
}

// U+1D670 MATHEMATICAL MONOSPACE CAPITAL A .. U+1D689
// U+1D68A MATHEMATICAL MONOSPACE SMALL A .. U+1D6A3
// U+1D7F6 MATHEMATICAL MONOSPACE DIGIT ZERO .. U+1D7FF
fn to_monospace(ch: char) -> char {
    match ch {
        'A'..='Z' => char::from_u32(0x1D670 + (ch as u32 - 'A' as u32)).unwrap_or(ch),
        'a'..='z' => char::from_u32(0x1D68A + (ch as u32 - 'a' as u32)).unwrap_or(ch),
        '0'..='9' => char::from_u32(0x1D7F6 + (ch as u32 - '0' as u32)).unwrap_or(ch),
        _ => ch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bold() {
        assert_eq!(to_bold('A'), '𝐀');
        assert_eq!(to_bold('Z'), '𝐙');
        assert_eq!(to_bold('a'), '𝐚');
        assert_eq!(to_bold('z'), '𝐳');
        assert_eq!(to_bold('0'), '𝟎');
    }

    #[test]
    fn test_double_struck() {
        assert_eq!(to_double_struck('R'), 'ℝ');
        assert_eq!(to_double_struck('Z'), 'ℤ');
        assert_eq!(to_double_struck('N'), 'ℕ');
        assert_eq!(to_double_struck('C'), 'ℂ');
        assert_eq!(to_double_struck('Q'), 'ℚ');
        // Non-exception uppercase
        assert_eq!(to_double_struck('A'), '𝔸');
    }

    #[test]
    fn test_script() {
        assert_eq!(to_script('L'), 'ℒ');
        assert_eq!(to_script('H'), 'ℋ');
        assert_eq!(to_script('B'), 'ℬ');
        // Non-exception
        assert_eq!(to_script('A'), '𝒜');
    }

    #[test]
    fn test_fraktur() {
        assert_eq!(to_fraktur('H'), 'ℌ');
        assert_eq!(to_fraktur('R'), 'ℜ');
        assert_eq!(to_fraktur('a'), '𝔞');
        assert_eq!(to_fraktur('g'), '𝔤');
    }

    #[test]
    fn test_sans_serif() {
        assert_eq!(to_sans_serif('A'), '𝖠');
        assert_eq!(to_sans_serif('a'), '𝖺');
    }

    #[test]
    fn test_monospace() {
        assert_eq!(to_monospace('A'), '𝙰');
        assert_eq!(to_monospace('a'), '𝚊');
        assert_eq!(to_monospace('0'), '𝟶');
    }

    #[test]
    fn test_non_letter_passthrough() {
        // Non-letter characters should pass through unchanged
        assert_eq!(map_char(&MathFontKind::Bold, '+'), '+');
        assert_eq!(map_char(&MathFontKind::Blackboard, ' '), ' ');
    }
}
