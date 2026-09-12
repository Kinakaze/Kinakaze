//! Service enumeration uses the common buffered database lifecycle.
super::super::records::cursor::database_cursor!(*b"CYSERV01");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_malformed_handoffs_before_adopting_any_descriptor() {
        let mut frame = [0u8; HEADER + 1];
        frame[..8].copy_from_slice(&MAGIC);
        frame[8..12].copy_from_slice(&(-1i32).to_le_bytes());
        let reject = |bytes: &[u8]| {
            assert_eq!(
                unsafe { child(bytes.as_ptr(), bytes.len()) },
                kinakaze_vfs::EINVAL
            );
        };
        reject(&frame[..HEADER - 1]);
        reject(&frame); // Unread count does not match the frame.
        frame[16] = 2;
        reject(&frame[..HEADER]); // Invalid EOF flag.
        frame[16] = 1;
        reject(&frame[..HEADER]); // A closed stream cannot carry EOF state.
        frame[16] = 0;
        frame[8..12].copy_from_slice(&(-2i32).to_le_bytes());
        reject(&frame[..HEADER]);
        frame[8..12].copy_from_slice(&(-1i32).to_le_bytes());
        frame[0] ^= 1;
        reject(&frame[..HEADER]);
        assert_eq!(
            unsafe { child(core::ptr::null(), HEADER) },
            kinakaze_vfs::EINVAL
        );
    }
}
