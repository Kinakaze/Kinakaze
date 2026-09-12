//! POSIX and GNU regular expressions.
#![allow(non_camel_case_types)]

use core::ffi::{c_char, c_int};
use core::ptr;

// POSIX cflags
pub const REG_EXTENDED: c_int = 1;
pub const REG_ICASE: c_int = 2;
pub const REG_NOSUB: c_int = 4;
pub const REG_NEWLINE: c_int = 8;

// POSIX eflags
pub const REG_NOTBOL: c_int = 1;
pub const REG_NOTEOL: c_int = 2;

// POSIX error codes
pub const REG_NOERROR: c_int = 0;
pub const REG_NOMATCH: c_int = 1;
pub const REG_BADPAT: c_int = 2;
pub const REG_ECOLLATE: c_int = 3;
pub const REG_ECTYPE: c_int = 4;
pub const REG_EESCAPE: c_int = 5;
pub const REG_ESUBREG: c_int = 6;
pub const REG_EBRACK: c_int = 7;
pub const REG_EPAREN: c_int = 8;
pub const REG_EBRACE: c_int = 9;
pub const REG_BADBR: c_int = 10;
pub const REG_ERANGE: c_int = 11;
pub const REG_ESPACE: c_int = 12;
pub const REG_BADRPT: c_int = 13;

pub type regoff_t = c_int;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct regmatch_t {
    pub rm_so: regoff_t,
    pub rm_eo: regoff_t,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct re_pattern_buffer {
    pub buffer: *mut u8,
    pub allocated: usize,
    pub used: usize,
    pub syntax: usize,
    pub fastmap: *mut c_char,
    pub translate: *mut u8,
    pub re_nsub: usize,
    pub flags: u32,
}

pub type regex_t = re_pattern_buffer;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct re_registers {
    pub num_regs: u32,
    pub start: *mut regoff_t,
    pub end: *mut regoff_t,
}

#[derive(Clone, Debug)]
enum RegexAst {
    Empty,
    Literal(u8),
    AnyChar,
    StartOfLine,
    EndOfLine,
    CharClass(Box<[bool; 256]>),
    Concat(Vec<RegexAst>),
    Alt(Vec<RegexAst>),
    Repeat(Box<RegexAst>, usize, usize), // min, max (usize::MAX for unlimited)
    Group(usize, Box<RegexAst>),         // group_index, sub_ast
}

struct CompiledPattern {
    ast: RegexAst,
    num_groups: usize,
    icase: bool,
    newline: bool,
}

struct Parser<'a> {
    chars: &'a [u8],
    pos: usize,
    extended: bool,
    group_counter: usize,
}

impl<'a> Parser<'a> {
    fn new(chars: &'a [u8], extended: bool) -> Self {
        Self {
            chars,
            pos: 0,
            extended,
            group_counter: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.chars.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let ch = self.chars.get(self.pos).copied()?;
        self.pos += 1;
        Some(ch)
    }

    fn parse(&mut self) -> Result<RegexAst, c_int> {
        let ast = self.parse_alt()?;
        if self.pos < self.chars.len() {
            return Err(REG_EPAREN);
        }
        Ok(ast)
    }

    fn parse_alt(&mut self) -> Result<RegexAst, c_int> {
        let mut branches = vec![self.parse_concat()?];
        while let Some(ch) = self.peek() {
            if self.extended && ch == b'|' {
                self.next();
                branches.push(self.parse_concat()?);
            } else if !self.extended && ch == b'\\' && self.chars.get(self.pos + 1) == Some(&b'|') {
                self.pos += 2;
                branches.push(self.parse_concat()?);
            } else {
                break;
            }
        }
        if branches.len() == 1 {
            Ok(branches.pop().unwrap())
        } else {
            Ok(RegexAst::Alt(branches))
        }
    }

    fn parse_concat(&mut self) -> Result<RegexAst, c_int> {
        let mut terms = Vec::new();
        while let Some(ch) = self.peek() {
            if self.extended && (ch == b'|' || ch == b')') {
                break;
            }
            if !self.extended && ch == b'\\' {
                let next_ch = self.chars.get(self.pos + 1).copied();
                if next_ch == Some(b'|') || next_ch == Some(b')') {
                    break;
                }
            }
            let atom = self.parse_quantified()?;
            terms.push(atom);
        }
        if terms.is_empty() {
            Ok(RegexAst::Empty)
        } else if terms.len() == 1 {
            Ok(terms.pop().unwrap())
        } else {
            Ok(RegexAst::Concat(terms))
        }
    }

    fn parse_quantified(&mut self) -> Result<RegexAst, c_int> {
        let atom = self.parse_atom()?;
        let (min, max) = if self.extended {
            match self.peek() {
                Some(b'*') => {
                    self.next();
                    (0, usize::MAX)
                }
                Some(b'+') => {
                    self.next();
                    (1, usize::MAX)
                }
                Some(b'?') => {
                    self.next();
                    (0, 1)
                }
                Some(b'{') => {
                    self.next();
                    self.parse_brace_bounds()?
                }
                _ => return Ok(atom),
            }
        } else {
            match self.peek() {
                Some(b'*') => {
                    self.next();
                    (0, usize::MAX)
                }
                Some(b'\\') => {
                    let next = self.chars.get(self.pos + 1).copied();
                    if next == Some(b'+') {
                        self.pos += 2;
                        (1, usize::MAX)
                    } else if next == Some(b'?') {
                        self.pos += 2;
                        (0, 1)
                    } else if next == Some(b'{') {
                        self.pos += 2;
                        self.parse_brace_bounds_bre()?
                    } else {
                        return Ok(atom);
                    }
                }
                _ => return Ok(atom),
            }
        };
        Ok(RegexAst::Repeat(Box::new(atom), min, max))
    }

    fn parse_brace_bounds(&mut self) -> Result<(usize, usize), c_int> {
        let mut min = 0usize;
        while let Some(ch) = self.peek()
            && ch.is_ascii_digit()
        {
            self.next();
            min = min.saturating_mul(10).saturating_add((ch - b'0') as usize);
        }
        let mut max = min;
        if self.peek() == Some(b',') {
            self.next();
            if self.peek() == Some(b'}') {
                max = usize::MAX;
            } else {
                max = 0;
                while let Some(ch) = self.peek()
                    && ch.is_ascii_digit()
                {
                    self.next();
                    max = max.saturating_mul(10).saturating_add((ch - b'0') as usize);
                }
            }
        }
        if self.next() != Some(b'}') {
            return Err(REG_EBRACE);
        }
        if min > max {
            return Err(REG_BADBR);
        }
        Ok((min, max))
    }

    fn parse_brace_bounds_bre(&mut self) -> Result<(usize, usize), c_int> {
        let mut min = 0usize;
        while let Some(ch) = self.peek()
            && ch.is_ascii_digit()
        {
            self.next();
            min = min.saturating_mul(10).saturating_add((ch - b'0') as usize);
        }
        let mut max = min;
        if self.peek() == Some(b',') {
            self.next();
            if self.peek() == Some(b'\\') && self.chars.get(self.pos + 1) == Some(&b'}') {
                max = usize::MAX;
            } else {
                max = 0;
                while let Some(ch) = self.peek()
                    && ch.is_ascii_digit()
                {
                    self.next();
                    max = max.saturating_mul(10).saturating_add((ch - b'0') as usize);
                }
            }
        }
        if self.next() != Some(b'\\') || self.next() != Some(b'}') {
            return Err(REG_EBRACE);
        }
        if min > max {
            return Err(REG_BADBR);
        }
        Ok((min, max))
    }

    fn parse_atom(&mut self) -> Result<RegexAst, c_int> {
        let ch = self.next().ok_or(REG_BADPAT)?;
        match ch {
            b'.' => Ok(RegexAst::AnyChar),
            b'^' => Ok(RegexAst::StartOfLine),
            b'$' => Ok(RegexAst::EndOfLine),
            b'[' => self.parse_bracket(),
            b'(' if self.extended => {
                self.group_counter += 1;
                let group_idx = self.group_counter;
                let inner = self.parse_alt()?;
                if self.next() != Some(b')') {
                    return Err(REG_EPAREN);
                }
                Ok(RegexAst::Group(group_idx, Box::new(inner)))
            }
            b'\\' => {
                let escaped = self.next().ok_or(REG_EESCAPE)?;
                if !self.extended && escaped == b'(' {
                    self.group_counter += 1;
                    let group_idx = self.group_counter;
                    let inner = self.parse_alt()?;
                    if self.next() != Some(b'\\') || self.next() != Some(b')') {
                        return Err(REG_EPAREN);
                    }
                    Ok(RegexAst::Group(group_idx, Box::new(inner)))
                } else {
                    let lit = match escaped {
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        b'0' => 0,
                        other => other,
                    };
                    Ok(RegexAst::Literal(lit))
                }
            }
            other => Ok(RegexAst::Literal(other)),
        }
    }

    fn parse_bracket(&mut self) -> Result<RegexAst, c_int> {
        let mut invert = false;
        if self.peek() == Some(b'^') {
            invert = true;
            self.next();
        }
        let mut class = Box::new([false; 256]);
        let mut first = true;

        while let Some(ch) = self.next() {
            if ch == b']' && !first {
                break;
            }
            first = false;
            if ch == b'[' && self.peek() == Some(b':') {
                // Character class like [[:alnum:]]
                self.next();
                let mut name = Vec::new();
                while let Some(c) = self.next() {
                    if c == b':' && self.peek() == Some(b']') {
                        self.next();
                        break;
                    }
                    name.push(c);
                }
                match name.as_slice() {
                    b"alnum" => {
                        for b in 0..=255u8 {
                            if b.is_ascii_alphanumeric() {
                                class[b as usize] = true;
                            }
                        }
                    }
                    b"alpha" => {
                        for b in 0..=255u8 {
                            if b.is_ascii_alphabetic() {
                                class[b as usize] = true;
                            }
                        }
                    }
                    b"digit" => {
                        for b in 0..=255u8 {
                            if b.is_ascii_digit() {
                                class[b as usize] = true;
                            }
                        }
                    }
                    b"space" => {
                        for b in 0..=255u8 {
                            if b.is_ascii_whitespace() {
                                class[b as usize] = true;
                            }
                        }
                    }
                    b"blank" => {
                        class[b' ' as usize] = true;
                        class[b'\t' as usize] = true;
                    }
                    b"punct" => {
                        for b in 0..=255u8 {
                            if b.is_ascii_punctuation() {
                                class[b as usize] = true;
                            }
                        }
                    }
                    b"xdigit" => {
                        for b in 0..=255u8 {
                            if b.is_ascii_hexdigit() {
                                class[b as usize] = true;
                            }
                        }
                    }
                    _ => {}
                }
                continue;
            }

            if self.peek() == Some(b'-') && self.chars.get(self.pos + 1) != Some(&b']') {
                self.next(); // skip '-'
                let end = self.next().ok_or(REG_EBRACK)?;
                for b in ch..=end {
                    class[b as usize] = true;
                }
            } else {
                class[ch as usize] = true;
            }
        }

        if invert {
            for b in 0..=255 {
                class[b] = !class[b];
            }
        }
        Ok(RegexAst::CharClass(class))
    }
}

// Matcher engine with capturing groups
impl CompiledPattern {
    fn match_at<'t>(
        &self,
        text: &'t [u8],
        start_pos: usize,
        eflags: c_int,
    ) -> Option<Vec<(isize, isize)>> {
        let mut captures = vec![(-1isize, -1isize); self.num_groups + 1];
        if let Some(end) = self.match_step(&self.ast, text, start_pos, eflags, &mut captures) {
            captures[0] = (start_pos as isize, end as isize);
            Some(captures)
        } else {
            None
        }
    }

    fn match_step(
        &self,
        node: &RegexAst,
        text: &[u8],
        pos: usize,
        eflags: c_int,
        captures: &mut [(isize, isize)],
    ) -> Option<usize> {
        match node {
            RegexAst::Empty => Some(pos),
            RegexAst::Literal(lit) => {
                let ch = text.get(pos).copied()?;
                let matches = if self.icase {
                    ch.to_ascii_lowercase() == lit.to_ascii_lowercase()
                } else {
                    ch == *lit
                };
                if matches { Some(pos + 1) } else { None }
            }
            RegexAst::AnyChar => {
                let ch = text.get(pos).copied()?;
                if self.newline && ch == b'\n' {
                    None
                } else {
                    Some(pos + 1)
                }
            }
            RegexAst::StartOfLine => {
                if pos == 0 {
                    if (eflags & REG_NOTBOL) == 0 {
                        Some(pos)
                    } else {
                        None
                    }
                } else if text.get(pos - 1) == Some(&b'\n') {
                    Some(pos)
                } else {
                    None
                }
            }
            RegexAst::EndOfLine => {
                if pos == text.len() {
                    if (eflags & REG_NOTEOL) == 0 {
                        Some(pos)
                    } else {
                        None
                    }
                } else if text.get(pos) == Some(&b'\n') {
                    Some(pos)
                } else {
                    None
                }
            }
            RegexAst::CharClass(class) => {
                let ch = text.get(pos).copied()?;
                let matches = if self.icase {
                    class[ch.to_ascii_lowercase() as usize]
                        || class[ch.to_ascii_uppercase() as usize]
                } else {
                    class[ch as usize]
                };
                if matches { Some(pos + 1) } else { None }
            }
            RegexAst::Concat(nodes) => {
                let mut cur = pos;
                for n in nodes {
                    cur = self.match_step(n, text, cur, eflags, captures)?;
                }
                Some(cur)
            }
            RegexAst::Alt(branches) => {
                for b in branches {
                    let mut saved = captures.to_vec();
                    if let Some(end) = self.match_step(b, text, pos, eflags, &mut saved) {
                        captures.copy_from_slice(&saved);
                        return Some(end);
                    }
                }
                None
            }
            RegexAst::Group(idx, inner) => {
                let start = pos as isize;
                let end = self.match_step(inner, text, pos, eflags, captures)?;
                if *idx < captures.len() {
                    captures[*idx] = (start, end as isize);
                }
                Some(end)
            }
            RegexAst::Repeat(inner, min, max) => {
                self.match_repeat(inner, *min, *max, text, pos, eflags, captures)
            }
        }
    }

    fn match_repeat(
        &self,
        inner: &RegexAst,
        min: usize,
        max: usize,
        text: &[u8],
        pos: usize,
        eflags: c_int,
        captures: &mut [(isize, isize)],
    ) -> Option<usize> {
        let mut matches = Vec::new();
        let mut cur = pos;
        let mut count = 0;

        while count < max {
            let mut snap = captures.to_vec();
            if let Some(next_pos) = self.match_step(inner, text, cur, eflags, &mut snap) {
                if next_pos == cur {
                    // Empty match, avoid infinite loop
                    break;
                }
                matches.push((cur, next_pos, snap));
                cur = next_pos;
                count += 1;
            } else {
                break;
            }
        }

        if count < min {
            return None;
        }

        // Try greedy from longest match down to min
        for k in (min..=count).rev() {
            if k == 0 {
                return Some(pos);
            }
            let (_, end_pos, ref snap) = matches[k - 1];
            captures.copy_from_slice(snap);
            return Some(end_pos);
        }
        None
    }
}

/// POSIX `regcomp(3)`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_regcomp(
    preg: *mut regex_t,
    pattern: *const c_char,
    cflags: c_int,
) -> c_int {
    if preg.is_null() || pattern.is_null() {
        return REG_BADPAT;
    }
    let len = unsafe { crate::string::strlen(pattern) };
    let bytes = unsafe { core::slice::from_raw_parts(pattern as *const u8, len) };
    let extended = (cflags & REG_EXTENDED) != 0;
    let icase = (cflags & REG_ICASE) != 0;
    let newline = (cflags & REG_NEWLINE) != 0;

    let mut parser = Parser::new(bytes, extended);
    let ast = match parser.parse() {
        Ok(ast) => ast,
        Err(err) => return err,
    };

    let compiled = Box::new(CompiledPattern {
        ast,
        num_groups: parser.group_counter,
        icase,
        newline,
    });

    unsafe {
        (*preg).re_nsub = compiled.num_groups;
        (*preg).buffer = Box::into_raw(compiled) as *mut u8;
        (*preg).allocated = 1;
        (*preg).used = 1;
    }
    REG_NOERROR
}

/// POSIX `regexec(3)`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_regexec(
    preg: *const regex_t,
    string: *const c_char,
    nmatch: usize,
    pmatch: *mut regmatch_t,
    eflags: c_int,
) -> c_int {
    if preg.is_null() || string.is_null() || unsafe { (*preg).buffer.is_null() } {
        return REG_NOMATCH;
    }
    let compiled = unsafe { &*((*preg).buffer as *const CompiledPattern) };
    let len = unsafe { crate::string::strlen(string) };
    let bytes = unsafe { core::slice::from_raw_parts(string as *const u8, len) };

    // Search anywhere in string
    for start in 0..=bytes.len() {
        if let Some(mut caps) = compiled.match_at(bytes, start, eflags) {
            caps[0].0 = start as isize;
            if !pmatch.is_null() && nmatch > 0 {
                for i in 0..nmatch {
                    if i < caps.len() {
                        let (s, e) = caps[i];
                        unsafe {
                            *pmatch.add(i) = regmatch_t {
                                rm_so: s as regoff_t,
                                rm_eo: e as regoff_t,
                            };
                        }
                    } else {
                        unsafe {
                            *pmatch.add(i) = regmatch_t {
                                rm_so: -1,
                                rm_eo: -1,
                            };
                        }
                    }
                }
            }
            return REG_NOERROR;
        }
    }
    REG_NOMATCH
}

/// POSIX `regerror(3)`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_regerror(
    errcode: c_int,
    _preg: *const regex_t,
    errbuf: *mut c_char,
    errbuf_size: usize,
) -> usize {
    let msg: &[u8] = match errcode {
        REG_NOERROR => b"Success\0",
        REG_NOMATCH => b"No match\0",
        REG_BADPAT => b"Invalid regular expression\0",
        REG_ECOLLATE => b"Invalid collating element\0",
        REG_ECTYPE => b"Invalid character class name\0",
        REG_EESCAPE => b"Trailing backslash\0",
        REG_ESUBREG => b"Invalid back reference\0",
        REG_EBRACK => b"Unmatched [, [^, [:, [., or [=\0",
        REG_EPAREN => b"Unmatched ( or \\(\0",
        REG_EBRACE => b"Unmatched \\{\0",
        REG_BADBR => b"Invalid content of \\{\\}\0",
        REG_ERANGE => b"Invalid range end\0",
        REG_ESPACE => b"Memory exhausted\0",
        REG_BADRPT => b"Invalid preceding regular expression\0",
        _ => b"Unknown regex error\0",
    };
    if !errbuf.is_null() && errbuf_size > 0 {
        let copy_len = msg.len().min(errbuf_size);
        unsafe {
            ptr::copy_nonoverlapping(msg.as_ptr(), errbuf as *mut u8, copy_len);
            if copy_len > 0 {
                *errbuf.add(copy_len - 1) = 0;
            }
        }
    }
    msg.len()
}

/// POSIX `regfree(3)`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_regfree(preg: *mut regex_t) {
    if !preg.is_null() && !unsafe { (*preg).buffer.is_null() } {
        let raw = unsafe { (*preg).buffer as *mut CompiledPattern };
        unsafe {
            drop(Box::from_raw(raw));
            (*preg).buffer = ptr::null_mut();
        }
    }
}

/// GNU `re_compile_pattern`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_re_compile_pattern(
    pattern: *const c_char,
    length: usize,
    buffer: *mut re_pattern_buffer,
) -> *const c_char {
    if pattern.is_null() || buffer.is_null() {
        return c"Invalid argument".as_ptr();
    }
    let bytes = unsafe { core::slice::from_raw_parts(pattern as *const u8, length) };
    // GNU callers (including sed -E) choose syntax with re_set_syntax rather
    // than POSIX regcomp flags. Preserve that choice in this compiled object.
    let syntax = RE_SYNTAX_OPTIONS.load(core::sync::atomic::Ordering::Acquire);
    let mut parser = Parser::new(bytes, syntax & (1 << 13) != 0); // RE_NO_BK_PARENS
    let ast = match parser.parse() {
        Ok(ast) => ast,
        Err(_) => return c"Invalid regular expression".as_ptr(),
    };

    let compiled = Box::new(CompiledPattern {
        ast,
        num_groups: parser.group_counter,
        icase: syntax & (1 << 22) != 0,  // RE_ICASE
        newline: syntax & (1 << 6) == 0, // !RE_DOT_NEWLINE
    });

    unsafe {
        (*buffer).syntax = syntax;
        (*buffer).re_nsub = compiled.num_groups;
        (*buffer).buffer = Box::into_raw(compiled) as *mut u8;
        (*buffer).allocated = 1;
        (*buffer).used = 1;
    }
    ptr::null()
}

/// GNU `re_search`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_re_search(
    buffer: *mut re_pattern_buffer,
    string: *const c_char,
    length: c_int,
    start: c_int,
    range: c_int,
    regs: *mut re_registers,
) -> c_int {
    if buffer.is_null() || string.is_null() || unsafe { (*buffer).buffer.is_null() } {
        return -1;
    }
    let compiled = unsafe { &*((*buffer).buffer as *const CompiledPattern) };
    let full_bytes =
        unsafe { core::slice::from_raw_parts(string as *const u8, length.max(0) as usize) };

    let start_idx = start.max(0) as usize;
    let end_idx = (start + range).max(0) as usize;
    let (search_from, search_to) = if start_idx <= end_idx {
        (start_idx, end_idx.min(full_bytes.len()))
    } else {
        (end_idx, start_idx.min(full_bytes.len()))
    };

    for pos in search_from..=search_to {
        if let Some(caps) = compiled.match_at(full_bytes, pos, 0) {
            if !regs.is_null() {
                let num_regs = (compiled.num_groups + 1).max(1);
                unsafe {
                    if (*regs).start.is_null() || (*regs).end.is_null() {
                        let size = num_regs * core::mem::size_of::<regoff_t>();
                        (*regs).num_regs = num_regs as u32;
                        (*regs).start = crate::c_malloc(size) as *mut regoff_t;
                        (*regs).end = crate::c_malloc(size) as *mut regoff_t;
                    }
                    let count = (*regs).num_regs as usize;
                    for i in 0..count {
                        if i < caps.len() {
                            let (s, e) = caps[i];
                            if !(*regs).start.is_null() {
                                *(*regs).start.add(i) = s as regoff_t;
                            }
                            if !(*regs).end.is_null() {
                                *(*regs).end.add(i) = e as regoff_t;
                            }
                        } else {
                            if !(*regs).start.is_null() {
                                *(*regs).start.add(i) = -1;
                            }
                            if !(*regs).end.is_null() {
                                *(*regs).end.add(i) = -1;
                            }
                        }
                    }
                }
            }
            return pos as c_int;
        }
    }
    -1
}

pub type reg_syntax_t = usize;

static RE_SYNTAX_OPTIONS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[unsafe(no_mangle)]
pub static mut re_syntax_options: reg_syntax_t = 0;

/// GNU `re_set_syntax`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_re_set_syntax(syntax: reg_syntax_t) -> reg_syntax_t {
    let prev = RE_SYNTAX_OPTIONS.swap(syntax, core::sync::atomic::Ordering::SeqCst);
    unsafe {
        re_syntax_options = syntax;
    }
    prev
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn re_set_syntax(syntax: reg_syntax_t) -> reg_syntax_t {
    kinakaze_abi_re_set_syntax(syntax)
}

/// GNU `re_compile_fastmap`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_re_compile_fastmap(
    _buffer: *mut re_pattern_buffer,
) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn re_compile_fastmap(buffer: *mut re_pattern_buffer) -> c_int {
    kinakaze_abi_re_compile_fastmap(buffer)
}

/// GNU `re_match`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_re_match(
    buffer: *mut re_pattern_buffer,
    string: *const c_char,
    length: c_int,
    start: c_int,
    regs: *mut re_registers,
) -> c_int {
    kinakaze_abi_re_search(buffer, string, length, start, 0, regs)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn re_match(
    buffer: *mut re_pattern_buffer,
    string: *const c_char,
    length: c_int,
    start: c_int,
    regs: *mut re_registers,
) -> c_int {
    kinakaze_abi_re_match(buffer, string, length, start, regs)
}

/// GNU `re_set_registers`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_re_set_registers(
    buffer: *mut re_pattern_buffer,
    regs: *mut re_registers,
    num_regs: u32,
    starts: *mut regoff_t,
    ends: *mut regoff_t,
) {
    if !regs.is_null() {
        unsafe {
            (*regs).num_regs = num_regs;
            (*regs).start = starts;
            (*regs).end = ends;
        }
    }
    if !buffer.is_null() {
        unsafe {
            (*buffer).re_nsub = num_regs as usize;
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn re_set_registers(
    buffer: *mut re_pattern_buffer,
    regs: *mut re_registers,
    num_regs: u32,
    starts: *mut regoff_t,
    ends: *mut regoff_t,
) {
    unsafe {
        kinakaze_abi_re_set_registers(buffer, regs, num_regs, starts, ends);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn regcomp(
    preg: *mut regex_t,
    pattern: *const c_char,
    cflags: c_int,
) -> c_int {
    unsafe { kinakaze_abi_regcomp(preg, pattern, cflags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn regexec(
    preg: *const regex_t,
    string: *const c_char,
    nmatch: usize,
    pmatch: *mut regmatch_t,
    eflags: c_int,
) -> c_int {
    unsafe { kinakaze_abi_regexec(preg, string, nmatch, pmatch, eflags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn regerror(
    errcode: c_int,
    preg: *const regex_t,
    errbuf: *mut c_char,
    errbuf_size: usize,
) -> usize {
    unsafe { kinakaze_abi_regerror(errcode, preg, errbuf, errbuf_size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn regfree(preg: *mut regex_t) {
    unsafe { kinakaze_abi_regfree(preg) };
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn re_compile_pattern(
    pattern: *const c_char,
    length: usize,
    buffer: *mut re_pattern_buffer,
) -> *const c_char {
    unsafe { kinakaze_abi_re_compile_pattern(pattern, length, buffer) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn re_search(
    buffer: *mut re_pattern_buffer,
    string: *const c_char,
    length: c_int,
    start: c_int,
    range: c_int,
    regs: *mut re_registers,
) -> c_int {
    unsafe { kinakaze_abi_re_search(buffer, string, length, start, range, regs) }
}
