//! PCM readiness uses native completion notifications and Linux eventfd.
use super::*;
use libc::fdio::PollFd;

pub(super) unsafe extern "system" fn completed(
    _handle: HWAVEOUT,
    message: u32,
    instance: usize,
    _header: usize,
    _reserved: usize,
) {
    if message == windows_sys::Win32::Media::MM_WOM_DONE {
        signal(instance as i32);
    }
}
pub(super) fn signal(fd: i32) {
    let _ = kinakaze_vfs::eventfd::write_eventfd(fd, &1u64.to_ne_bytes(), true);
}
pub(super) fn clear(fd: i32) {
    let _ = kinakaze_vfs::eventfd::read_eventfd(fd, &mut [0; 8], true);
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_poll_descriptors_count")]
pub unsafe extern "sysv64" fn snd_pcm_poll_descriptors_count(pcm: *mut SndPcm) -> c_int {
    if pcm.is_null() { -22 } else { 1 }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_poll_descriptors")]
pub unsafe extern "sysv64" fn snd_pcm_poll_descriptors(
    pcm: *mut SndPcm,
    fds: *mut PollFd,
    space: c_uint,
) -> c_int {
    if pcm.is_null() || fds.is_null() || space < 1 {
        return -22;
    }
    unsafe {
        *fds = PollFd {
            fd: (*pcm).poll_fd,
            events: 1,
            revents: 0,
        };
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_poll_descriptors_revents")]
pub unsafe extern "sysv64" fn snd_pcm_poll_descriptors_revents(
    pcm: *mut SndPcm,
    fds: *mut PollFd,
    count: c_uint,
    revents: *mut u16,
) -> c_int {
    if pcm.is_null()
        || fds.is_null()
        || count != 1
        || revents.is_null()
        || unsafe { (*fds).fd != (*pcm).poll_fd }
    {
        return -22;
    }
    unsafe {
        *revents = (*fds).revents as u16 & (8 | 16 | 32);
        if (*fds).revents & 1 != 0 {
            clear((*pcm).poll_fd);
            if snd_pcm_avail_update(pcm) >= (*pcm).sw_params.avail_min.max(1) as i64 {
                *revents |= 4;
                signal((*pcm).poll_fd);
            }
        }
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_link")]
pub unsafe extern "sysv64" fn snd_pcm_link(first: *mut SndPcm, second: *mut SndPcm) -> c_int {
    if first.is_null() || second.is_null() || first == second {
        -22
    } else {
        -38
    }
    // WinMM cannot atomically start independent PCM clocks as a hardware group.
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_unlink")]
pub unsafe extern "sysv64" fn snd_pcm_unlink(pcm: *mut SndPcm) -> c_int {
    if pcm.is_null() { -22 } else { 0 } // No linked groups are accepted above.
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_wait")]
pub unsafe extern "sysv64" fn snd_pcm_wait(pcm: *mut SndPcm, timeout: c_int) -> c_int {
    if pcm.is_null() {
        return -22;
    }
    let deadline = (timeout >= 0).then(|| Instant::now() + Duration::from_millis(timeout as u64));
    loop {
        if unsafe { snd_pcm_avail_update(pcm) }
            >= unsafe { (*pcm).sw_params.avail_min.max(1) } as i64
        {
            return 1;
        }
        let remaining = deadline.map_or(-1, |end| {
            end.saturating_duration_since(Instant::now())
                .as_millis()
                .min(i32::MAX as u128) as i32
        });
        let mut fd = PollFd {
            fd: unsafe { (*pcm).poll_fd },
            events: 1,
            revents: 0,
        };
        let result = unsafe { libc::fdio::kinakaze_abi_poll(&raw mut fd, 1, remaining) };
        if result < 0 {
            return -kinakaze_tls::errno();
        }
        if result == 0 {
            return 0;
        }
        clear(fd.fd);
    }
}
