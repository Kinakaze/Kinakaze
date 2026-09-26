//! Standard C/C.UTF-8 formatting and language information.
use super::*;
const DAYS: [&CStr; 7] = [
    c"Sunday",
    c"Monday",
    c"Tuesday",
    c"Wednesday",
    c"Thursday",
    c"Friday",
    c"Saturday",
];
const SHORT_DAYS: [&CStr; 7] = [c"Sun", c"Mon", c"Tue", c"Wed", c"Thu", c"Fri", c"Sat"];
const MONTHS: [&CStr; 12] = [
    c"January",
    c"February",
    c"March",
    c"April",
    c"May",
    c"June",
    c"July",
    c"August",
    c"September",
    c"October",
    c"November",
    c"December",
];
const SHORT_MONTHS: [&CStr; 12] = [
    c"Jan", c"Feb", c"Mar", c"Apr", c"May", c"Jun", c"Jul", c"Aug", c"Sep", c"Oct", c"Nov", c"Dec",
];

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_nl_langinfo_l(
    item: c_int,
    locale: *mut c_void,
) -> *const c_char {
    let category = (item as u32 >> 16) as usize;
    let index = item as u32 & 0xffff;
    let selected = flags(locale as usize);
    // GNU word-valued items are returned through the pointer-sized union slot.
    // GNOME uses the integer value itself to determine the calendar week base.
    if (category, index) == (2, 0x66) {
        return 19971130usize as *const c_char;
    }
    if index == 0xffff && category < CATEGORIES && category != LC_ALL {
        return name(selected & (1 << category) != 0).as_ptr();
    }
    match (category, index) {
        (0, 14) => {
            if selected & 1 != 0 {
                c"UTF-8"
            } else {
                c"ANSI_X3.4-1968"
            }
        }
        (1, 0) => c".",
        (1, 1 | 2) => c"",
        (2, 0..=6) => SHORT_DAYS[index as usize],
        (2, 7..=13) => DAYS[index as usize - 7],
        (2, 14..=25) => SHORT_MONTHS[index as usize - 14],
        (2, 26..=37) => MONTHS[index as usize - 26],
        (2, 38) => c"AM",
        (2, 39) => c"PM",
        (2, 40) => c"%a %b %e %H:%M:%S %Y",
        (2, 41) => c"%m/%d/%y",
        (2, 42) => c"%H:%M:%S",
        (2, 43) => c"%I:%M:%S %p",
        (2, 0x65) => c"\x07",
        (2, 0x67) => c"\x04",
        (2, 0x68 | 0x6a) => c"\x01",
        (2, 0x69) => c"\x02",
        (2, 0x6c) => c"%a %b %e %H:%M:%S %Z %Y",
        (2, 0x6e) => {
            if selected & (1 << 2) != 0 {
                c"UTF-8"
            } else {
                c"ANSI_X3.4-1968"
            }
        }
        (5, 0) => c"^[yY]",
        (5, 1) => c"^[nN]",
        (5, 2) => c"",
        (5, 3) => c"",
        _ => c"",
    }
    .as_ptr()
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_nl_langinfo(item: c_int) -> *const c_char {
    kinakaze_abi_nl_langinfo_l(item, kinakaze_tls::locale() as _)
}
alias!(safe kinakaze_abi___nl_langinfo_l,kinakaze_abi_nl_langinfo_l,(item:c_int,locale:*mut c_void)->*const c_char);

#[repr(C)]
pub struct Lconv {
    pub decimal_point: *mut c_char,
    pub thousands_sep: *mut c_char,
    pub grouping: *mut c_char,
    pub int_curr_symbol: *mut c_char,
    pub currency_symbol: *mut c_char,
    pub mon_decimal_point: *mut c_char,
    pub mon_thousands_sep: *mut c_char,
    pub mon_grouping: *mut c_char,
    pub positive_sign: *mut c_char,
    pub negative_sign: *mut c_char,
    pub int_frac_digits: c_char,
    pub frac_digits: c_char,
    pub p_cs_precedes: c_char,
    pub p_sep_by_space: c_char,
    pub n_cs_precedes: c_char,
    pub n_sep_by_space: c_char,
    pub p_sign_posn: c_char,
    pub n_sign_posn: c_char,
    pub int_p_cs_precedes: c_char,
    pub int_p_sep_by_space: c_char,
    pub int_n_cs_precedes: c_char,
    pub int_n_sep_by_space: c_char,
    pub int_p_sign_posn: c_char,
    pub int_n_sign_posn: c_char,
}
static mut CONVENTIONS: Lconv = Lconv {
    decimal_point: c".".as_ptr().cast_mut(),
    thousands_sep: c"".as_ptr().cast_mut(),
    grouping: c"".as_ptr().cast_mut(),
    int_curr_symbol: c"".as_ptr().cast_mut(),
    currency_symbol: c"".as_ptr().cast_mut(),
    mon_decimal_point: c"".as_ptr().cast_mut(),
    mon_thousands_sep: c"".as_ptr().cast_mut(),
    mon_grouping: c"".as_ptr().cast_mut(),
    positive_sign: c"".as_ptr().cast_mut(),
    negative_sign: c"".as_ptr().cast_mut(),
    int_frac_digits: 127,
    frac_digits: 127,
    p_cs_precedes: 127,
    p_sep_by_space: 127,
    n_cs_precedes: 127,
    n_sep_by_space: 127,
    p_sign_posn: 127,
    n_sign_posn: 127,
    int_p_cs_precedes: 127,
    int_p_sep_by_space: 127,
    int_n_cs_precedes: 127,
    int_n_sep_by_space: 127,
    int_p_sign_posn: 127,
    int_n_sign_posn: 127,
};
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_localeconv() -> *mut Lconv {
    ptr::addr_of_mut!(CONVENTIONS)
}

/// GNU libc's message domain is an array object, including its terminating NUL.
/// locale/getent binaries use a five-byte ELF COPY relocation for this ABI.
#[unsafe(no_mangle)]
pub static kinakaze_abi__libc_intl_domainname: [u8; 5] = *b"libc\0";
