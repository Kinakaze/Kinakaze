//! String and memory functions beyond the C standard set.
//!
//! These are the GNU and POSIX extensions real programs use constantly. They live
//! apart from `string.rs` only so this file can grow without colliding with it.

#[cfg(all(windows, target_arch = "x86_64"))]
pub mod windows {
    use core::ffi::{c_char, c_int, c_long, c_void};

    /// Length of a NUL-terminated string.
    ///
    /// # Safety
    ///
    /// `text` must be NUL-terminated.
    unsafe fn length(text: *const c_char) -> usize {
        let mut count = 0usize;
        // SAFETY: forwarded from the caller's contract.
        unsafe {
            while *text.add(count) != 0 {
                count += 1;
            }
        }
        count
    }

    /// `stpcpy`: copies a string and returns a pointer to the destination's NUL.
    ///
    /// The return value is the whole point — it lets a caller concatenate without
    /// rescanning, which is why code that builds paths in a loop prefers it to
    /// `strcpy`.
    ///
    /// # Safety
    ///
    /// `source` must be NUL-terminated and `destination` must have room for it.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_stpcpy(
        destination: *mut c_char,
        source: *const c_char,
    ) -> *mut c_char {
        // SAFETY: forwarded from this function's contract.
        unsafe {
            let mut at = 0usize;
            loop {
                let byte = *source.add(at);
                *destination.add(at) = byte;
                if byte == 0 {
                    return destination.add(at);
                }
                at += 1;
            }
        }
    }

    /// `stpncpy`: the bounded form, returning a pointer past the last byte written.
    ///
    /// Unlike `stpcpy` the result is not always the NUL: when the source fills the
    /// buffer there is no terminator, and the return value is one past the end.
    ///
    /// # Safety
    ///
    /// `destination` must have room for `count` bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_stpncpy(
        destination: *mut c_char,
        source: *const c_char,
        count: usize,
    ) -> *mut c_char {
        // SAFETY: forwarded from this function's contract.
        unsafe {
            let mut at = 0usize;
            while at < count {
                let byte = *source.add(at);
                *destination.add(at) = byte;
                if byte == 0 {
                    // The tail is padded with NULs, as `strncpy` specifies.
                    let end = destination.add(at);
                    let mut pad = at;
                    while pad < count {
                        *destination.add(pad) = 0;
                        pad += 1;
                    }
                    return end;
                }
                at += 1;
            }
            destination.add(count)
        }
    }

    /// `mempcpy`: like `memcpy` but returns the end of the destination.
    ///
    /// # Safety
    ///
    /// Both regions must be valid for `count` bytes and must not overlap.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_mempcpy(
        destination: *mut c_void,
        source: *const c_void,
        count: usize,
    ) -> *mut c_void {
        // SAFETY: forwarded from this function's contract.
        unsafe {
            std::ptr::copy_nonoverlapping(source.cast::<u8>(), destination.cast::<u8>(), count);
            destination.cast::<u8>().add(count).cast()
        }
    }

    /// `strchrnul`: `strchr`, but returns the NUL instead of null when absent.
    ///
    /// The difference removes a branch from every caller, which is why parsing code
    /// uses it.
    ///
    /// # Safety
    ///
    /// `text` must be NUL-terminated.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_strchrnul(
        text: *const c_char,
        character: c_int,
    ) -> *mut c_char {
        let wanted = character as c_char;
        // SAFETY: forwarded from this function's contract.
        unsafe {
            let mut at = 0usize;
            loop {
                let byte = *text.add(at);
                if byte == wanted || byte == 0 {
                    return text.add(at).cast_mut();
                }
                at += 1;
            }
        }
    }

    /// `memrchr`: finds the last occurrence of a byte in a region.
    ///
    /// # Safety
    ///
    /// `region` must be readable for `count` bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_memrchr(
        region: *const c_void,
        character: c_int,
        count: usize,
    ) -> *mut c_void {
        let wanted = character as u8;
        // SAFETY: forwarded from this function's contract.
        let bytes = unsafe { std::slice::from_raw_parts(region.cast::<u8>(), count) };
        match bytes.iter().rposition(|byte| *byte == wanted) {
            // SAFETY: the index came from this slice.
            Some(index) => unsafe { region.cast::<u8>().add(index).cast_mut().cast() },
            None => std::ptr::null_mut(),
        }
    }

    /// `strcasecmp`: compares ignoring ASCII case.
    ///
    /// Only ASCII case is folded. That is correct for the C locale and is what the
    /// callers here rely on; honouring a locale would need locale data this layer
    /// does not carry.
    ///
    /// # Safety
    ///
    /// Both strings must be NUL-terminated.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_strcasecmp(
        left: *const c_char,
        right: *const c_char,
    ) -> c_int {
        // SAFETY: forwarded from this function's contract.
        unsafe {
            let mut at = 0usize;
            loop {
                let a = (*left.add(at) as u8).to_ascii_lowercase();
                let b = (*right.add(at) as u8).to_ascii_lowercase();
                if a != b {
                    return c_int::from(a) - c_int::from(b);
                }
                if a == 0 {
                    return 0;
                }
                at += 1;
            }
        }
    }

    /// `strncasecmp`: the bounded form of [`kinakaze_abi_strcasecmp`].
    ///
    /// # Safety
    ///
    /// Both strings must be readable up to `count` bytes or a NUL.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_strncasecmp(
        left: *const c_char,
        right: *const c_char,
        count: usize,
    ) -> c_int {
        // SAFETY: forwarded from this function's contract.
        unsafe {
            let mut at = 0usize;
            while at < count {
                let a = (*left.add(at) as u8).to_ascii_lowercase();
                let b = (*right.add(at) as u8).to_ascii_lowercase();
                if a != b {
                    return c_int::from(a) - c_int::from(b);
                }
                if a == 0 {
                    return 0;
                }
                at += 1;
            }
        }
        0
    }

    /// `strcasestr`: case-insensitive substring search.
    ///
    /// # Safety
    ///
    /// Both strings must be NUL-terminated.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_strcasestr(
        haystack: *const c_char,
        needle: *const c_char,
    ) -> *mut c_char {
        // SAFETY: forwarded from this function's contract.
        unsafe {
            let needle_length = length(needle);
            if needle_length == 0 {
                return haystack.cast_mut();
            }
            let haystack_length = length(haystack);
            if needle_length > haystack_length {
                return std::ptr::null_mut();
            }
            for start in 0..=(haystack_length - needle_length) {
                let mut matched = true;
                for offset in 0..needle_length {
                    let a = (*haystack.add(start + offset) as u8).to_ascii_lowercase();
                    let b = (*needle.add(offset) as u8).to_ascii_lowercase();
                    if a != b {
                        matched = false;
                        break;
                    }
                }
                if matched {
                    return haystack.add(start).cast_mut();
                }
            }
            std::ptr::null_mut()
        }
    }

    /// `strsep`: splits a string in place at any delimiter.
    ///
    /// Advances `*text` past the token and overwrites the delimiter with a NUL, so
    /// the caller's buffer is modified. Unlike `strtok` it reports empty fields,
    /// which is why parsers of `/etc/passwd`-style data use it.
    ///
    /// # Safety
    ///
    /// `text` must point to a mutable pointer into a writable NUL-terminated string.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_strsep(
        text: *mut *mut c_char,
        delimiters: *const c_char,
    ) -> *mut c_char {
        // SAFETY: forwarded from this function's contract.
        unsafe {
            if text.is_null() {
                return std::ptr::null_mut();
            }
            let start = *text;
            if start.is_null() {
                return std::ptr::null_mut();
            }
            let delimiter_count = length(delimiters);
            let mut at = 0usize;
            loop {
                let byte = *start.add(at);
                if byte == 0 {
                    // Last token: the next call reports exhaustion.
                    *text = std::ptr::null_mut();
                    return start;
                }
                let mut is_delimiter = false;
                for index in 0..delimiter_count {
                    if byte == *delimiters.add(index) {
                        is_delimiter = true;
                        break;
                    }
                }
                if is_delimiter {
                    *start.add(at) = 0;
                    *text = start.add(at + 1);
                    return start;
                }
                at += 1;
            }
        }
    }

    /// `strverscmp`: compares strings treating digit runs as numbers.
    ///
    /// `ls -v` and version sorting depend on `file9` ordering before `file10`, which
    /// a byte comparison gets backwards.
    ///
    /// # Safety
    ///
    /// Both strings must be NUL-terminated.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_strverscmp(
        left: *const c_char,
        right: *const c_char,
    ) -> c_int {
        // SAFETY: forwarded from this function's contract.
        let (a, b) = unsafe {
            (
                std::slice::from_raw_parts(left.cast::<u8>(), length(left)),
                std::slice::from_raw_parts(right.cast::<u8>(), length(right)),
            )
        };

        let mut i = 0usize;
        let mut j = 0usize;
        while i < a.len() && j < b.len() {
            if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
                // Compare the whole digit runs numerically. Leading zeroes are
                // skipped so `08` and `8` compare equal in magnitude.
                let start_a = i;
                let start_b = j;
                while i < a.len() && a[i].is_ascii_digit() {
                    i += 1;
                }
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                let run_a = trim_zeroes(&a[start_a..i]);
                let run_b = trim_zeroes(&b[start_b..j]);
                if run_a.len() != run_b.len() {
                    return if run_a.len() < run_b.len() { -1 } else { 1 };
                }
                match run_a.cmp(run_b) {
                    std::cmp::Ordering::Equal => {}
                    std::cmp::Ordering::Less => return -1,
                    std::cmp::Ordering::Greater => return 1,
                }
                continue;
            }
            if a[i] != b[j] {
                return c_int::from(a[i]) - c_int::from(b[j]);
            }
            i += 1;
            j += 1;
        }
        // The shorter string sorts first when one is a prefix of the other.
        match (a.len() - i).cmp(&(b.len() - j)) {
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Greater => 1,
        }
    }

    fn trim_zeroes(run: &[u8]) -> &[u8] {
        let first = run.iter().position(|byte| *byte != b'0');
        match first {
            Some(index) => &run[index..],
            // All zeroes: keep one so an empty slice never compares as shorter.
            None => &run[run.len().saturating_sub(1)..],
        }
    }

    /// `bzero` and `bcopy`: the legacy BSD spellings.
    ///
    /// # Safety
    ///
    /// The regions must be valid for `count` bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_bzero(region: *mut c_void, count: usize) {
        // SAFETY: forwarded from this function's contract.
        unsafe { std::ptr::write_bytes(region.cast::<u8>(), 0, count) };
    }

    /// # Safety
    ///
    /// Both regions must be valid for `count` bytes; overlap is permitted.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_bcopy(
        source: *const c_void,
        destination: *mut c_void,
        count: usize,
    ) {
        // `bcopy` takes its arguments in the opposite order to `memmove` and is
        // defined to handle overlap.
        // SAFETY: forwarded from this function's contract.
        unsafe { std::ptr::copy(source.cast::<u8>(), destination.cast::<u8>(), count) };
    }

    pub use crate::locale::info::*;
    pub use crate::locale::wide::*;
    pub type WcharT = i32;

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcslen(s: *const WcharT) -> usize {
        if s.is_null() {
            return 0;
        }
        let mut len = 0;
        unsafe {
            while *s.add(len) != 0 {
                len += 1;
            }
        }
        len
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcsdup(s: *const WcharT) -> *mut WcharT {
        let length = unsafe { kinakaze_abi_wcslen(s) };
        let Some(bytes) = length
            .checked_add(1)
            .and_then(|n| n.checked_mul(size_of::<WcharT>()))
        else {
            crate::set_errno(kinakaze_vfs::ENOMEM);
            return std::ptr::null_mut();
        };
        let result = unsafe { crate::c_malloc(bytes) }.cast::<WcharT>();
        if !result.is_null() {
            unsafe { std::ptr::copy_nonoverlapping(s, result, length + 1) };
        }
        result
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcscmp(
        s1: *const WcharT,
        s2: *const WcharT,
    ) -> c_int {
        let mut i = 0;
        unsafe {
            loop {
                let c1 = *s1.add(i);
                let c2 = *s2.add(i);
                if c1 != c2 {
                    return if c1 < c2 { -1 } else { 1 };
                }
                if c1 == 0 {
                    return 0;
                }
                i += 1;
            }
        }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcsncmp(
        s1: *const WcharT,
        s2: *const WcharT,
        n: usize,
    ) -> c_int {
        if n == 0 {
            return 0;
        }
        for i in 0..n {
            unsafe {
                let c1 = *s1.add(i);
                let c2 = *s2.add(i);
                if c1 != c2 {
                    return if c1 < c2 { -1 } else { 1 };
                }
                if c1 == 0 {
                    return 0;
                }
            }
        }
        0
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wmemcpy(
        d: *mut WcharT,
        s: *const WcharT,
        n: usize,
    ) -> *mut WcharT {
        unsafe { std::ptr::copy_nonoverlapping(s, d, n) };
        d
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wmemmove(
        d: *mut WcharT,
        s: *const WcharT,
        n: usize,
    ) -> *mut WcharT {
        unsafe { std::ptr::copy(s, d, n) };
        d
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wmemset(
        s: *mut WcharT,
        c: WcharT,
        n: usize,
    ) -> *mut WcharT {
        for i in 0..n {
            unsafe { *s.add(i) = c };
        }
        s
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wmemcmp(
        s1: *const WcharT,
        s2: *const WcharT,
        n: usize,
    ) -> c_int {
        for i in 0..n {
            unsafe {
                let c1 = *s1.add(i);
                let c2 = *s2.add(i);
                if c1 != c2 {
                    return if c1 < c2 { -1 } else { 1 };
                }
            }
        }
        0
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wmemchr(
        s: *const WcharT,
        c: WcharT,
        n: usize,
    ) -> *mut WcharT {
        for i in 0..n {
            unsafe {
                if *s.add(i) == c {
                    return s.add(i) as *mut WcharT;
                }
            }
        }
        std::ptr::null_mut()
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcscasecmp(
        left: *const WcharT,
        right: *const WcharT,
    ) -> c_int {
        unsafe { kinakaze_abi_wcsncasecmp(left, right, usize::MAX) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcsncasecmp(
        left: *const WcharT,
        right: *const WcharT,
        length: usize,
    ) -> c_int {
        for index in 0..length {
            let a = unsafe { kinakaze_abi_towlower(*left.add(index) as u32) };
            let b = unsafe { kinakaze_abi_towlower(*right.add(index) as u32) };
            if a != b {
                return if a < b { -1 } else { 1 };
            }
            if a == 0 {
                return 0;
            }
        }
        0
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn wcwidth(wc: WcharT) -> c_int {
        kinakaze_abi_wcwidth(wc as u32)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn wcswidth(pwcs: *const WcharT, n: usize) -> c_int {
        unsafe { kinakaze_abi_wcswidth(pwcs, n) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcswidth(pwcs: *const WcharT, n: usize) -> c_int {
        if pwcs.is_null() {
            return 0;
        }
        let mut total = 0;
        for i in 0..n {
            let ch = unsafe { *pwcs.add(i) };
            if ch == 0 {
                break;
            }
            let w = kinakaze_abi_wcwidth(ch as u32);
            if w < 0 {
                return -1;
            }
            total += w;
        }
        total
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn btowc(c: c_int) -> WcharT {
        unsafe { kinakaze_abi_btowc(c) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_btowc(c: c_int) -> WcharT {
        if c >= 0 && c < 128 { c as WcharT } else { -1 }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn wctob(wc: WcharT) -> c_int {
        unsafe { kinakaze_abi_wctob(wc) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wctob(wc: WcharT) -> c_int {
        if wc >= 0 && wc < 128 { wc as c_int } else { -1 }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn nl_langinfo(item: c_int) -> *const c_char {
        kinakaze_abi_nl_langinfo(item)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn nl_langinfo_l(item: c_int, loc: *mut c_void) -> *const c_char {
        kinakaze_abi___nl_langinfo_l(item, loc)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn __nl_langinfo_l(item: c_int, loc: *mut c_void) -> *const c_char {
        kinakaze_abi___nl_langinfo_l(item, loc)
    }

    pub use crate::glob::{GlobT, glob, globfree};

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_bindtextdomain(
        _domain: *const c_char,
        _dir: *const c_char,
    ) -> *const c_char {
        c"/usr/share/locale".as_ptr()
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_bind_textdomain_codeset(
        _domain: *const c_char,
        _codeset: *const c_char,
    ) -> *const c_char {
        c"UTF-8".as_ptr()
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_textdomain(domain: *const c_char) -> *const c_char {
        if domain.is_null() {
            c"messages".as_ptr()
        } else {
            domain
        }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_gettext(msgid: *const c_char) -> *const c_char {
        msgid
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_dgettext(
        _domain: *const c_char,
        msgid: *const c_char,
    ) -> *const c_char {
        msgid
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_dcgettext(
        _domain: *const c_char,
        msgid: *const c_char,
        _category: c_int,
    ) -> *const c_char {
        msgid
    }

    // --- Additional WCHAR & POSIX Functions ---

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcsnlen(s: *const WcharT, maxlen: usize) -> usize {
        if s.is_null() {
            return 0;
        }
        let mut len = 0;
        unsafe {
            while len < maxlen && *s.add(len) != 0 {
                len += 1;
            }
        }
        len
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcscpy(
        dest: *mut WcharT,
        src: *const WcharT,
    ) -> *mut WcharT {
        if dest.is_null() || src.is_null() {
            return dest;
        }
        let mut i = 0;
        unsafe {
            loop {
                let ch = *src.add(i);
                *dest.add(i) = ch;
                if ch == 0 {
                    break;
                }
                i += 1;
            }
        }
        dest
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcsncpy(
        dest: *mut WcharT,
        src: *const WcharT,
        n: usize,
    ) -> *mut WcharT {
        if dest.is_null() || src.is_null() {
            return dest;
        }
        let mut i = 0;
        unsafe {
            while i < n && *src.add(i) != 0 {
                *dest.add(i) = *src.add(i);
                i += 1;
            }
            while i < n {
                *dest.add(i) = 0;
                i += 1;
            }
        }
        dest
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcscat(
        dest: *mut WcharT,
        src: *const WcharT,
    ) -> *mut WcharT {
        if dest.is_null() || src.is_null() {
            return dest;
        }
        let mut d_len = 0;
        unsafe {
            while *dest.add(d_len) != 0 {
                d_len += 1;
            }
            let mut i = 0;
            loop {
                let ch = *src.add(i);
                *dest.add(d_len + i) = ch;
                if ch == 0 {
                    break;
                }
                i += 1;
            }
        }
        dest
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcsncat(
        dest: *mut WcharT,
        src: *const WcharT,
        n: usize,
    ) -> *mut WcharT {
        if dest.is_null() || src.is_null() {
            return dest;
        }
        let mut d_len = 0;
        unsafe {
            while *dest.add(d_len) != 0 {
                d_len += 1;
            }
            let mut i = 0;
            while i < n && *src.add(i) != 0 {
                *dest.add(d_len + i) = *src.add(i);
                i += 1;
            }
            *dest.add(d_len + i) = 0;
        }
        dest
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcschr(s: *const WcharT, c: WcharT) -> *mut WcharT {
        if s.is_null() {
            return core::ptr::null_mut();
        }
        let mut p = s;
        unsafe {
            loop {
                if *p == c {
                    return p as *mut WcharT;
                }
                if *p == 0 {
                    return core::ptr::null_mut();
                }
                p = p.add(1);
            }
        }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcsrchr(s: *const WcharT, c: WcharT) -> *mut WcharT {
        if s.is_null() {
            return core::ptr::null_mut();
        }
        let mut last = core::ptr::null_mut();
        let mut p = s;
        unsafe {
            loop {
                if *p == c {
                    last = p as *mut WcharT;
                }
                if *p == 0 {
                    return last;
                }
                p = p.add(1);
            }
        }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcscspn(
        s: *const WcharT,
        reject: *const WcharT,
    ) -> usize {
        let mut count = 0;
        unsafe {
            while *s.add(count) != 0 {
                let ch = *s.add(count);
                let mut r = reject;
                while *r != 0 {
                    if *r == ch {
                        return count;
                    }
                    r = r.add(1);
                }
                count += 1;
            }
        }
        count
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcsspn(
        s: *const WcharT,
        accept: *const WcharT,
    ) -> usize {
        let mut count = 0;
        unsafe {
            while *s.add(count) != 0 {
                let ch = *s.add(count);
                let mut a = accept;
                let mut found = false;
                while *a != 0 {
                    if *a == ch {
                        found = true;
                        break;
                    }
                    a = a.add(1);
                }
                if !found {
                    return count;
                }
                count += 1;
            }
        }
        count
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcspbrk(
        s: *const WcharT,
        accept: *const WcharT,
    ) -> *mut WcharT {
        let mut p = s;
        unsafe {
            while *p != 0 {
                let ch = *p;
                let mut a = accept;
                while *a != 0 {
                    if *a == ch {
                        return p as *mut WcharT;
                    }
                    a = a.add(1);
                }
                p = p.add(1);
            }
        }
        core::ptr::null_mut()
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcsstr(
        haystack: *const WcharT,
        needle: *const WcharT,
    ) -> *mut WcharT {
        unsafe {
            if *needle == 0 {
                return haystack as *mut WcharT;
            }
            let mut h = haystack;
            while *h != 0 {
                let mut h_sub = h;
                let mut n = needle;
                while *h_sub != 0 && *n != 0 && *h_sub == *n {
                    h_sub = h_sub.add(1);
                    n = n.add(1);
                }
                if *n == 0 {
                    return h as *mut WcharT;
                }
                h = h.add(1);
            }
        }
        core::ptr::null_mut()
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcstok(
        s: *mut WcharT,
        delim: *const WcharT,
        ptr: *mut *mut WcharT,
    ) -> *mut WcharT {
        unsafe {
            let mut str_ptr = if !s.is_null() {
                s
            } else if !ptr.is_null() {
                *ptr
            } else {
                core::ptr::null_mut()
            };
            if str_ptr.is_null() {
                return core::ptr::null_mut();
            }

            while *str_ptr != 0 {
                let ch = *str_ptr;
                let mut d = delim;
                let mut is_delim = false;
                while *d != 0 {
                    if *d == ch {
                        is_delim = true;
                        break;
                    }
                    d = d.add(1);
                }
                if !is_delim {
                    break;
                }
                str_ptr = str_ptr.add(1);
            }
            if *str_ptr == 0 {
                if !ptr.is_null() {
                    *ptr = str_ptr;
                }
                return core::ptr::null_mut();
            }
            let token_start = str_ptr;
            while *str_ptr != 0 {
                let ch = *str_ptr;
                let mut d = delim;
                let mut is_delim = false;
                while *d != 0 {
                    if *d == ch {
                        is_delim = true;
                        break;
                    }
                    d = d.add(1);
                }
                if is_delim {
                    *str_ptr = 0;
                    if !ptr.is_null() {
                        *ptr = str_ptr.add(1);
                    }
                    return token_start;
                }
                str_ptr = str_ptr.add(1);
            }
            if !ptr.is_null() {
                *ptr = str_ptr;
            }
            token_start
        }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcscoll(
        s1: *const WcharT,
        s2: *const WcharT,
    ) -> c_int {
        unsafe { kinakaze_abi_wcscmp(s1, s2) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcsxfrm(
        dest: *mut WcharT,
        src: *const WcharT,
        n: usize,
    ) -> usize {
        let len = unsafe { kinakaze_abi_wcslen(src) };
        if n > 0 && !dest.is_null() {
            unsafe { kinakaze_abi_wcsncpy(dest, src, n) };
        }
        len
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcsftime(
        s: *mut WcharT,
        maxsize: usize,
        format: *const WcharT,
        timeptr: *const c_void,
    ) -> usize {
        if s.is_null() || maxsize == 0 || format.is_null() {
            return 0;
        }
        let mut narrow_fmt = Vec::new();
        let mut p = format;
        unsafe {
            while *p != 0 {
                narrow_fmt.push(*p as u8);
                p = p.add(1);
            }
        }
        narrow_fmt.push(0);
        let mut narrow_buf = vec![0u8; maxsize * 4 + 1];
        let n = unsafe {
            crate::time::kinakaze_abi_strftime(
                narrow_buf.as_mut_ptr() as *mut c_char,
                narrow_buf.len(),
                narrow_fmt.as_ptr() as *const c_char,
                timeptr as *const crate::time::Tm,
            )
        };
        if n == 0 {
            return 0;
        }
        let to_copy = n.min(maxsize - 1);
        unsafe {
            for i in 0..to_copy {
                *s.add(i) = narrow_buf[i] as WcharT;
            }
            *s.add(to_copy) = 0;
        }
        to_copy
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcstol(
        nptr: *const WcharT,
        endptr: *mut *mut WcharT,
        base: c_int,
    ) -> i64 {
        if nptr.is_null() {
            return 0;
        }
        let mut p = nptr;
        unsafe {
            while *p != 0 && kinakaze_abi_iswspace(*p as u32) != 0 {
                p = p.add(1);
            }
            let mut negative = false;
            if *p == b'-' as WcharT {
                negative = true;
                p = p.add(1);
            } else if *p == b'+' as WcharT {
                p = p.add(1);
            }

            let base = if base == 0 {
                if *p == b'0' as WcharT {
                    if *p.add(1) == b'x' as WcharT || *p.add(1) == b'X' as WcharT {
                        p = p.add(2);
                        16
                    } else {
                        8
                    }
                } else {
                    10
                }
            } else if base == 16
                && *p == b'0' as WcharT
                && (*p.add(1) == b'x' as WcharT || *p.add(1) == b'X' as WcharT)
            {
                p = p.add(2);
                16
            } else {
                base
            };

            let mut val = 0i64;
            let start = p;
            while *p != 0 {
                let digit = match *p as u8 {
                    b'0'..=b'9' => (*p - b'0' as WcharT) as i64,
                    b'a'..=b'z' => (*p - b'a' as WcharT + 10) as i64,
                    b'A'..=b'Z' => (*p - b'A' as WcharT + 10) as i64,
                    _ => break,
                };
                if digit >= base as i64 {
                    break;
                }
                val = val.saturating_mul(base as i64).saturating_add(digit);
                p = p.add(1);
            }
            if !endptr.is_null() {
                *endptr = if p == start {
                    nptr as *mut WcharT
                } else {
                    p as *mut WcharT
                };
            }
            if negative { -val } else { val }
        }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcstoll(
        nptr: *const WcharT,
        endptr: *mut *mut WcharT,
        base: c_int,
    ) -> i64 {
        unsafe { kinakaze_abi_wcstol(nptr, endptr, base) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcstoul(
        nptr: *const WcharT,
        endptr: *mut *mut WcharT,
        base: c_int,
    ) -> u64 {
        unsafe { kinakaze_abi_wcstol(nptr, endptr, base) as u64 }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcstoull(
        nptr: *const WcharT,
        endptr: *mut *mut WcharT,
        base: c_int,
    ) -> u64 {
        unsafe { kinakaze_abi_wcstol(nptr, endptr, base) as u64 }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcstod(
        nptr: *const WcharT,
        endptr: *mut *mut WcharT,
    ) -> f64 {
        let mut narrow = Vec::new();
        let mut p = nptr;
        unsafe {
            while *p != 0 {
                narrow.push(*p as u8);
                p = p.add(1);
            }
        }
        narrow.push(0);
        let mut end_narrow: *const c_char = core::ptr::null();
        let res = unsafe {
            crate::process::kinakaze_abi_strtod(narrow.as_ptr() as *const c_char, &mut end_narrow)
        };
        if !endptr.is_null() {
            let offset = end_narrow as usize - narrow.as_ptr() as usize;
            unsafe { *endptr = nptr.add(offset) as *mut WcharT };
        }
        res
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcstof(
        nptr: *const WcharT,
        endptr: *mut *mut WcharT,
    ) -> f32 {
        unsafe { kinakaze_abi_wcstod(nptr, endptr) as f32 }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_wcstold(
        nptr: *const WcharT,
        endptr: *mut *mut WcharT,
    ) -> f64 {
        unsafe { kinakaze_abi_wcstod(nptr, endptr) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___rawmemchr(
        s: *const c_void,
        c: c_int,
    ) -> *mut c_void {
        let mut p = s as *const u8;
        let target = c as u8;
        unsafe {
            while *p != target {
                p = p.add(1);
            }
        }
        p as *mut c_void
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___strdup(s: *const c_char) -> *mut c_char {
        unsafe { crate::string::strdup(s) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_strcoll(
        s1: *const c_char,
        s2: *const c_char,
    ) -> c_int {
        unsafe { crate::string::strcmp(s1, s2) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_bcmp(
        s1: *const c_void,
        s2: *const c_void,
        n: usize,
    ) -> c_int {
        unsafe { crate::string::memcmp(s1, s2, n) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi__IO_getc(stream: *mut crate::stdio::File) -> c_int {
        crate::stdio::fgetc(stream)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi__IO_putc(
        c: c_int,
        stream: *mut crate::stdio::File,
    ) -> c_int {
        crate::stdio::fputc(c, stream)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___uflow(stream: *mut crate::stdio::File) -> c_int {
        crate::stdio::fgetc(stream)
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi_flockfile(_stream: *mut c_void) {}

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi_funlockfile(_stream: *mut c_void) {}

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi_clock() -> i64 {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let start = START.get_or_init(std::time::Instant::now);
        start.elapsed().as_micros() as i64
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi_pause() -> c_int {
        std::thread::park();
        -1
    }

    /// # Safety
    /// A non-null buf must be writable for len bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_confstr(
        name: c_int,
        buf: *mut c_char,
        len: usize,
    ) -> usize {
        let val: &[u8] = match name {
            2 => b"/bin:/usr/bin\0", // _CS_PATH
            _ => b"\0",
        };
        if len > 0 && !buf.is_null() {
            let to_copy = val.len().min(len);
            unsafe {
                core::ptr::copy_nonoverlapping(val.as_ptr() as *const c_char, buf, to_copy);
                if to_copy < len {
                    *buf.add(to_copy) = 0;
                }
            }
        }
        val.len()
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___fdelt_chk(d: c_long) -> c_long {
        if d < 0 || d >= 1024 {
            eprintln!("kinakaze: *** buffer overflow in select ***");
            std::process::abort();
        }
        d / 64
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___xpg_strerror_r(
        errnum: c_int,
        buf: *mut c_char,
        buflen: usize,
    ) -> c_int {
        unsafe { crate::string::xpg_strerror_r(errnum, buf, buflen) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_strtok(
        text: *mut c_char,
        delimiters: *const c_char,
    ) -> *mut c_char {
        thread_local! {
            static SAVED: core::cell::UnsafeCell<*mut c_char> = const {
                core::cell::UnsafeCell::new(core::ptr::null_mut())
            };
        }
        SAVED.with(|cell| unsafe {
            crate::misc::kinakaze_abi_strtok_r(text, delimiters, cell.get())
        })
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_strlcpy(
        dst: *mut c_char,
        src: *const c_char,
        size: usize,
    ) -> usize {
        if src.is_null() {
            return 0;
        }
        let src_len = unsafe { core::ffi::CStr::from_ptr(src) }.to_bytes().len();
        if size > 0 && !dst.is_null() {
            let copy_len = src_len.min(size - 1);
            unsafe {
                core::ptr::copy_nonoverlapping(src, dst, copy_len);
                *dst.add(copy_len) = 0;
            }
        }
        src_len
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_strxfrm(
        dest: *mut c_char,
        src: *const c_char,
        n: usize,
    ) -> usize {
        if src.is_null() {
            return 0;
        }
        let len = unsafe { core::ffi::CStr::from_ptr(src) }.to_bytes().len();
        if n > 0 && !dest.is_null() {
            let copy_len = len.min(n - 1);
            unsafe {
                core::ptr::copy_nonoverlapping(src, dest, copy_len);
                *dest.add(copy_len) = 0;
            }
        }
        len
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___xpg_basename(path: *mut c_char) -> *mut c_char {
        unsafe { crate::fsextra::kinakaze_abi_basename(path) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_qsort_r(
        base: *mut c_void,
        nmemb: usize,
        size: usize,
        compar: Option<
            unsafe extern "sysv64" fn(*const c_void, *const c_void, *mut c_void) -> c_int,
        >,
        arg: *mut c_void,
    ) {
        if base.is_null() || nmemb <= 1 || size == 0 {
            return;
        }
        let Some(cmp) = compar else {
            return;
        };
        let ptr = base as *mut u8;
        for i in 1..nmemb {
            let mut j = i;
            while j > 0 {
                let elem_a = unsafe { ptr.add((j - 1) * size) };
                let elem_b = unsafe { ptr.add(j * size) };
                if unsafe { cmp(elem_a.cast(), elem_b.cast(), arg) } > 0 {
                    for k in 0..size {
                        unsafe {
                            let tmp = *elem_a.add(k);
                            *elem_a.add(k) = *elem_b.add(k);
                            *elem_b.add(k) = tmp;
                        }
                    }
                    j -= 1;
                } else {
                    break;
                }
            }
        }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn dcngettext(
        _domain: *const c_char,
        msgid1: *const c_char,
        msgid2: *const c_char,
        n: usize,
        _category: c_int,
    ) -> *const c_char {
        if n == 1 { msgid1 } else { msgid2 }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_dcngettext(
        _domain: *const c_char,
        msgid1: *const c_char,
        msgid2: *const c_char,
        n: usize,
        _category: c_int,
    ) -> *const c_char {
        if n == 1 { msgid1 } else { msgid2 }
    }

    // The current locale profile has no translated catalogs. Share the normal
    // singular/plural fallback with dcngettext (LC_MESSAGES = 5 on Linux).
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_dngettext(
        domain: *const c_char,
        singular: *const c_char,
        plural: *const c_char,
        count: usize,
    ) -> *const c_char {
        unsafe { kinakaze_abi_dcngettext(domain, singular, plural, count, 5) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_ngettext(
        singular: *const c_char,
        plural: *const c_char,
        count: usize,
    ) -> *const c_char {
        unsafe { kinakaze_abi_dngettext(std::ptr::null(), singular, plural, count) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn mbsinit(ps: *const c_void) -> c_int {
        unsafe { crate::uchar::state_is_initial(ps.cast()) as c_int }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_mbsinit(ps: *const c_void) -> c_int {
        unsafe { crate::uchar::state_is_initial(ps.cast()) as c_int }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn wctype(name: *const c_char) -> usize {
        unsafe { crate::locale::wide::kinakaze_abi_wctype(name) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn iswctype(wc: u32, desc: usize) -> c_int {
        crate::locale::wide::kinakaze_abi_iswctype(wc, desc)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn rpmatch(response: *const c_char) -> c_int {
        if response.is_null() {
            return -1;
        }
        let b = unsafe { *response as u8 };
        if b == b'y' || b == b'Y' {
            1
        } else if b == b'n' || b == b'N' {
            0
        } else {
            -1
        }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_rpmatch(response: *const c_char) -> c_int {
        unsafe { rpmatch(response) }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::ffi::CString;

        #[test]
        fn stpcpy_returns_the_terminator() {
            let source = CString::new("abc").unwrap();
            let mut buffer = [0i8; 8];
            // SAFETY: the buffer is larger than the source.
            let end = unsafe { kinakaze_abi_stpcpy(buffer.as_mut_ptr(), source.as_ptr()) };
            // The whole reason to use stpcpy is that this points at the NUL.
            assert_eq!(end, unsafe { buffer.as_mut_ptr().add(3) });
            // SAFETY: within the buffer.
            assert_eq!(unsafe { *end }, 0);
        }

        #[test]
        fn stpncpy_reports_one_past_the_end_when_it_fills() {
            let source = CString::new("abcdef").unwrap();
            let mut buffer = [1i8; 4];
            // SAFETY: the buffer holds 4 bytes.
            let end = unsafe { kinakaze_abi_stpncpy(buffer.as_mut_ptr(), source.as_ptr(), 4) };
            // No terminator was written, so the result is past the end, not at a NUL.
            assert_eq!(end, unsafe { buffer.as_mut_ptr().add(4) });
            assert_eq!(&buffer, &[b'a' as i8, b'b' as i8, b'c' as i8, b'd' as i8]);
        }

        #[test]
        fn strchrnul_returns_the_terminator_when_absent() {
            let text = CString::new("hello").unwrap();
            // SAFETY: the string is NUL-terminated.
            let found = unsafe { kinakaze_abi_strchrnul(text.as_ptr(), i32::from(b'z')) };
            // strchr would give null here; the difference is the entire point.
            // SAFETY: the returned pointer is inside the string.
            assert_eq!(unsafe { *found }, 0);
            // SAFETY: as above.
            let hit = unsafe { kinakaze_abi_strchrnul(text.as_ptr(), i32::from(b'l')) };
            assert_eq!(hit, unsafe { text.as_ptr().add(2).cast_mut() });
        }

        #[test]
        fn memrchr_finds_the_last_match() {
            let data = b"abcabc";
            // SAFETY: the slice is readable for its length.
            let found =
                unsafe { kinakaze_abi_memrchr(data.as_ptr().cast(), i32::from(b'b'), data.len()) };
            assert_eq!(found as usize, data.as_ptr() as usize + 4);
        }

        #[test]
        fn case_insensitive_comparison_folds_only_ascii() {
            let upper = CString::new("HELLO").unwrap();
            let lower = CString::new("hello").unwrap();
            // SAFETY: both are NUL-terminated.
            assert_eq!(
                unsafe { kinakaze_abi_strcasecmp(upper.as_ptr(), lower.as_ptr()) },
                0
            );
            let other = CString::new("hellp").unwrap();
            // SAFETY: as above.
            assert!(unsafe { kinakaze_abi_strcasecmp(upper.as_ptr(), other.as_ptr()) } < 0);
            // SAFETY: as above, bounded to the common prefix.
            assert_eq!(
                unsafe { kinakaze_abi_strncasecmp(upper.as_ptr(), other.as_ptr(), 4) },
                0
            );
        }

        #[test]
        fn strsep_reports_empty_fields() {
            // This is the difference from strtok, and the reason passwd-style
            // parsers use it: `a::b` has three fields, the middle one empty.
            let mut owned = *b"a::b\0";
            let mut cursor = owned.as_mut_ptr().cast::<c_char>();
            let delimiters = CString::new(":").unwrap();
            let mut fields = Vec::new();
            // SAFETY: `cursor` walks a writable NUL-terminated buffer.
            unsafe {
                while !cursor.is_null() {
                    let token = kinakaze_abi_strsep(&raw mut cursor, delimiters.as_ptr());
                    if token.is_null() {
                        break;
                    }
                    let text = std::ffi::CStr::from_ptr(token)
                        .to_string_lossy()
                        .into_owned();
                    fields.push(text);
                }
            }
            assert_eq!(fields, vec!["a".to_owned(), String::new(), "b".to_owned()]);
        }

        #[test]
        fn strverscmp_orders_digit_runs_numerically() {
            let compare = |a: &str, b: &str| {
                let a = CString::new(a).unwrap();
                let b = CString::new(b).unwrap();
                // SAFETY: both are NUL-terminated.
                unsafe { kinakaze_abi_strverscmp(a.as_ptr(), b.as_ptr()) }
            };
            // The case a byte comparison gets backwards.
            assert!(compare("file9", "file10") < 0);
            assert!(compare("file10", "file9") > 0);
            assert_eq!(compare("file10", "file10"), 0);
            // Leading zeroes do not change magnitude.
            assert_eq!(compare("file08", "file8"), 0);
            // A prefix sorts first.
            assert!(compare("file", "file1") < 0);
        }

        #[test]
        fn mempcpy_returns_the_end_of_the_destination() {
            let source = [1u8, 2, 3, 4];
            let mut destination = [0u8; 4];
            // SAFETY: both regions hold 4 bytes and do not overlap.
            let end = unsafe {
                kinakaze_abi_mempcpy(destination.as_mut_ptr().cast(), source.as_ptr().cast(), 4)
            };
            assert_eq!(destination, source);
            assert_eq!(end as usize, destination.as_ptr() as usize + 4);
        }
    }
}
