//! Resolver facade over the worker's canonical libc DNS and errno state.
//! DNS wire structures follow glibc's x86-64 <arpa/nameser.h> ABI.
//! ABI reference: https://sourceware.org/git/?p=glibc.git;a=blob;f=resolv/arpa/nameser.h

#[cfg(all(windows, target_arch = "x86_64"))]
pub mod windows {
    use core::ffi::{c_char, c_int, c_void};
    use core::ptr;
    pub use libc::netdb::ResState;

    fn fail(error: i32) -> i32 {
        unsafe {
            *libc::kinakaze_abi___errno_location() = error;
        }
        -1
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv___res_init")]
    pub extern "sysv64" fn __res_init() -> c_int {
        libc::netdb::kinakaze_abi___res_init()
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv___res_state")]
    pub extern "sysv64" fn __res_state() -> *mut ResState {
        libc::netdb::kinakaze_abi___res_state()
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv_res_query")]
    pub unsafe extern "sysv64" fn res_query(
        dname: *const c_char,
        class: c_int,
        record_type: c_int,
        answer: *mut u8,
        answer_length: c_int,
    ) -> c_int {
        unsafe {
            libc::netdb::kinakaze_abi_res_query(dname, class, record_type, answer, answer_length)
        }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv___res_query")]
    pub unsafe extern "sysv64" fn __res_query(
        dname: *const c_char,
        class: c_int,
        record_type: c_int,
        answer: *mut u8,
        answer_length: c_int,
    ) -> c_int {
        unsafe {
            libc::netdb::kinakaze_abi___res_query(dname, class, record_type, answer, answer_length)
        }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv_res_search")]
    pub unsafe extern "sysv64" fn res_search(
        dname: *const c_char,
        class: c_int,
        record_type: c_int,
        answer: *mut u8,
        answer_length: c_int,
    ) -> c_int {
        unsafe {
            libc::netdb::kinakaze_abi_res_search(dname, class, record_type, answer, answer_length)
        }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv___res_search")]
    pub unsafe extern "sysv64" fn __res_search(
        dname: *const c_char,
        class: c_int,
        record_type: c_int,
        answer: *mut u8,
        answer_length: c_int,
    ) -> c_int {
        unsafe {
            libc::netdb::kinakaze_abi___res_search(dname, class, record_type, answer, answer_length)
        }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv_res_mkquery")]
    pub unsafe extern "sysv64" fn res_mkquery(
        _op: c_int,
        dname: *const c_char,
        class: c_int,
        record_type: c_int,
        _data: *const c_void,
        _datalen: c_int,
        _newrr: *const c_void,
        buf: *mut u8,
        buflen: c_int,
    ) -> c_int {
        unsafe {
            libc::netdb::kinakaze_abi_res_mkquery(
                _op,
                dname,
                class,
                record_type,
                _data,
                _datalen,
                _newrr,
                buf,
                buflen,
            )
        }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv___res_mkquery")]
    pub unsafe extern "sysv64" fn __res_mkquery(
        op: c_int,
        dname: *const c_char,
        class: c_int,
        record_type: c_int,
        data: *const c_void,
        datalen: c_int,
        newrr: *const c_void,
        buf: *mut u8,
        buflen: c_int,
    ) -> c_int {
        unsafe {
            libc::netdb::kinakaze_abi___res_mkquery(
                op,
                dname,
                class,
                record_type,
                data,
                datalen,
                newrr,
                buf,
                buflen,
            )
        }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv_res_ninit")]
    pub unsafe extern "sysv64" fn res_ninit(state: *mut ResState) -> c_int {
        unsafe { libc::netdb::kinakaze_abi_res_ninit(state) }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv___res_ninit")]
    pub unsafe extern "sysv64" fn __res_ninit(state: *mut ResState) -> c_int {
        unsafe { libc::netdb::kinakaze_abi___res_ninit(state) }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv_res_nclose")]
    pub unsafe extern "sysv64" fn res_nclose(_state: *mut ResState) {
        unsafe { libc::netdb::kinakaze_abi_res_nclose(_state) }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv___res_nclose")]
    pub unsafe extern "sysv64" fn __res_nclose(state: *mut ResState) {
        unsafe { libc::netdb::kinakaze_abi___res_nclose(state) }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv_res_nmkquery")]
    pub unsafe extern "sysv64" fn res_nmkquery(
        _state: *mut ResState,
        op: c_int,
        dname: *const c_char,
        class: c_int,
        record_type: c_int,
        data: *const c_void,
        datalen: c_int,
        newrr: *const c_void,
        buf: *mut u8,
        buflen: c_int,
    ) -> c_int {
        unsafe {
            libc::netdb::kinakaze_abi_res_nmkquery(
                _state,
                op,
                dname,
                class,
                record_type,
                data,
                datalen,
                newrr,
                buf,
                buflen,
            )
        }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv___res_nmkquery")]
    pub unsafe extern "sysv64" fn __res_nmkquery(
        state: *mut ResState,
        op: c_int,
        dname: *const c_char,
        class: c_int,
        record_type: c_int,
        data: *const c_void,
        datalen: c_int,
        newrr: *const c_void,
        buf: *mut u8,
        buflen: c_int,
    ) -> c_int {
        unsafe {
            libc::netdb::kinakaze_abi___res_nmkquery(
                state,
                op,
                dname,
                class,
                record_type,
                data,
                datalen,
                newrr,
                buf,
                buflen,
            )
        }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv_res_nquery")]
    pub unsafe extern "sysv64" fn res_nquery(
        _state: *mut ResState,
        dname: *const c_char,
        class: c_int,
        record_type: c_int,
        answer: *mut u8,
        answer_length: c_int,
    ) -> c_int {
        unsafe {
            libc::netdb::kinakaze_abi_res_nquery(
                _state,
                dname,
                class,
                record_type,
                answer,
                answer_length,
            )
        }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv___res_nquery")]
    pub unsafe extern "sysv64" fn __res_nquery(
        state: *mut ResState,
        dname: *const c_char,
        class: c_int,
        record_type: c_int,
        answer: *mut u8,
        answer_length: c_int,
    ) -> c_int {
        unsafe {
            libc::netdb::kinakaze_abi___res_nquery(
                state,
                dname,
                class,
                record_type,
                answer,
                answer_length,
            )
        }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv_res_nsearch")]
    pub unsafe extern "sysv64" fn res_nsearch(
        _state: *mut ResState,
        dname: *const c_char,
        class: c_int,
        record_type: c_int,
        answer: *mut u8,
        answer_length: c_int,
    ) -> c_int {
        unsafe {
            libc::netdb::kinakaze_abi_res_nsearch(
                _state,
                dname,
                class,
                record_type,
                answer,
                answer_length,
            )
        }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv___res_nsearch")]
    pub unsafe extern "sysv64" fn __res_nsearch(
        state: *mut ResState,
        dname: *const c_char,
        class: c_int,
        record_type: c_int,
        answer: *mut u8,
        answer_length: c_int,
    ) -> c_int {
        unsafe {
            libc::netdb::kinakaze_abi___res_nsearch(
                state,
                dname,
                class,
                record_type,
                answer,
                answer_length,
            )
        }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv_res_nsend")]
    pub unsafe extern "sysv64" fn res_nsend(
        _state: *mut ResState,
        msg: *const u8,
        msglen: c_int,
        answer: *mut u8,
        anslen: c_int,
    ) -> c_int {
        unsafe { libc::netdb::kinakaze_abi_res_nsend(_state, msg, msglen, answer, anslen) }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv___res_nsend")]
    pub unsafe extern "sysv64" fn __res_nsend(
        state: *mut ResState,
        msg: *const u8,
        msglen: c_int,
        answer: *mut u8,
        anslen: c_int,
    ) -> c_int {
        unsafe { libc::netdb::kinakaze_abi___res_nsend(state, msg, msglen, answer, anslen) }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv_res_send")]
    pub unsafe extern "sysv64" fn res_send(
        msg: *const u8,
        msglen: c_int,
        answer: *mut u8,
        anslen: c_int,
    ) -> c_int {
        unsafe { libc::netdb::kinakaze_abi_res_send(msg, msglen, answer, anslen) }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv___res_send")]
    pub unsafe extern "sysv64" fn __res_send(
        msg: *const u8,
        msglen: c_int,
        answer: *mut u8,
        anslen: c_int,
    ) -> c_int {
        unsafe { libc::netdb::kinakaze_abi___res_send(msg, msglen, answer, anslen) }
    }

    #[repr(C)]
    pub struct NsMsg {
        pub message: *const u8,
        pub end: *const u8,
        pub id: u16,
        pub flags: u16,
        pub counts: [u16; 4],
        pub sections: [*const u8; 4],
        pub section: c_int,
        pub record_index: c_int,
        pub cursor: *const u8,
    }

    #[repr(C)]
    pub struct NsRr {
        pub name: [c_char; 1025],
        pub record_type: u16,
        pub class: u16,
        pub ttl: u32,
        pub data_length: u16,
        pub data: *const u8,
    }

    fn be16(bytes: &[u8], offset: usize) -> Result<u16, i32> {
        let p = bytes
            .get(offset..offset.checked_add(2).ok_or(90)?)
            .ok_or(90)?;
        Ok(u16::from_be_bytes([p[0], p[1]]))
    }

    // Returns presentation bytes and consumption at the original cursor.
    // Compression targets are bounded and cycles are rejected before dereference.
    fn expand_name(bytes: &[u8], start: usize) -> Result<(Vec<u8>, usize), i32> {
        libc::netdb::expand_dns_name(bytes, start)
    }

    fn record_end(bytes: &[u8], start: usize, question: bool) -> Result<usize, i32> {
        let (_, used) = expand_name(bytes, start)?;
        let mut end = start.checked_add(used).ok_or(90)?;
        if question {
            end = end.checked_add(4).ok_or(90)?;
        } else {
            let length = be16(bytes, end + 8)? as usize;
            end = end.checked_add(10 + length).ok_or(90)?;
        }
        if end > bytes.len() {
            return Err(90);
        }
        Ok(end)
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv_ns_initparse")]
    pub unsafe extern "sysv64" fn ns_initparse(
        message: *const u8,
        length: c_int,
        handle: *mut NsMsg,
    ) -> c_int {
        if message.is_null() || handle.is_null() {
            return fail(22);
        }
        if length < 12 || length > 65535 {
            return fail(90);
        }
        let bytes = unsafe { core::slice::from_raw_parts(message, length as usize) };
        let mut value = NsMsg {
            message,
            end: unsafe { message.add(bytes.len()) },
            id: u16::from_be_bytes([bytes[0], bytes[1]]),
            flags: u16::from_be_bytes([bytes[2], bytes[3]]),
            counts: [0; 4],
            sections: [ptr::null(); 4],
            section: 4,
            record_index: -1,
            cursor: ptr::null(),
        };
        let mut offset = 12;
        for section in 0..4 {
            value.counts[section] = be16(bytes, 4 + section * 2).unwrap();
            if value.counts[section] != 0 {
                value.sections[section] = unsafe { message.add(offset) };
            }
            for _ in 0..value.counts[section] {
                offset = match record_end(bytes, offset, section == 0) {
                    Ok(end) => end,
                    Err(error) => return fail(error),
                };
            }
        }
        if offset != bytes.len() {
            return fail(90);
        }
        unsafe {
            handle.write(value);
        }
        0
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv_ns_parserr")]
    pub unsafe extern "sysv64" fn ns_parserr(
        handle: *mut NsMsg,
        section: c_int,
        index: c_int,
        record: *mut NsRr,
    ) -> c_int {
        if handle.is_null() || record.is_null() || !(0..4).contains(&section) || index < -1 {
            return fail(22);
        }
        let handle = unsafe { &mut *handle };
        let base = handle.message as usize;
        let end = handle.end as usize;
        if base == 0 || end < base || end - base > 65535 {
            return fail(90);
        }
        let bytes = unsafe { core::slice::from_raw_parts(handle.message, end - base) };
        let wanted = if index == -1 {
            if handle.section == section {
                handle.record_index
            } else {
                0
            }
        } else {
            index
        };
        if wanted < 0 || wanted >= handle.counts[section as usize] as i32 {
            return fail(19);
        }
        let section_start = handle.sections[section as usize] as usize;
        if section_start < base || section_start >= end {
            return fail(90);
        }
        let mut offset = section_start - base;
        for _ in 0..wanted {
            offset = match record_end(bytes, offset, section == 0) {
                Ok(end) => end,
                Err(error) => return fail(error),
            };
        }
        let (name, used) = match expand_name(bytes, offset) {
            Ok(v) => v,
            Err(error) => return fail(error),
        };
        let next = match record_end(bytes, offset, section == 0) {
            Ok(v) => v,
            Err(error) => return fail(error),
        };
        offset += used;
        let mut value = NsRr {
            name: [0; 1025],
            record_type: 0,
            class: 0,
            ttl: 0,
            data_length: 0,
            data: ptr::null(),
        };
        if name.len() >= value.name.len() {
            return fail(90);
        }
        for (to, from) in value.name.iter_mut().zip(name) {
            *to = from as c_char;
        }
        value.record_type = be16(bytes, offset).unwrap();
        value.class = be16(bytes, offset + 2).unwrap();
        if section != 0 {
            value.ttl = u32::from_be_bytes(bytes[offset + 4..offset + 8].try_into().unwrap());
            value.data_length = be16(bytes, offset + 8).unwrap();
            value.data = unsafe { handle.message.add(offset + 10) };
        }
        unsafe {
            record.write(value);
        }
        handle.section = section;
        handle.record_index = wanted + 1;
        handle.cursor = unsafe { handle.message.add(next) };
        0
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv_ns_name_uncompress")]
    pub unsafe extern "sysv64" fn ns_name_uncompress(
        message: *const u8,
        end: *const u8,
        source: *const u8,
        output: *mut c_char,
        capacity: usize,
    ) -> c_int {
        unsafe {
            libc::netdb::kinakaze_abi_ns_name_uncompress(message, end, source, output, capacity)
        }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv_dn_expand")]
    pub unsafe extern "sysv64" fn dn_expand(
        message: *const u8,
        end: *const u8,
        source: *const u8,
        output: *mut c_char,
        capacity: c_int,
    ) -> c_int {
        if capacity <= 0 {
            return fail(90);
        }
        let result = unsafe { ns_name_uncompress(message, end, source, output, capacity as usize) };
        if result > 0 && unsafe { *output == b'.' as c_char && *output.add(1) == 0 } {
            unsafe {
                *output = 0;
            }
        }
        result
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv_ns_name_skip")]
    pub unsafe extern "sysv64" fn ns_name_skip(cursor: *mut *const u8, end: *const u8) -> c_int {
        unsafe { libc::netdb::kinakaze_abi_ns_name_skip(cursor, end) }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv_dn_skipname")]
    pub unsafe extern "sysv64" fn dn_skipname(start: *const u8, end: *const u8) -> c_int {
        unsafe { libc::netdb::kinakaze_abi_dn_skipname(start, end) }
    }

    #[unsafe(export_name = "kinakaze_engine_libresolv_ns_get16")]
    pub unsafe extern "sysv64" fn ns_get16(source: *const u8) -> u32 {
        unsafe { u32::from(u16::from_be_bytes([*source, *source.add(1)])) }
    }
    #[unsafe(export_name = "kinakaze_engine_libresolv_ns_get32")]
    pub unsafe extern "sysv64" fn ns_get32(source: *const u8) -> u32 {
        unsafe { u32::from_be_bytes([*source, *source.add(1), *source.add(2), *source.add(3)]) }
    }
    #[unsafe(export_name = "kinakaze_engine_libresolv_ns_put16")]
    pub unsafe extern "sysv64" fn ns_put16(value: u32, destination: *mut u8) {
        unsafe {
            ptr::copy_nonoverlapping((value as u16).to_be_bytes().as_ptr(), destination, 2);
        }
    }
    #[unsafe(export_name = "kinakaze_engine_libresolv_ns_put32")]
    pub unsafe extern "sysv64" fn ns_put32(value: u32, destination: *mut u8) {
        unsafe {
            ptr::copy_nonoverlapping(value.to_be_bytes().as_ptr(), destination, 4);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn wire_layout_and_compressed_answer_match_linux_abi() {
            assert_eq!(core::mem::size_of::<NsMsg>(), 80);
            assert_eq!(core::mem::size_of::<NsRr>(), 1048);
            assert_eq!(core::mem::offset_of!(NsRr, data), 1040);
            let wire = b"\x12\x34\x81\x80\0\x01\0\x01\0\0\0\0\x03www\x07example\x03com\0\0\x01\0\x01\xc0\x0c\0\x01\0\x01\0\0\0\x3c\0\x04\x7f\0\0\x01";
            let mut handle = core::mem::MaybeUninit::<NsMsg>::uninit();
            assert_eq!(
                unsafe { ns_initparse(wire.as_ptr(), wire.len() as i32, handle.as_mut_ptr()) },
                0
            );
            let mut handle = unsafe { handle.assume_init() };
            assert_eq!(handle.id, 0x1234);
            let mut record = core::mem::MaybeUninit::<NsRr>::uninit();
            assert_eq!(
                unsafe { ns_parserr(&mut handle, 1, -1, record.as_mut_ptr()) },
                0
            );
            let record = unsafe { record.assume_init() };
            assert_eq!(
                unsafe { core::ffi::CStr::from_ptr(record.name.as_ptr()) }.to_bytes(),
                b"www.example.com"
            );
            assert_eq!(
                (
                    record.record_type,
                    record.class,
                    record.ttl,
                    record.data_length
                ),
                (1, 1, 60, 4)
            );
            assert_eq!(
                unsafe { core::slice::from_raw_parts(record.data, 4) },
                &[127, 0, 0, 1]
            );
        }
        #[test]
        fn malformed_names_and_records_cannot_escape_message() {
            assert_eq!(expand_name(b"\xc0\0", 0), Err(90));
            assert_eq!(expand_name(b"\xc0\x80", 0), Err(90));
            assert_eq!(expand_name(b"\x03ab", 0), Err(90));
            assert_eq!(expand_name(b"\0", 0).unwrap(), (b".".to_vec(), 1));
            assert_eq!(
                expand_name(b"\x03a.b\0", 0).unwrap(),
                (b"a\\.b".to_vec(), 5)
            );
            let truncated = [0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0];
            let mut handle = core::mem::MaybeUninit::<NsMsg>::uninit();
            assert_eq!(
                unsafe {
                    ns_initparse(
                        truncated.as_ptr(),
                        truncated.len() as i32,
                        handle.as_mut_ptr(),
                    )
                },
                -1
            );
            assert_eq!(unsafe { *libc::kinakaze_abi___errno_location() }, 90);
        }
        #[test]
        fn mkquery_uses_real_libc_dns_packet_builder() {
            let mut bytes = [0u8; 512];
            let length = unsafe {
                res_mkquery(
                    0,
                    c"example.com".as_ptr(),
                    1,
                    1,
                    ptr::null(),
                    0,
                    ptr::null(),
                    bytes.as_mut_ptr(),
                    bytes.len() as i32,
                )
            };
            assert!(length > 12);
            let mut handle = core::mem::MaybeUninit::<NsMsg>::uninit();
            assert_eq!(
                unsafe { ns_initparse(bytes.as_ptr(), length, handle.as_mut_ptr()) },
                0
            );
            assert_eq!(unsafe { handle.assume_init().counts }, [1, 0, 0, 0]);
        }
    }
}

mod object_layout;
