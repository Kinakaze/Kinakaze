//! Match immediate TCP loopback refusal without changing host-wide settings.
//! Windows normally retries a rejected loopback SYN for about two seconds.
//! Linux callers with a shorter deadline otherwise mistake refusal for timeout.
use windows_sys::Win32::Networking::WinSock::{
    SIO_TCP_INITIAL_RTO, SO_TYPE, SOCK_STREAM, SOCKET, SOCKET_ERROR, SOL_SOCKET,
    TCP_INITIAL_RTO_DEFAULT_MAX_SYN_RETRANSMISSIONS, TCP_INITIAL_RTO_NO_SYN_RETRANSMISSIONS,
    TCP_INITIAL_RTO_PARAMETERS, WSAIoctl, getsockopt,
};

fn loopback(address: &[u8]) -> Option<bool> {
    let family = u16::from_ne_bytes(address.get(..2)?.try_into().ok()?);
    match family {
        2 if address.len() >= 16 => Some(address[4] == 127),
        23 if address.len() >= 28 => {
            let ip = &address[8..24];
            Some(
                (ip[..15] == [0; 15] && ip[15] == 1)
                    || (ip[..10] == [0; 10] && ip[10..12] == [255; 2] && ip[12] == 127),
            )
        }
        _ => None,
    }
}

/// The socket is owned by this runtime. Failure leaves normal Winsock connect
/// behavior intact; it must not be converted to a fabricated connection error.
pub(crate) fn configure(socket: SOCKET, address: &[u8]) -> Result<(), i32> {
    let Some(local) = loopback(address) else {
        return Ok(());
    };
    let mut kind = 0i32;
    let mut length = size_of::<i32>() as i32;
    // SAFETY: the two outputs are writable locals and socket is live.
    if unsafe {
        getsockopt(
            socket,
            SOL_SOCKET,
            SO_TYPE,
            (&raw mut kind).cast(),
            &mut length,
        )
    } == SOCKET_ERROR
    {
        return Err(super::last_wsa_errno());
    }
    if kind != SOCK_STREAM {
        return Ok(());
    }
    let parameters = TCP_INITIAL_RTO_PARAMETERS {
        // Preserve the administrator's initial RTT. Only loopback skips SYN
        // retries; a subsequent non-loopback attempt restores system policy.
        Rtt: u16::MAX,
        MaxSynRetransmissions: if local {
            TCP_INITIAL_RTO_NO_SYN_RETRANSMISSIONS as u8
        } else {
            // UNSPECIFIED preserves a preceding per-socket override on current
            // Windows; DEFAULT explicitly re-enables normal SYN retries.
            TCP_INITIAL_RTO_DEFAULT_MAX_SYN_RETRANSMISSIONS as u8
        },
    };
    let mut returned = 0;
    // SAFETY: input/output storage stays valid through this synchronous ioctl.
    let result = unsafe {
        WSAIoctl(
            socket,
            SIO_TCP_INITIAL_RTO,
            (&raw const parameters).cast(),
            size_of::<TCP_INITIAL_RTO_PARAMETERS>() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
            None,
        )
    };
    if result == SOCKET_ERROR {
        Err(super::last_wsa_errno())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
