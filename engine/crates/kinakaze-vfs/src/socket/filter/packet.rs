use super::classic::Packet;

/// A kernel-created netlink message or Unix datagram, with no IP/link header.
pub(crate) struct LocalPacket<'a>(pub(crate) &'a [u8]);
impl Packet for LocalPacket<'_> {
    fn len(&self) -> u32 {
        self.0.len().min(u32::MAX as usize) as u32
    }
    fn load(&self, offset: i32, size: usize) -> Option<u32> {
        let offset = usize::try_from(offset).ok()?;
        let data = self.0.get(offset..offset.checked_add(size)?)?;
        match size {
            1 => Some(data[0] as u32),
            2 => Some(u16::from_be_bytes(data.try_into().ok()?) as u32),
            4 => Some(u32::from_be_bytes(data.try_into().ok()?)),
            _ => None,
        }
    }
    fn ancillary(&self, offset: u32, a: u32, x: u32) -> Option<u32> {
        match offset {
            // These local skbs carry no device, protocol, mark, hash or VLAN.
            0 | 4 | 20 | 24 | 32 | 44 | 48 | 52 | 60 => Some(0),
            8 | 28 => None,
            12 => Some(find_attribute(self.0, a as usize, self.0.len(), x)),
            16 => {
                let search = || {
                    let start = a as usize;
                    let length = self.0.get(start..start.checked_add(2)?)?;
                    let length = u16::from_ne_bytes(length.try_into().ok()?) as usize;
                    if length < 4 || start.checked_add(length)? > self.0.len() {
                        return None;
                    }
                    Some(find_attribute(self.0, start + 4, start + length, x))
                };
                Some(search().unwrap_or(0))
            }
            36 => {
                Some(unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessorNumber() })
            }
            56 => {
                #[link(name = "bcrypt")]
                unsafe extern "system" {
                    fn BCryptGenRandom(
                        provider: *mut core::ffi::c_void,
                        output: *mut u8,
                        length: u32,
                        flags: u32,
                    ) -> i32;
                }
                let mut bytes = [0; 4];
                let ok = unsafe { BCryptGenRandom(std::ptr::null_mut(), bytes.as_mut_ptr(), 4, 2) };
                (ok >= 0).then(|| u32::from_ne_bytes(bytes))
            }
            _ => None,
        }
    }
}

fn find_attribute(data: &[u8], mut start: usize, end: usize, kind: u32) -> u32 {
    while start.checked_add(4).is_some_and(|next| next <= end) {
        let length = u16::from_ne_bytes(data[start..start + 2].try_into().unwrap()) as usize;
        if length < 4 || start.checked_add(length).is_none_or(|next| next > end) {
            break;
        }
        let ty = u16::from_ne_bytes(data[start + 2..start + 4].try_into().unwrap()) & 0x3fff;
        if ty as u32 == kind {
            return start as u32;
        }
        start += (length + 3) & !3;
    }
    0
}
