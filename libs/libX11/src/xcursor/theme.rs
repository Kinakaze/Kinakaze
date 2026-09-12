//! Decode the Xcursor file format from the guest filesystem, without host paths.
use super::*;
use std::ffi::CString;
fn read(path: &[u8]) -> Option<Vec<u8>> {
    let path = CString::new(path).ok()?;
    let fd = unsafe { libc::fsextra::kinakaze_abi_open64(path.as_ptr(), 0, 0) };
    if fd < 0 {
        return None;
    }
    let size = libc::fsextra::kinakaze_abi_lseek64(fd, 0, 2);
    let mut bytes = Vec::new();
    let result = (|| {
        if size < 0 || libc::fsextra::kinakaze_abi_lseek64(fd, 0, 0) < 0 {
            return None;
        }
        bytes.try_reserve_exact(size as usize).ok()?;
        bytes.resize(size as usize, 0);
        let mut at = 0;
        while at < bytes.len() {
            let n = unsafe {
                libc::kinakaze_abi_read(fd, bytes[at..].as_mut_ptr().cast(), bytes.len() - at)
            };
            if n <= 0 {
                return None;
            }
            at += n as usize;
        }
        Some(bytes)
    })();
    libc::kinakaze_abi_close(fd);
    result
}
fn word(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}
unsafe fn decode(data: &[u8], requested: u32) -> Option<*mut XcursorImages> {
    const IMAGE: u32 = 0xfffd0002;
    if word(data, 0)? != 0x72756358 || word(data, 8)? != 0x10000 {
        return None;
    }
    let header = word(data, 4)? as usize;
    if header < 16 {
        return None;
    }
    let count = word(data, 12)? as usize;
    data.get(header..header.checked_add(count.checked_mul(12)?)?)?;
    let mut best = None;
    for i in 0..count {
        let at = header + i * 12;
        if word(data, at)? == IMAGE {
            let size = word(data, at + 4)?;
            if best.is_none_or(|b: u32| size.abs_diff(requested) < b.abs_diff(requested)) {
                best = Some(size);
            }
        }
    }
    let best = best?;
    let positions: Vec<usize> = (0..count)
        .filter_map(|i| {
            let at = header + i * 12;
            (word(data, at) == Some(IMAGE) && word(data, at + 4) == Some(best))
                .then(|| word(data, at + 8).unwrap() as usize)
        })
        .collect();
    let images = unsafe { XcursorImagesCreate(c_int::try_from(positions.len()).ok()?) };
    if images.is_null() {
        return None;
    }
    for at in positions {
        let frame = (|| {
            let header = word(data, at)? as usize;
            if header < 36
                || word(data, at + 4)? != IMAGE
                || word(data, at + 8)? != best
                || word(data, at + 12)? != 1
            {
                return None;
            }
            let w = word(data, at + 16)?;
            let h = word(data, at + 20)?;
            let x = word(data, at + 24)?;
            let y = word(data, at + 28)?;
            let delay = word(data, at + 32)?;
            if w == 0 || h == 0 || w > 32767 || h > 32767 || x >= w || y >= h {
                return None;
            }
            let pixels = at.checked_add(header)?;
            let n = (w as usize).checked_mul(h as usize)?;
            let payload = data.get(pixels..pixels.checked_add(n.checked_mul(4)?)?)?;
            let image = unsafe { XcursorImageCreate(w as i32, h as i32) };
            if image.is_null() {
                return None;
            }
            unsafe {
                (*image).size = best;
                (*image).xhot = x;
                (*image).yhot = y;
                (*image).delay = delay;
                ptr::copy_nonoverlapping(payload.as_ptr(), (*image).pixels.cast(), payload.len());
            }
            Some(image)
        })();
        let Some(frame) = frame else {
            unsafe {
                XcursorImagesDestroy(images);
            }
            return None;
        };
        unsafe {
            *(*images).images.add((*images).nimage as usize) = frame;
            (*images).nimage += 1;
        }
    }
    Some(images)
}
pub(super) unsafe fn load(name: &[u8], theme: &[u8], size: i32) -> *mut XcursorImages {
    if name.is_empty()
        || theme.is_empty()
        || name.contains(&b'/')
        || theme.contains(&b'/')
        || name == b".."
        || theme == b".."
    {
        return ptr::null_mut();
    }
    let mut path = b"/usr/share/icons/".to_vec();
    path.extend_from_slice(theme);
    path.extend_from_slice(b"/cursors/");
    path.extend_from_slice(name);
    let Some(data) = read(&path) else {
        return ptr::null_mut();
    };
    unsafe { decode(&data, size as u32) }.unwrap_or(ptr::null_mut())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_parser_rejects_truncated_and_invalid_headers() {
        for data in [b"".as_slice(), b"Xcur", &[0; 32]] {
            assert!(unsafe { decode(data, 24) }.is_none());
        }
    }
}
