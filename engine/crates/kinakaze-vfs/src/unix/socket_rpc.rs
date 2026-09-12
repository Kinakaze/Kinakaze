//! Winsock recipes are bound to a target PID. The keeper produces a fresh
//! recipe for each recvmsg, including repeated MSG_PEEK calls.
use super::*;
use crate::mount::shared::Store;
use crate::state_codec::{Reader, bytes, word};

pub(super) fn notification(id: u64) -> Result<Object, i32> {
    Object::owned(unsafe {
        CreateEventW(
            std::ptr::null(),
            0,
            0,
            wide(&format!("Local\\kinakaze-rights-rpc-{id:x}")).as_ptr(),
        )
    })
}

fn completion(token: u64) -> Result<Object, i32> {
    Object::owned(unsafe {
        CreateEventW(
            std::ptr::null(),
            1,
            0,
            wide(&format!("Local\\kinakaze-rights-reply-{token:x}")).as_ptr(),
        )
    })
}

fn wait_for_completion(reply: &Object, keeper: HANDLE, timeout: u32) -> Result<(), i32> {
    // Payload has already been consumed by recvmsg. Do not introduce EINTR here:
    // retrying the entire receive would lose its accompanying descriptors.
    let handles = [reply.raw(), keeper];
    match unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, timeout) } {
        WAIT_OBJECT_0 => Ok(()),
        _ => Err(EIO),
    }
}
pub(super) fn request(
    id: u64,
    index: usize,
    keeper: HANDLE,
) -> Result<crate::socket::RightsSocket, i32> {
    let store = Store::user_object(id, false)?;
    let token = random_id()?;
    // Enroll before publishing. A keeper may finish before the caller waits;
    // the manual-reset event retains that completion without a polling loop.
    let reply = completion(token)?;
    let mut payload = Vec::new();
    for value in [
        1,
        std::process::id() as u64,
        creation_time(unsafe { GetCurrentProcess() })?,
        index as u64,
        token,
    ] {
        word(&mut payload, value);
    }
    store.update(|_| Ok((payload, ())))?;
    let notify = notification(id)?;
    unsafe {
        SetEvent(notify.raw());
    }
    wait_for_completion(&reply, keeper, 10_000)?;
    let (_, payload) = store.read()?;
    let mut input = Reader(&payload);
    if input.word()? != 2 || input.word()? != token {
        return Err(EIO);
    }
    let error = input.word()? as i32;
    if error != 0 {
        return Err(error);
    }
    let protocol = input.bytes()?;
    input.end()?;
    crate::socket::import_rights(protocol)
}
pub(super) fn serve(store: &Store, descriptors: &[i32]) -> Result<(), i32> {
    let (_, payload) = store.read()?;
    if payload.is_empty() {
        return Ok(());
    }
    let mut input = Reader(&payload);
    if input.word()? != 1 {
        return Ok(());
    }
    let pid = input.word()? as u32;
    let created = input.word()?;
    let index = input.word()? as usize;
    let token = input.word()?;
    input.end()?;
    let result = (|| {
        let process =
            Object::owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })?;
        if creation_time(process.raw())? != created {
            return Err(EIO);
        }
        let entry = crate::get(*descriptors.get(index).ok_or(EINVAL)?)?;
        if entry.kind != FdKind::Socket {
            return Err(EINVAL);
        }
        crate::socket::export_rights(entry.raw, pid)
    })();
    let mut response = Vec::new();
    word(&mut response, 2);
    word(&mut response, token);
    match result {
        Ok(protocol) => {
            word(&mut response, 0);
            bytes(&mut response, &protocol);
        }
        Err(e) => word(&mut response, e as u64),
    }
    publish_response(store, &payload, response, token)
}

fn publish_response(
    store: &Store,
    request: &[u8],
    response: Vec<u8>,
    token: u64,
) -> Result<(), i32> {
    // A previous caller may have timed out. Its delayed reply must not replace
    // the request of a later MSG_PEEK/recvmsg using this keeper.
    let published = store.update(|current| {
        if current == request {
            Ok((response, true))
        } else {
            Ok((current.to_vec(), false))
        }
    })?;
    if published {
        let reply = completion(token)?;
        if unsafe { SetEvent(reply.raw()) } == 0 {
            return Err(EIO);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::WAIT_TIMEOUT;

    #[test]
    fn reply_before_wait_is_retained_and_old_replies_do_not_replace_new_requests() {
        let store = crate::mount::shared::new_object().unwrap();
        let token = random_id().unwrap();
        let reply = completion(token).unwrap();
        store.update(|_| Ok((b"new".to_vec(), ()))).unwrap();
        publish_response(&store, b"old", b"obsolete".to_vec(), token).unwrap();
        assert_eq!(store.read().unwrap().1, b"new");
        assert_eq!(unsafe { WaitForSingleObject(reply.raw(), 0) }, WAIT_TIMEOUT);
        publish_response(&store, b"new", b"response".to_vec(), token).unwrap();
        assert_eq!(store.read().unwrap().1, b"response");
        let keeper = Object::duplicate(unsafe { GetCurrentProcess() }).unwrap();
        assert_eq!(wait_for_completion(&reply, keeper.raw(), 0), Ok(()));
    }

    #[test]
    fn blocked_request_wakes_on_reply_and_keeper_exit_without_polling() {
        let reply = completion(random_id().unwrap()).unwrap();
        // A manual event models the documented signaled process-object state.
        let exited = completion(random_id().unwrap()).unwrap();
        assert_eq!(wait_for_completion(&reply, exited.raw(), 0), Err(EIO));
        unsafe {
            SetEvent(exited.raw());
        }
        assert_eq!(
            wait_for_completion(&reply, exited.raw(), u32::MAX),
            Err(EIO)
        );
        unsafe {
            ResetEvent(exited.raw());
        }
        let signal = Object::duplicate(reply.raw()).unwrap();
        let worker = std::thread::spawn(move || unsafe { SetEvent(signal.raw()) });
        assert_eq!(wait_for_completion(&reply, exited.raw(), 1000), Ok(()));
        worker.join().unwrap();
    }

    #[test]
    fn actual_socket_recipe_reply_imports_a_new_native_socket() {
        let fd = crate::socket::socket(crate::socket::AF_INET, SOCK_STREAM, 0).unwrap();
        let store = crate::mount::shared::new_object().unwrap();
        let notify = notification(store.id()).unwrap();
        let id = store.id();
        let worker = std::thread::spawn(move || {
            assert_eq!(
                unsafe { WaitForSingleObject(notify.raw(), 2000) },
                WAIT_OBJECT_0
            );
            serve(&store, &[fd]).unwrap();
        });
        let keeper = Object::duplicate(unsafe { GetCurrentProcess() }).unwrap();
        let result = request(id, 0, keeper.raw());
        worker.join().unwrap();
        crate::close(fd).unwrap();
        assert!(
            result.is_ok(),
            "recipe import errno={:?}",
            result.as_ref().err()
        );
        drop(result);
    }
}
