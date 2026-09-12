//! GNU glibc `argp` argument parsing interface.
//!
//! Provides `argp_parse`, `argp_help`, `argp_state_help`, `argp_error`,
//! `argp_failure`, `argp_usage`, and associated hooks used by tools like `crun`,
//! `tar`, `coreutils`, etc.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_uint, c_void};
use std::ptr;

pub const ARGP_KEY_ARG: c_int = 0;
pub const ARGP_KEY_END: c_int = 0x1000001;
pub const ARGP_KEY_NO_ARGS: c_int = 0x1000002;
pub const ARGP_KEY_INIT: c_int = 0x1000003;
pub const ARGP_KEY_SUCCESS: c_int = 0x1000004;
pub const ARGP_KEY_ERROR: c_int = 0x1000005;
pub const ARGP_KEY_ARGS: c_int = 0x1000006;
pub const ARGP_KEY_FINI: c_int = 0x1000007;

pub const ARGP_ERR_UNKNOWN: c_int = 7; // E2BIG in Linux

pub const OPTION_ARG_OPTIONAL: c_int = 0x1;
pub const OPTION_HIDDEN: c_int = 0x2;
pub const OPTION_ALIAS: c_int = 0x4;
pub const OPTION_DOC: c_int = 0x8;
pub const OPTION_NO_USAGE: c_int = 0x10;

pub const ARGP_PARSE_ARGV0: c_uint = 0x01;
pub const ARGP_NO_ERRS: c_uint = 0x02;
pub const ARGP_NO_ARGS: c_uint = 0x04;
pub const ARGP_IN_ORDER: c_uint = 0x08;
pub const ARGP_NO_HELP: c_uint = 0x10;
pub const ARGP_NO_EXIT: c_uint = 0x20;
pub const ARGP_LONG_ONLY: c_uint = 0x40;
pub const ARGP_SILENT: c_uint = ARGP_NO_ERRS | ARGP_NO_HELP | ARGP_NO_EXIT;

pub const ARGP_HELP_USAGE: c_uint = 0x01;
pub const ARGP_HELP_SHORT_USAGE: c_uint = 0x02;
pub const ARGP_HELP_SEE: c_uint = 0x04;
pub const ARGP_HELP_LONG: c_uint = 0x08;
pub const ARGP_HELP_PRE_DOC: c_uint = 0x10;
pub const ARGP_HELP_POST_DOC: c_uint = 0x20;
pub const ARGP_HELP_DOC: c_uint = 0x30;
pub const ARGP_HELP_BUG_ADDR: c_uint = 0x40;
pub const ARGP_HELP_LONG_ONLY: c_uint = 0x80;
pub const ARGP_HELP_EXIT_OK: c_uint = 0x100;
pub const ARGP_HELP_EXIT_ERR: c_uint = 0x200;
pub const ARGP_HELP_STD_USAGE: c_uint = 0x07;
pub const ARGP_HELP_STD_HELP: c_uint = 0x7f;

#[repr(C)]
pub struct ArgpOption {
    pub name: *const c_char,
    pub key: c_int,
    pub arg: *const c_char,
    pub flags: c_int,
    pub doc: *const c_char,
    pub group: c_int,
}

pub type ArgpParserFn =
    unsafe extern "sysv64" fn(key: c_int, arg: *mut c_char, state: *mut ArgpState) -> c_int;

#[repr(C)]
pub struct ArgpChild {
    pub argp: *const Argp,
    pub flags: c_int,
    pub header: *const c_char,
    pub group: c_int,
}

#[repr(C)]
pub struct Argp {
    pub options: *const ArgpOption,
    pub parser: Option<ArgpParserFn>,
    pub args_doc: *const c_char,
    pub doc: *const c_char,
    pub children: *const ArgpChild,
    pub help_filter: Option<
        unsafe extern "sysv64" fn(
            key: c_int,
            text: *const c_char,
            input: *mut c_void,
        ) -> *mut c_char,
    >,
    pub argp_domain: *const c_char,
}

#[repr(C)]
pub struct ArgpState {
    pub root_argp: *const Argp,
    pub argc: c_int,
    pub argv: *mut *mut c_char,
    pub next: c_int,
    pub flags: c_uint,
    pub arg_num: c_uint,
    pub quoted: c_int,
    pub input: *mut c_void,
    pub child_inputs: *mut *mut c_void,
    pub hook: *mut c_void,
    pub name: *mut c_char,
    pub err_stream: *mut c_void,
    pub out_stream: *mut c_void,
    pub pstate: *mut c_void,
}

#[unsafe(no_mangle)]
pub static mut argp_program_version: *const c_char = ptr::null();

#[unsafe(no_mangle)]
pub static mut argp_program_version_hook: Option<
    unsafe extern "sysv64" fn(stream: *mut c_void, state: *mut ArgpState),
> = None;

#[unsafe(no_mangle)]
pub static mut argp_program_bug_address: *const c_char = ptr::null();

#[unsafe(no_mangle)]
pub static mut argp_err_exit_status: c_int = 64;

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_argp_parse(
    argp: *const Argp,
    argc: c_int,
    argv: *mut *mut c_char,
    flags: c_uint,
    arg_index: *mut c_int,
    input: *mut c_void,
) -> c_int {
    unsafe { argp_parse(argp, argc, argv, flags, arg_index, input) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn argp_parse(
    argp: *const Argp,
    argc: c_int,
    argv: *mut *mut c_char,
    flags: c_uint,
    arg_index: *mut c_int,
    input: *mut c_void,
) -> c_int {
    if argp.is_null() || argv.is_null() || argc < 0 {
        return crate::EINVAL;
    }

    let program_name = if argc > 0 && !(*argv).is_null() {
        *argv
    } else {
        ptr::null_mut()
    };

    let mut state = ArgpState {
        root_argp: argp,
        argc,
        argv,
        next: if (flags & ARGP_PARSE_ARGV0) != 0 {
            0
        } else {
            1
        },
        flags,
        arg_num: 0,
        quoted: 0,
        input,
        child_inputs: ptr::null_mut(),
        hook: ptr::null_mut(),
        name: program_name,
        err_stream: ptr::null_mut(),
        out_stream: ptr::null_mut(),
        pstate: ptr::null_mut(),
    };

    // Helper closure to invoke parser for an argp and its children
    unsafe fn dispatch_key(
        argp_ptr: *const Argp,
        key: c_int,
        arg: *mut c_char,
        state: *mut ArgpState,
    ) -> c_int {
        if argp_ptr.is_null() {
            return ARGP_ERR_UNKNOWN;
        }
        let a = &*argp_ptr;
        if let Some(parser) = a.parser {
            let res = parser(key, arg, state);
            if res != ARGP_ERR_UNKNOWN {
                return res;
            }
        }
        if !a.children.is_null() {
            let mut child_ptr = a.children;
            while !(*child_ptr).argp.is_null() {
                let res = dispatch_key((*child_ptr).argp, key, arg, state);
                if res != ARGP_ERR_UNKNOWN {
                    return res;
                }
                child_ptr = child_ptr.add(1);
            }
        }
        ARGP_ERR_UNKNOWN
    }

    // Helper to find option in argp or children
    unsafe fn find_option<'a>(
        argp_ptr: *const Argp,
        match_fn: &dyn Fn(&ArgpOption) -> bool,
    ) -> Option<(&'a ArgpOption, *const Argp)> {
        if argp_ptr.is_null() {
            return None;
        }
        let a = &*argp_ptr;
        if !a.options.is_null() {
            let mut opt = a.options;
            while (*opt).name != ptr::null() || (*opt).key != 0 || (*opt).doc != ptr::null() {
                if match_fn(&*opt) {
                    return Some((&*opt, argp_ptr));
                }
                opt = opt.add(1);
            }
        }
        if !a.children.is_null() {
            let mut child_ptr = a.children;
            while !(*child_ptr).argp.is_null() {
                if let Some(found) = find_option((*child_ptr).argp, match_fn) {
                    return Some(found);
                }
                child_ptr = child_ptr.add(1);
            }
        }
        None
    }

    // 1. Send ARGP_KEY_INIT
    let init_res = dispatch_key(argp, ARGP_KEY_INIT, ptr::null_mut(), &mut state);
    if init_res != 0 && init_res != ARGP_ERR_UNKNOWN {
        return init_res;
    }

    let mut non_option_seen = false;

    // 2. Parse arguments
    while state.next < argc {
        let arg_ptr = *argv.add(state.next as usize);
        if arg_ptr.is_null() {
            break;
        }
        let arg_str = match CStr::from_ptr(arg_ptr).to_str() {
            Ok(s) => s,
            Err(_) => {
                state.next += 1;
                continue;
            }
        };

        if arg_str == "--" && state.quoted == 0 {
            state.next += 1;
            state.quoted = state.next;
            continue;
        }

        if state.quoted != 0 || !arg_str.starts_with('-') || arg_str == "-" {
            if flags & ARGP_NO_ARGS != 0 {
                break;
            }
            let index = state.next;
            // The callback sees the next unconsumed argument and may adjust it.
            state.next += 1;
            let res = dispatch_key(argp, ARGP_KEY_ARG, arg_ptr, &mut state);
            if res == ARGP_ERR_UNKNOWN {
                state.next = index;
                break;
            }
            if res != 0 {
                return res;
            }
            if !(0..=argc).contains(&state.next) {
                return crate::EINVAL;
            }
            if state.next > index {
                non_option_seen = true;
                state.arg_num += (state.next - index) as c_uint;
            }
            continue;
        }

        if arg_str == "--version" || arg_str == "-V" {
            // Builtin version handler
            if let Some(hook) = argp_program_version_hook {
                hook(ptr::null_mut(), &mut state);
            } else if !argp_program_version.is_null() {
                let v = CStr::from_ptr(argp_program_version).to_string_lossy();
                println!("{v}");
            } else {
                let p = if !program_name.is_null() {
                    CStr::from_ptr(program_name).to_string_lossy()
                } else {
                    "program".into()
                };
                println!("{p} version unknown");
            }
            if (flags & ARGP_NO_EXIT) == 0 {
                crate::process::kinakaze_abi_exit(0);
            }
            state.next += 1;
            continue;
        }

        if arg_str == "--help" || arg_str == "-?" {
            kinakaze_abi_argp_help(argp, ptr::null_mut(), ARGP_HELP_STD_HELP, program_name);
            if (flags & ARGP_NO_EXIT) == 0 {
                crate::process::kinakaze_abi_exit(0);
            }
            state.next += 1;
            continue;
        }

        if arg_str == "--usage" {
            kinakaze_abi_argp_help(argp, ptr::null_mut(), ARGP_HELP_STD_USAGE, program_name);
            if (flags & ARGP_NO_EXIT) == 0 {
                crate::process::kinakaze_abi_exit(0);
            }
            state.next += 1;
            continue;
        }

        if arg_str.starts_with("--") && arg_str.len() > 2 {
            // Long option
            let opt_body = &arg_str[2..];
            let (opt_name, inline_val) = if let Some(eq_idx) = opt_body.find('=') {
                (&opt_body[..eq_idx], Some(&opt_body[eq_idx + 1..]))
            } else {
                (opt_body, None)
            };

            let matched = find_option(argp, &|opt| {
                if opt.name.is_null() {
                    return false;
                }
                if let Ok(name) = CStr::from_ptr(opt.name).to_str() {
                    name == opt_name
                } else {
                    false
                }
            });

            if let Some((opt, target_argp)) = matched {
                let val_ptr = if let Some(val) = inline_val {
                    let c_val = CString::new(val).unwrap();
                    c_val.into_raw()
                } else if !opt.arg.is_null() && (opt.flags & OPTION_ARG_OPTIONAL) == 0 {
                    if state.next + 1 < argc {
                        state.next += 1;
                        *argv.add(state.next as usize)
                    } else {
                        ptr::null_mut()
                    }
                } else {
                    ptr::null_mut()
                };

                state.next += 1;
                let target = &*target_argp;
                if let Some(parser) = target.parser {
                    let res = parser(opt.key, val_ptr, &mut state);
                    if res != 0 && res != ARGP_ERR_UNKNOWN {
                        return res;
                    }
                }
            } else {
                // Unknown option
                state.next += 1;
            }
        } else if arg_str.starts_with('-') && arg_str.len() > 1 && arg_str != "-" {
            // Short option
            let mut char_iter = arg_str[1..].chars();
            state.next += 1;
            while let Some(ch) = char_iter.next() {
                let key_code = ch as c_int;
                let matched = find_option(argp, &|opt| opt.key == key_code);
                if let Some((opt, target_argp)) = matched {
                    let val_ptr = if !opt.arg.is_null() {
                        let remaining: String = char_iter.by_ref().collect();
                        if !remaining.is_empty() {
                            CString::new(remaining).unwrap().into_raw()
                        } else if state.next < argc {
                            let next_val = *argv.add(state.next as usize);
                            state.next += 1;
                            next_val
                        } else {
                            ptr::null_mut()
                        }
                    } else {
                        ptr::null_mut()
                    };
                    let target = &*target_argp;
                    if let Some(parser) = target.parser {
                        let res = parser(opt.key, val_ptr, &mut state);
                        if res != 0 && res != ARGP_ERR_UNKNOWN {
                            return res;
                        }
                    }
                    if !opt.arg.is_null() {
                        break;
                    }
                }
            }
        }
    }

    if !arg_index.is_null() {
        *arg_index = state.next;
    } else if state.next < argc {
        let _ = dispatch_key(argp, ARGP_KEY_ERROR, ptr::null_mut(), &mut state);
        let _ = dispatch_key(argp, ARGP_KEY_FINI, ptr::null_mut(), &mut state);
        return crate::EINVAL;
    }

    // 3. Post-parsing lifecycle hooks
    if state.next == argc {
        if !non_option_seen {
            let _ = dispatch_key(argp, ARGP_KEY_NO_ARGS, ptr::null_mut(), &mut state);
        }
        let end_res = dispatch_key(argp, ARGP_KEY_END, ptr::null_mut(), &mut state);
        if end_res != 0 && end_res != ARGP_ERR_UNKNOWN {
            return end_res;
        }
    }

    let _ = dispatch_key(argp, ARGP_KEY_SUCCESS, ptr::null_mut(), &mut state);
    let _ = dispatch_key(argp, ARGP_KEY_FINI, ptr::null_mut(), &mut state);

    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_argp_help(
    argp: *const Argp,
    _stream: *mut c_void,
    flags: c_uint,
    name: *const c_char,
) {
    if argp.is_null() {
        return;
    }
    let prog = if !name.is_null() {
        CStr::from_ptr(name).to_string_lossy()
    } else {
        "program".into()
    };

    let a = &*argp;
    let doc = if !a.doc.is_null() {
        CStr::from_ptr(a.doc).to_string_lossy()
    } else {
        "".into()
    };
    let args_doc = if !a.args_doc.is_null() {
        CStr::from_ptr(a.args_doc).to_string_lossy()
    } else {
        "[OPTION...]".into()
    };

    if (flags & ARGP_HELP_USAGE) != 0 || (flags & ARGP_HELP_SHORT_USAGE) != 0 {
        println!("Usage: {prog} {args_doc}");
    }
    if (flags & ARGP_HELP_DOC) != 0 && !doc.is_empty() {
        println!("\n{doc}");
    }
    if (flags & ARGP_HELP_LONG) != 0 && !a.options.is_null() {
        println!("\nOptions:");
        let mut opt = a.options;
        while (*opt).name != ptr::null() || (*opt).key != 0 || (*opt).doc != ptr::null() {
            let o = &*opt;
            if (o.flags & OPTION_HIDDEN) == 0 {
                let name_str = if !o.name.is_null() {
                    format!("--{}", CStr::from_ptr(o.name).to_string_lossy())
                } else {
                    "".into()
                };
                let key_str = if o.key > 0 && o.key < 256 && (o.key as u8).is_ascii_graphic() {
                    format!("-{}, ", o.key as u8 as char)
                } else {
                    "    ".into()
                };
                let doc_str = if !o.doc.is_null() {
                    CStr::from_ptr(o.doc).to_string_lossy()
                } else {
                    "".into()
                };
                println!("  {key_str}{name_str:<25} {doc_str}");
            }
            opt = opt.add(1);
        }
    }
    if (flags & ARGP_HELP_BUG_ADDR) != 0 && !argp_program_bug_address.is_null() {
        let addr = CStr::from_ptr(argp_program_bug_address).to_string_lossy();
        println!("\nReport bugs to <{addr}>.");
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_argp_state_help(
    state: *const ArgpState,
    stream: *mut c_void,
    flags: c_uint,
) {
    if state.is_null() {
        return;
    }
    let s = &*state;
    kinakaze_abi_argp_help(s.root_argp, stream, flags, s.name);
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_argp_usage(state: *const ArgpState) {
    kinakaze_abi_argp_state_help(state, ptr::null_mut(), ARGP_HELP_USAGE | ARGP_HELP_EXIT_ERR);
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_argp_error(state: *const ArgpState, fmt: *const c_char) {
    if !state.is_null() && !(*state).name.is_null() {
        let name = CStr::from_ptr((*state).name).to_string_lossy();
        eprint!("{name}: ");
    }
    if !fmt.is_null() {
        let msg = CStr::from_ptr(fmt).to_string_lossy();
        eprintln!("{msg}");
    }
    if state.is_null() || ((*state).flags & ARGP_NO_EXIT) == 0 {
        crate::process::kinakaze_abi_exit(argp_err_exit_status);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_argp_failure(
    state: *const ArgpState,
    status: c_int,
    errnum: c_int,
    fmt: *const c_char,
) {
    if !state.is_null() && !(*state).name.is_null() {
        let name = CStr::from_ptr((*state).name).to_string_lossy();
        eprint!("{name}: ");
    }
    if !fmt.is_null() {
        let msg = CStr::from_ptr(fmt).to_string_lossy();
        eprint!("{msg}");
    }
    if errnum != 0 {
        let err_ptr = crate::string::strerror(errnum);
        if !err_ptr.is_null() {
            let err_msg = CStr::from_ptr(err_ptr).to_string_lossy();
            eprint!(": {err_msg}");
        }
    }
    eprintln!();
    if status != 0 {
        crate::process::kinakaze_abi_exit(status);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn argp_help(
    argp: *const Argp,
    stream: *mut c_void,
    flags: c_uint,
    name: *const c_char,
) {
    unsafe { kinakaze_abi_argp_help(argp, stream, flags, name) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn argp_state_help(
    state: *const ArgpState,
    stream: *mut c_void,
    flags: c_uint,
) {
    unsafe { kinakaze_abi_argp_state_help(state, stream, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn argp_usage(state: *const ArgpState) {
    unsafe { kinakaze_abi_argp_usage(state) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn argp_error(state: *const ArgpState, fmt: *const c_char) {
    unsafe { kinakaze_abi_argp_error(state, fmt) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn argp_failure(
    state: *const ArgpState,
    status: c_int,
    errnum: c_int,
    fmt: *const c_char,
) {
    unsafe { kinakaze_abi_argp_failure(state, status, errnum, fmt) }
}
