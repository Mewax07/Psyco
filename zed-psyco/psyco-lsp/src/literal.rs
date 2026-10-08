//! Hover text for integer and character literals.

/// Groups digits from the right: `11110000` -> `1111_0000`.
fn group(digits: &str, size: usize) -> String {
    let chars: Vec<char> = digits.chars().collect();
    let mut out = String::new();
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && (chars.len() - i) % size == 0 {
            out.push('_');
        }
        out.push(*c);
    }
    out
}

fn bits_of(suffix: &str) -> Option<u32> {
    Some(match suffix {
        "u8" | "i8" => 8,
        "u16" | "i16" => 16,
        "u32" | "i32" => 32,
        "u64" | "i64" | "usize" | "isize" => 64,
        _ => return None,
    })
}

fn char_name(c: char) -> Option<String> {
    Some(match c {
        '\0' => "'\0' (NUL)".into(),
        '\t' => "'\t' (tab)".into(),
        '\n' => "'\n' (line feed)".into(),
        '\r' => "'\r' (carriage return)".into(),
        '\x08' => "backspace".into(),
        '\x1b' => "escape".into(),
        ' ' => "' ' (space)".into(),
        '\x7f' => "delete".into(),
        c if c.is_control() => return None,
        c => format!("'{c}'"),
    })
}

/// Markdown for a literal with value `value`, an optional type suffix, and
/// whether it was written as a character (`'a'`).
pub fn describe(value: u64, suffix: Option<&str>, is_char: bool) -> String {
    let bits = suffix.and_then(bits_of);
    // Pad to the width of the type when it is known.
    let width = |base_bits: u32| {
        bits.map_or(0, |b| b.div_ceil(base_bits) as usize)
    };
    let hex = format!("{value:0w$X}", w = width(4));
    let bin = format!("{value:0w$b}", w = width(1).max(1));
    let bin = if bits.is_none() && bin.len() % 4 != 0 {
        format!("{value:0w$b}", w = bin.len().div_ceil(4) * 4)
    } else {
        bin
    };

    let mut rows = vec![
        ("dec", value.to_string()),
        ("hex", format!("0x{}", group(&hex, 4))),
        ("bin", format!("0b{}", group(&bin, 4))),
        ("oct", format!("0o{value:o}")),
    ];
    if let Some(b) = bits {
        // Same bits read as signed / unsigned.
        let signed = suffix.is_some_and(|s| s.starts_with('i'));
        let top = 1u64 << (b - 1);
        if value & top != 0 && b < 64 && !signed {
            let v = value as i64 - (1i64 << b);
            rows.push(("as signed", format!("{v}i{b}")));
        } else if value & top != 0 && b == 64 && !signed {
            rows.push(("as signed", format!("{}i64", value as i64)));
        }
    }
    if let Some(c) = char::from_u32(value as u32).filter(|_| value <= u32::MAX as u64) {
        if let Some(name) = char_name(c) {
            rows.push(("char", format!("{name}  U+{:04X}", value)));
        }
    }

    let title = match (is_char, suffix) {
        (true, _) => "character literal".to_string(),
        (false, Some(s)) => format!("integer literal `{s}`"),
        (false, None) => "integer literal".to_string(),
    };
    let pad = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    let table: Vec<String> = rows
        .into_iter()
        .map(|(k, v)| format!("{k:<pad$}  {v}"))
        .collect();
    format!("*{title}*\n\n```text\n{}\n```", table.join("\n"))
}
