// Generated object layout query; PE export tables do not carry data sizes.
// Buffers are borrowed only for this call. No memory ownership crosses the ABI.
#[rustfmt::skip]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kinakaze_module_object_v1(name: *const u8, length: usize) -> u64 {
    if name.is_null() || length > 128 { return 0; }
    match unsafe { core::slice::from_raw_parts(name, length) } {
        b"__daylight" => (4u64 << 32) | 4,
        b"__environ" => (8u64 << 32) | 8,
        b"__libc_single_threaded" => (1u64 << 32) | 1,
        b"__libc_stack_end" => (8u64 << 32) | 8,
        b"__progname" => (8u64 << 32) | 8,
        b"__progname_full" => (8u64 << 32) | 8,
        b"__timezone" => (8u64 << 32) | 8,
        b"__tzname" => (8u64 << 32) | 16,
        b"_environ" => (8u64 << 32) | 8,
        b"_libc_intl_domainname" => (1u64 << 32) | 5,
        b"argp_err_exit_status" => (4u64 << 32) | 4,
        b"argp_program_bug_address" => (8u64 << 32) | 8,
        b"argp_program_version" => (8u64 << 32) | 8,
        b"argp_program_version_hook" => (8u64 << 32) | 8,
        b"daylight" => (4u64 << 32) | 4,
        b"environ" => (8u64 << 32) | 8,
        b"error_message_count" => (4u64 << 32) | 4,
        b"error_one_per_line" => (4u64 << 32) | 4,
        b"error_print_progname" => (8u64 << 32) | 8,
        b"in6addr_any" => (4u64 << 32) | 16,
        b"in6addr_loopback" => (4u64 << 32) | 16,
        b"obstack_alloc_failed_handler" => (8u64 << 32) | 8,
        b"obstack_exit_failure" => (4u64 << 32) | 4,
        b"optarg" => (8u64 << 32) | 8,
        b"opterr" => (4u64 << 32) | 4,
        b"optind" => (4u64 << 32) | 4,
        b"optopt" => (4u64 << 32) | 4,
        b"program_invocation_name" => (8u64 << 32) | 8,
        b"program_invocation_short_name" => (8u64 << 32) | 8,
        b"re_syntax_options" => (8u64 << 32) | 8,
        b"stderr" => (8u64 << 32) | 8,
        b"stdin" => (8u64 << 32) | 8,
        b"stdout" => (8u64 << 32) | 8,
        b"timezone" => (8u64 << 32) | 8,
        b"tzname" => (8u64 << 32) | 16,
        _ => 0,
    }
}
