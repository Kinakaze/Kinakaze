//! Exact x87 formatting: retain the 64-bit significand and full exponent range.
use super::{Sink, Spec};
use rustc_apfloat::{Float, ieee::X87DoubleExtended};

fn rounding_up(negative: bool, nonzero: bool, half: core::cmp::Ordering, odd: bool) -> bool {
    if !nonzero {
        return false;
    }
    let mut control = 0u16;
    unsafe {
        core::arch::asm!("fnstcw [{}]", in(reg) &mut control, options(nostack, preserves_flags));
    }
    match (control >> 10) & 3 {
        1 => negative,
        2 => !negative,
        3 => false,
        _ => half.is_gt() || half.is_eq() && odd,
    }
}

// APFloat's bounded x87 domain has fewer than 20,000 exact decimal digits.
// Asking for all of them avoids its own significant-digit rounding; rounding
// below then honors C's current direction and ties-to-even rule exactly once.
fn decimal(bits: u128) -> (Vec<u8>, i32) {
    let value = X87DoubleExtended::from_bits(bits & !(1u128 << 79));
    if value.is_zero() {
        return (vec![b'0'], 1);
    }
    let text = format!("{value:width$.precision$}", width = 0, precision = 20_000);
    let (digits, exponent) = text
        .split_once('E')
        .expect("APFloat scientific representation");
    (
        digits.bytes().filter(|b| *b != b'.').collect(),
        exponent.parse::<i32>().unwrap() + 1,
    )
}

fn round(digits: &mut Vec<u8>, point: &mut i32, keep: i32, negative: bool) {
    if keep >= digits.len() as i32 {
        return;
    }
    let at = keep.max(0) as usize;
    let tail = &digits[at..];
    let nonzero = tail.iter().any(|b| *b != b'0');
    let half = if keep < 0 {
        core::cmp::Ordering::Less
    } else {
        tail[0].cmp(&b'5').then_with(|| {
            if tail[1..].iter().any(|b| *b != b'0') {
                core::cmp::Ordering::Greater
            } else {
                core::cmp::Ordering::Equal
            }
        })
    };
    let up = rounding_up(
        negative,
        nonzero,
        half,
        at > 0 && (digits[at - 1] - b'0') % 2 != 0,
    );
    if keep <= 0 {
        *point = if up { *point - keep + 1 } else { 1 };
        *digits = vec![if up { b'1' } else { b'0' }];
        return;
    }
    digits.truncate(at);
    if up {
        for digit in digits.iter_mut().rev() {
            if *digit != b'9' {
                *digit += 1;
                return;
            }
            *digit = b'0';
        }
        digits.insert(0, b'1');
        *point += 1;
    }
}

fn fixed(digits: &[u8], point: i32, precision: usize, alternate: bool) -> String {
    let digit = |at: i64| {
        if at < 0 {
            b'0'
        } else {
            digits.get(at as usize).copied().unwrap_or(b'0')
        }
    };
    let mut text = String::new();
    if point <= 0 {
        text.push('0');
    } else {
        for at in 0..point {
            text.push(digit(at as i64) as char);
        }
    }
    if precision > 0 || alternate {
        text.push('.');
    }
    for at in 0..precision {
        text.push(digit(point as i64 + at as i64) as char);
    }
    text
}

fn scientific(digits: &[u8], point: i32, precision: usize, alternate: bool, upper: bool) -> String {
    let mut text = fixed(digits, 1, precision, alternate);
    let exponent = if digits.iter().all(|b| *b == b'0') {
        0
    } else {
        point - 1
    };
    text.push(if upper { 'E' } else { 'e' });
    text.push(if exponent < 0 { '-' } else { '+' });
    text.push_str(&format!("{:02}", exponent.unsigned_abs()));
    text
}

fn hex(bits: u128, precision: Option<usize>, alternate: bool, negative: bool) -> String {
    let significand = bits as u64;
    let raw_exp = ((bits >> 64) & 0x7fff) as i32;
    let exponent = if significand == 0 {
        0
    } else {
        raw_exp.max(1) - 16383 - 3
    };
    let mut digits = format!("{significand:016x}").into_bytes();
    if let Some(p) = precision.filter(|p| *p < 15) {
        let shift = (15 - p) * 4;
        let mask = (1u64 << shift) - 1;
        let tail = significand & mask;
        let mut retained = significand >> shift;
        if rounding_up(
            negative,
            tail != 0,
            tail.cmp(&(1u64 << (shift - 1))),
            retained & 1 != 0,
        ) {
            retained += 1;
        }
        digits = format!("{retained:x}").into_bytes();
        while digits.len() < p + 1 {
            digits.insert(0, b'0');
        }
    }
    let integral = if precision.is_some_and(|p| p < 15) {
        digits.len() - precision.unwrap()
    } else {
        1
    };
    let mut text = String::from_utf8(digits[..integral].to_vec()).unwrap();
    let mut fraction = String::from_utf8(digits[integral..].to_vec()).unwrap();
    match precision {
        Some(p) => {
            while fraction.len() < p {
                fraction.push('0');
            }
        }
        None => {
            while fraction.ends_with('0') {
                fraction.pop();
            }
        }
    }
    if !fraction.is_empty() || alternate {
        text.push('.');
        text.push_str(&fraction);
    }
    text.push_str(&format!("p{exponent:+}"));
    text
}

pub(super) fn emit<S: Sink>(sink: &mut S, spec: &Spec, bits: u128) {
    let negative = bits >> 79 != 0;
    let sign = if negative {
        Some(b'-')
    } else if spec.plus {
        Some(b'+')
    } else if spec.space {
        Some(b' ')
    } else {
        None
    };
    let value = X87DoubleExtended::from_bits(bits);
    let upper = spec.conversion.is_ascii_uppercase();
    if !value.is_finite() {
        let text = if value.is_nan() {
            if upper { b"NAN" } else { b"nan" }
        } else if upper {
            b"INF"
        } else {
            b"inf"
        };
        Spec {
            zero_pad: false,
            ..*spec
        }
        .pad(sink, sign, b"", text);
        return;
    }
    if matches!(spec.conversion, b'a' | b'A') {
        let mut text = hex(bits, spec.precision, spec.alternate, negative);
        if upper {
            text.make_ascii_uppercase();
        }
        spec.pad(
            sink,
            sign,
            if upper { b"0X" } else { b"0x" },
            text.as_bytes(),
        );
        return;
    }
    let (mut digits, mut point) = decimal(bits);
    let p = spec.precision.unwrap_or(6);
    let mut text = match spec.conversion {
        b'e' | b'E' => {
            round(
                &mut digits,
                &mut point,
                p.saturating_add(1).min(i32::MAX as usize) as i32,
                negative,
            );
            scientific(&digits, point, p, spec.alternate, upper)
        }
        b'g' | b'G' => {
            let p = p.max(1);
            round(
                &mut digits,
                &mut point,
                p.min(i32::MAX as usize) as i32,
                negative,
            );
            let mut text = if point <= -4 || point > p as i32 {
                scientific(&digits, point, p - 1, spec.alternate, upper)
            } else {
                fixed(
                    &digits,
                    point,
                    (p as i64 - point as i64).max(0) as usize,
                    spec.alternate,
                )
            };
            if !spec.alternate {
                let end = text.find(['e', 'E']).unwrap_or(text.len());
                if text[..end].contains('.') {
                    let trimmed = text[..end]
                        .trim_end_matches('0')
                        .trim_end_matches('.')
                        .len();
                    text.replace_range(trimmed..end, "");
                }
            }
            text
        }
        _ => {
            let keep = (point as i64 + p as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            round(&mut digits, &mut point, keep, negative);
            fixed(&digits, point, p, spec.alternate)
        }
    };
    if upper {
        text.make_ascii_uppercase();
    }
    spec.pad(sink, sign, b"", text.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustc_apfloat::Round;

    fn convert(value: &str, precision: usize) -> String {
        let bits = X87DoubleExtended::from_str_r(value, Round::NearestTiesToEven)
            .unwrap()
            .value
            .to_bits();
        let (mut digits, mut point) = decimal(bits);
        let keep = point + precision as i32;
        round(&mut digits, &mut point, keep, bits >> 79 != 0);
        fixed(&digits, point, precision, false)
    }

    #[test]
    fn exact_integer_and_ties() {
        assert_eq!(convert("2048", 0), "2048");
        assert_eq!(convert("18446744073709551615", 0), "18446744073709551615");
        assert_eq!(convert("1.25", 1), "1.2");
        assert_eq!(convert("1.75", 1), "1.8");
        assert_eq!(convert("0.0625", 3), "0.062");
        assert_eq!(convert("0.5", 0), "0");
        assert_eq!(convert("0.75", 0), "1");
        assert_eq!(convert("9.5", 0), "10");
        assert_eq!(convert("1e4000", 0).len(), 4000);
    }
}
