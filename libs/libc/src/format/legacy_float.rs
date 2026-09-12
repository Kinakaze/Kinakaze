//! System V decimal conversion, used by older Chromium builds.
use core::cell::RefCell;
use core::ffi::{c_char, c_int};

thread_local! { static FCVT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) }; }

fn convert(mut value: f64, digits: i32) -> (String, i32, i32) {
    if !value.is_finite() {
        return (
            if value.is_nan() {
                "nan"
            } else if value.is_sign_negative() {
                "-inf"
            } else {
                "inf"
            }
            .into(),
            0,
            0,
        );
    }
    let sign = i32::from(value.is_sign_negative());
    value = value.abs();
    // Negative precision rounds left of the decimal, retaining at least one
    // significant digit. This bounds even an INT_MIN precision to 308 steps.
    let mut left = 0;
    while left < digits.saturating_neg() && value * 0.1 >= 1.0 {
        value *= 0.1;
        left += 1;
    }
    let precision = digits.clamp(0, 17) as usize;
    let text = format!("{value:.precision$}");
    let point = text.find('.').unwrap_or(text.len());
    let mut decimal = point as i32;
    let mut result = text.replace('.', "");
    if point == 1 && result.starts_with('0') && value != 0.0 && precision > 0 {
        let leading = result.bytes().take_while(|b| *b == b'0').count();
        result.drain(..leading);
        decimal -= leading as i32;
    }
    result.extend(std::iter::repeat_n('0', left as usize));
    (result, decimal + left, sign)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fcvt(
    value: f64,
    digits: c_int,
    decimal: *mut c_int,
    sign: *mut c_int,
) -> *mut c_char {
    if decimal.is_null() || sign.is_null() {
        crate::set_errno(22);
        return core::ptr::null_mut();
    }
    let (text, point, negative) = convert(value, digits);
    unsafe {
        *decimal = point;
        *sign = negative;
    }
    FCVT.with(|slot| {
        let mut buffer = slot.borrow_mut();
        buffer.clear();
        buffer.extend_from_slice(text.as_bytes());
        buffer.push(0);
        buffer.as_mut_ptr().cast()
    })
}

#[cfg(test)]
mod tests {
    use super::convert;
    #[test]
    fn decimal_positions_and_rounding() {
        assert_eq!(convert(12.375, 2), ("1238".into(), 2, 0));
        assert_eq!(convert(-0.00125, 5), ("125".into(), -2, 1));
        assert_eq!(convert(123.0, -1), ("120".into(), 3, 0));
        assert_eq!(convert(123.0, -999), ("100".into(), 3, 0));
        assert_eq!(convert(-0.0, 2), ("000".into(), 1, 1));
    }
}
