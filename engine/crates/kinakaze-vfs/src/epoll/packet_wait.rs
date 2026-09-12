//! Packet transports wait for input, protocol deadlines and endpoint commands
//! using the same cancellable AFD request as epoll. Raw UDP writability is only
//! requested when an actual packet send encountered backpressure.
use super::*;
use std::net::UdpSocket;
use std::os::windows::io::AsRawSocket;

pub(crate) struct PacketWait {
    complete: crate::fs::object::Object,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compacted_afd_results_do_not_report_an_unready_second_socket() {
        let first = UdpSocket::bind("127.0.0.1:0").unwrap();
        let second = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        sender
            .send_to(b"ready", first.local_addr().unwrap())
            .unwrap();
        let changed = crate::fs::object::Object::owned(unsafe {
            CreateEventW(std::ptr::null(), 1, 0, std::ptr::null())
        })
        .unwrap();
        let wait = PacketWait::new().unwrap();
        let ready = unsafe {
            wait.sockets(
                &[
                    (first.as_raw_socket() as usize, 1),
                    (second.as_raw_socket() as usize, 1),
                ],
                changed.raw(),
                Some(1000),
            )
        }
        .unwrap();
        assert_ne!(ready[0] & 1, 0);
        assert_eq!(ready[1], 0);
    }
}
impl PacketWait {
    /// Gateway sockets are owned by the caller throughout submission and AFD
    /// cancellation. Read/write interests reflect actual buffer pressure.
    pub(crate) unsafe fn sockets(
        &self,
        sockets: &[(usize, u32)],
        changed: HANDLE,
        timeout: Option<u32>,
    ) -> Result<Vec<u32>, i32> {
        unsafe { self.sockets_policy(sockets, changed, timeout, true) }
    }
    /// Finish a bounded internal publication, e.g. listen's host activation.
    /// Pending guest signals remain pending until the syscall boundary; this
    /// must not turn a completed nonblocking operation into a spurious EINTR.
    pub(crate) unsafe fn publication(
        &self,
        sockets: &[(usize, u32)],
        changed: HANDLE,
        timeout: u32,
    ) -> Result<Vec<u32>, i32> {
        unsafe { self.sockets_policy(sockets, changed, Some(timeout), false) }
    }
    unsafe fn sockets_policy(
        &self,
        sockets: &[(usize, u32)],
        changed: HANDLE,
        timeout: Option<u32>,
        interruptible: bool,
    ) -> Result<Vec<u32>, i32> {
        let mut info: AfdPollInfo = unsafe { std::mem::zeroed() };
        if sockets.is_empty() || sockets.len() > info.handles.len() {
            return Err(EINVAL);
        }
        info.handle_count = sockets.len() as u32;
        let mut bases = Vec::with_capacity(sockets.len());
        for (entry, &(raw, interest)) in info.handles.iter_mut().zip(sockets) {
            entry.handle = base_handle(raw)?;
            bases.push(entry.handle);
            entry.events = interest | AFD_POLL_ABORT | AFD_POLL_LOCAL_CLOSE | AFD_POLL_CONNECT_FAIL;
        }
        if !matches!(afd_poll_batch_policy(&mut info, timeout, self.complete.raw(), changed, std::ptr::null_mut(),interruptible)?, AfdPollResult::Ready(n) if n!=0)
        {
            return Ok(vec![0; sockets.len()]);
        }
        // AFD compacts the output array to the ready handles. Unreturned slots
        // still contain the caller's requested interests, not readiness!
        let mut ready = vec![0; sockets.len()];
        for entry in info.handles.iter().take(info.handle_count as usize) {
            for (index, base) in bases.iter().enumerate() {
                if *base == entry.handle {
                    ready[index] |= entry.events;
                }
            }
        }
        Ok(ready)
    }
    pub(crate) fn new() -> Result<Self, i32> {
        Ok(Self {
            complete: crate::fs::object::Object::owned(unsafe {
                CreateEventW(std::ptr::null(), 1, 0, std::ptr::null())
            })?,
        })
    }
    /// `changed` must remain live until this call returns. The borrowed socket
    /// keeps the AFD handle alive across submission, cancellation and retirement.
    pub(crate) unsafe fn wait(
        &self,
        socket: &UdpSocket,
        changed: HANDLE,
        output_blocked: bool,
        timeout: Option<u32>,
    ) -> Result<(), i32> {
        unsafe {
            self.wait_extra(
                socket,
                changed,
                std::ptr::null_mut(),
                output_blocked,
                timeout,
            )
        }
    }
    pub(crate) unsafe fn wait_extra(
        &self,
        socket: &UdpSocket,
        changed: HANDLE,
        lifetime: HANDLE,
        output_blocked: bool,
        timeout: Option<u32>,
    ) -> Result<(), i32> {
        let mut info: AfdPollInfo = unsafe { std::mem::zeroed() };
        info.handle_count = 1;
        info.handles[0].handle = base_handle(socket.as_raw_socket() as SOCKET)?;
        info.handles[0].events = AFD_POLL_RECEIVE
            | AFD_POLL_ABORT
            | AFD_POLL_LOCAL_CLOSE
            | if output_blocked { AFD_POLL_SEND } else { 0 };
        let result = afd_poll_batch(&mut info, timeout, self.complete.raw(), changed, lifetime)?;
        if matches!(result, AfdPollResult::Ready(n) if n != 0)
            && info.handles[0].events & (AFD_POLL_ABORT | AFD_POLL_LOCAL_CLOSE) != 0
        {
            return Err(EIO);
        }
        Ok(())
    }
}
