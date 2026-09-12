use super::*;

fn now() -> i128 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(value) => value.as_micros() as i128,
        Err(value) => -(value.duration().as_micros() as i128),
    }
}
fn micros(tv: Timeval) -> i128 {
    i128::from(tv.tv_sec) * 1_000_000 + i128::from(tv.tv_usec)
}
fn millis(us: i128) -> c_int {
    ((us.max(0) + 999) / 1000).min(c_int::MAX as i128) as c_int
}
fn interest(flags: c_int) -> i16 {
    (if flags & 1 != 0 { fdio::POLLIN } else { 0 })
        | (if flags & 2 != 0 { fdio::POLLOUT } else { 0 })
}
fn occurred(revents: i16) -> c_int {
    (if revents & fdio::POLLIN != 0 { 1 } else { 0 })
        | (if revents & fdio::POLLOUT != 0 { 2 } else { 0 })
        | (if revents & fdio::POLLHUP != 0 { 4 } else { 0 })
        | (if revents & (fdio::POLLERR | fdio::POLLNVAL) != 0 {
            8
        } else {
            0
        })
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_mainloop_prepare")]
pub unsafe extern "sysv64" fn pa_mainloop_prepare(
    pointer: *mut pa_mainloop,
    timeout: c_int,
) -> c_int {
    let Some(loop_) = (unsafe { pointer.as_ref() }) else {
        return -1;
    };
    loop_.drain_wakeup();
    let mut state = lock(&loop_.state);
    if state.quitting {
        return -2;
    }
    if state.phase != Phase::Idle || timeout < -1 {
        return -1;
    }
    let mut fds = core::mem::take(&mut state.fds);
    let mut ids = core::mem::take(&mut state.ids);
    fds.clear();
    ids.clear();
    fds.push(PollFd {
        fd: loop_.wake_fd,
        events: fdio::POLLIN,
        revents: 0,
    });
    ids.push(0);
    let now = now();
    let mut wait = if timeout < 0 {
        None
    } else {
        Some(i128::from(timeout))
    };
    for (&id, event) in &state.events {
        let delay = match event.kind {
            Kind::Io { fd, flags, .. } if flags != 0 => {
                fds.push(PollFd {
                    fd,
                    events: interest(flags),
                    revents: 0,
                });
                ids.push(id);
                None
            }
            Kind::Time {
                deadline: Some(tv), ..
            } => Some((micros(tv) - now).max(0)),
            Kind::Defer { enabled: true, .. } => Some(0),
            _ => None,
        };
        if let Some(delay) = delay {
            wait = Some(wait.map_or(delay, |old| old.min(delay)));
        }
    }
    state.timeout_ms = wait.map_or(-1, millis);
    state.fds = fds;
    state.ids = ids;
    state.phase = Phase::Prepared;
    0
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_mainloop_poll")]
pub unsafe extern "sysv64" fn pa_mainloop_poll(pointer: *mut pa_mainloop) -> c_int {
    let Some(loop_) = (unsafe { pointer.as_ref() }) else {
        return -1;
    };
    let (mut fds, timeout, callback, userdata, deferred) = {
        let mut state = lock(&loop_.state);
        if state.quitting {
            state.phase = Phase::Idle;
            return -2;
        }
        if state.phase != Phase::Prepared {
            return -1;
        }
        (
            core::mem::take(&mut state.fds),
            state.timeout_ms,
            state.poll_func,
            state.poll_userdata,
            state
                .events
                .values()
                .any(|event| matches!(event.kind, Kind::Defer { enabled: true, .. })),
        )
    };
    let mut result = if deferred {
        0
    } else {
        unsafe {
            match callback {
                Some(callback) => callback(
                    fds.as_mut_ptr(),
                    fds.len() as u64,
                    timeout,
                    userdata as *mut c_void,
                ),
                None => fdio::kinakaze_abi_poll(fds.as_mut_ptr(), fds.len() as u64, timeout),
            }
        }
    };
    if result < 0 && libc::kinakaze_errno() == 4 {
        result = 0;
        for fd in &mut fds {
            fd.revents = 0;
        }
    }
    let mut state = lock(&loop_.state);
    state.fds = fds;
    state.phase = if result < 0 {
        Phase::Idle
    } else {
        Phase::Polled
    };
    result
}

enum Call {
    Io(IoCallback, c_int, c_int),
    Time(TimeCallback, Timeval),
    Defer(DeferCallback),
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_mainloop_dispatch")]
pub unsafe extern "sysv64" fn pa_mainloop_dispatch(pointer: *mut pa_mainloop) -> c_int {
    let Some(loop_) = (unsafe { pointer.as_ref() }) else {
        return -1;
    };
    let mut ready = {
        let mut state = lock(&loop_.state);
        if state.phase != Phase::Polled {
            return -1;
        }
        state.phase = Phase::Idle;
        if state.quitting {
            return -2;
        }
        let mut ready = core::mem::take(&mut state.ready);
        ready.clear();
        let now = now();
        let deferred = state
            .events
            .values()
            .any(|event| matches!(event.kind, Kind::Defer { enabled: true, .. }));
        for (&id, event) in &state.events {
            match event.kind {
                Kind::Time {
                    deadline: Some(tv), ..
                } if !deferred && micros(tv) <= now => ready.push((id, Trigger::Time)),
                Kind::Defer { enabled: true, .. } => ready.push((id, Trigger::Defer)),
                _ => {}
            }
        }
        for (fd, &id) in state
            .fds
            .iter()
            .zip(&state.ids)
            .skip(1)
            .filter(|_| !deferred)
        {
            let flags = occurred(fd.revents);
            if flags != 0 {
                ready.push((id, Trigger::Io(flags)));
            }
        }
        ready
    };
    let api = unsafe { pa_mainloop_get_api(pointer) };
    let mut count = 0;
    for &(id, trigger) in &ready {
        let call = {
            let mut state = lock(&loop_.state);
            if state.quitting {
                break;
            }
            let Some(event) = state.events.get_mut(&id) else {
                continue;
            };
            let call = match (&mut event.kind, trigger) {
                (
                    Kind::Io {
                        fd,
                        flags,
                        callback,
                    },
                    Trigger::Io(ready),
                ) if *flags != 0 => {
                    let ready = ready & (*flags | 4 | 8);
                    if ready == 0 {
                        continue;
                    }
                    Call::Io(*callback, *fd, ready)
                }
                (Kind::Time { deadline, callback }, Trigger::Time) => {
                    let Some(tv) = *deadline else {
                        continue;
                    };
                    if micros(tv) > now() {
                        continue;
                    }
                    *deadline = None; // one-shot, may be restarted by its callback
                    Call::Time(*callback, tv)
                }
                (
                    Kind::Defer {
                        enabled: true,
                        callback,
                    },
                    Trigger::Defer,
                ) => Call::Defer(*callback),
                _ => continue,
            };
            (
                call,
                (&raw mut **event).cast::<c_void>(),
                event.userdata as *mut c_void,
            )
        };
        // IDs never repeat, so a callback freeing another ready event and
        // allocating a replacement cannot dispatch the replacement too early.
        unsafe {
            match call.0 {
                Call::Io(callback, fd, flags) => callback(api, call.1, fd, flags, call.2),
                Call::Time(callback, tv) => callback(api, call.1, &raw const tv, call.2),
                Call::Defer(callback) => callback(api, call.1, call.2),
            }
        }
        count += 1;
    }
    ready.clear();
    let mut state = lock(&loop_.state);
    state.ready = ready;
    if state.quitting { -2 } else { count }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_mainloop_iterate")]
pub unsafe extern "sysv64" fn pa_mainloop_iterate(
    pointer: *mut pa_mainloop,
    block: c_int,
    retval: *mut c_int,
) -> c_int {
    let result = unsafe { pa_mainloop_prepare(pointer, if block != 0 { -1 } else { 0 }) };
    let result = if result < 0 {
        result
    } else {
        unsafe { pa_mainloop_poll(pointer) }
    };
    let result = if result < 0 {
        result
    } else {
        unsafe { pa_mainloop_dispatch(pointer) }
    };
    if result == -2 && !retval.is_null() {
        unsafe {
            *retval = pa_mainloop_get_retval(pointer);
        }
    }
    result
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_mainloop_run")]
pub unsafe extern "sysv64" fn pa_mainloop_run(
    pointer: *mut pa_mainloop,
    retval: *mut c_int,
) -> c_int {
    loop {
        match unsafe { pa_mainloop_iterate(pointer, 1, retval) } {
            -2 => return 1,
            result if result < 0 => return result,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timeout_rounds_up_and_does_not_overflow() {
        assert_eq!(millis(-1), 0);
        assert_eq!(millis(1), 1);
        assert_eq!(millis(1001), 2);
        assert_eq!(millis(i128::from(i64::MAX) * 1_000_000), i32::MAX);
        assert_eq!(occurred(fdio::POLLNVAL | fdio::POLLHUP), 12);
    }
}
