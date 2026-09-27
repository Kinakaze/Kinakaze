use super::*;
use std::ffi::CString;

fn run(pattern: &str, input: &str, flags: c_int, eflags: c_int) -> Option<Vec<(i32, i32)>> {
    let pattern = CString::new(pattern).unwrap();
    let input = CString::new(input).unwrap();
    let mut compiled: regex_t = unsafe { core::mem::zeroed() };
    assert_eq!(
        unsafe { regcomp(&mut compiled, pattern.as_ptr(), flags) },
        0
    );
    let mut matches = vec![
        regmatch_t {
            rm_so: -7,
            rm_eo: -7
        };
        compiled.re_nsub + 1
    ];
    let status = unsafe {
        regexec(
            &compiled,
            input.as_ptr(),
            matches.len(),
            matches.as_mut_ptr(),
            eflags,
        )
    };
    unsafe { regfree(&mut compiled) };
    assert!(status == 0 || status == REG_NOMATCH);
    (status == 0).then(|| matches.into_iter().map(|m| (m.rm_so, m.rm_eo)).collect())
}

#[test]
fn installer_manifest_checksum_capture() {
    let checksum = "1859583ce32920595c61ef868bee52e1b1594f7486db209935e01f1e5e804ae2";
    let text = format!(
        r#"{{"linux-x64": {{ "binary": "claude", "checksum": "{checksum}", "size": 241556664 }} }}"#
    );
    let pattern = r#""linux-x64"[[:space:]]*:[[:space:]]*[{][^{}]*"checksum"[[:space:]]*:[[:space:]]*"([a-f0-9]{64})""#;
    let matches = run(pattern, &text, REG_EXTENDED, 0).unwrap();
    let (start, end) = matches[1];
    assert_eq!(&text[start as usize..end as usize], checksum);
}

#[test]
fn repetition_backtracks_into_following_expression() {
    assert_eq!(
        run("^(a*)(a+)$", "aaa", REG_EXTENDED, 0).unwrap(),
        [(0, 3), (0, 2), (2, 3)]
    );
    assert!(run("^ab?bc$", "abc", REG_EXTENDED, 0).is_some());
    assert!(run("^a{2,4}ab$", "aaab", REG_EXTENDED, 0).is_some());
    assert!(run("^a{2,4}ab$", "aab", REG_EXTENDED, 0).is_none());
}

#[test]
fn groups_and_alternatives_backtrack() {
    assert_eq!(
        run("^(ab|a)b$", "ab", REG_EXTENDED, 0).unwrap(),
        [(0, 2), (0, 1)]
    );
    assert_eq!(
        run("^(a|aa)", "aa", REG_EXTENDED, 0).unwrap(),
        [(0, 2), (0, 2)]
    );
    assert_eq!(
        run("(ab|a)(bc|c)", "abc", REG_EXTENDED, 0).unwrap(),
        [(0, 3), (0, 2), (2, 3)]
    );
}

#[test]
fn repeated_empty_groups_terminate_and_satisfy_minimum() {
    assert!(run("^(a?){4}$", "aa", REG_EXTENDED, 0).is_some());
    assert!(run("^(a?){4}$", "", REG_EXTENDED, 0).is_some());
    assert!(run("^(a?)*$", "aaa", REG_EXTENDED, 0).is_some());
    assert!(run("^(a?){2}$", "aaa", REG_EXTENDED, 0).is_none());
    assert!(run("^(){0}$", "", REG_EXTENDED, 0).is_some());
}

#[test]
fn backreferences_keep_alternative_capture_states() {
    assert_eq!(
        run(r"^\(a*\)b\1$", "aabaa", 0, 0).unwrap(),
        [(0, 5), (0, 2)]
    );
    assert_eq!(
        run(r"^(a|aa)\1$", "aaaa", REG_EXTENDED, 0).unwrap(),
        [(0, 4), (0, 2)]
    );
    assert!(run(r"^(a)\1$", "ab", REG_EXTENDED, 0).is_none());
    assert!(run(r"^(a)\1$", "aA", REG_EXTENDED | REG_ICASE, 0).is_some());
}

#[test]
fn glibc_flag_values_newlines_and_no_sub() {
    assert!(run("^b$", "a\nb\nc", 1, 0).is_none());
    assert!(run("^b$", "a\nb\nc", 1 | 4, REG_NOTBOL | REG_NOTEOL).is_some());
    assert!(run("^b$", "b", 1, REG_NOTBOL).is_none());
    assert!(run("^b$", "b", 1, REG_NOTEOL).is_none());
    assert!(run("^.$", "\n", 1 | 4, 0).is_none());
    assert!(run("^[^a]$", "\n", 1 | 4, 0).is_none());
    assert_eq!(run("(a)", "a", 1 | 8, 0).unwrap(), [(-7, -7), (-7, -7)]);
}

#[test]
fn inverted_classes_respect_case_and_posix_classes() {
    assert!(run("^[^a]$", "A", REG_EXTENDED | REG_ICASE, 0).is_none());
    assert!(
        run(
            "^[[:upper:]][[:lower:]][[:cntrl:]][[:graph:]][[:print:]]$",
            "Aa\t! ",
            REG_EXTENDED,
            0
        )
        .is_some()
    );
}

#[test]
fn malformed_classes_and_backreferences_report_errors() {
    for (pattern, expected) in [
        ("[abc", REG_EBRACK),
        ("[z-a]", REG_ERANGE),
        ("[[:unknown:]]", REG_ECTYPE),
        (r"\1", REG_ESUBREG),
    ] {
        let mut compiled: regex_t = unsafe { core::mem::zeroed() };
        let pattern = CString::new(pattern).unwrap();
        assert_eq!(
            unsafe { regcomp(&mut compiled, pattern.as_ptr(), REG_EXTENDED) },
            expected
        );
    }
}

#[test]
fn long_repetition_uses_no_input_length_recursion() {
    let input = "a".repeat(8192) + "b";
    assert_eq!(
        run("^a*ab$", &input, REG_EXTENDED, 0).unwrap()[0],
        (0, 8193)
    );
}

#[test]
fn gnu_search_direction_and_match_length() {
    let mut compiled: regex_t = unsafe { core::mem::zeroed() };
    unsafe {
        assert_eq!(regcomp(&mut compiled, c"ab+".as_ptr(), REG_EXTENDED), 0);
        assert_eq!(
            kinakaze_abi_re_search(
                &mut compiled,
                c"abb--ab".as_ptr(),
                7,
                6,
                -6,
                ptr::null_mut()
            ),
            5
        );
        assert_eq!(
            kinakaze_abi_re_match(&mut compiled, c"abb--ab".as_ptr(), 7, 0, ptr::null_mut()),
            3
        );
        assert_eq!(
            kinakaze_abi_re_match(&mut compiled, c"abb--ab".as_ptr(), 7, 5, ptr::null_mut()),
            2
        );
        assert_eq!(
            kinakaze_abi_re_search(&mut compiled, c"abb--ab".as_ptr(), 7, 8, 1, ptr::null_mut()),
            -1
        );
        regfree(&mut compiled);
    }
}
