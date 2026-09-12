//! Caller-owned SPA hooks. Guest spa_hook_remove unlinks the intrusive node;
//! dispatch snapshots callbacks and checks membership before every call.
use super::*;
pub(super) struct Hooks {
    head: Box<SpaHook>,
}
impl Hooks {
    pub(super) fn new() -> Self {
        let mut head = Box::new(SpaHook { _opaque: [0; 6] });
        let pointer = (&raw mut *head) as usize;
        head._opaque[0] = pointer;
        head._opaque[1] = pointer;
        Self { head }
    }
    pub(super) unsafe fn add(
        &mut self,
        hook: *mut SpaHook,
        funcs: *const c_void,
        data: *mut c_void,
    ) -> c_int {
        if hook.is_null() || funcs.is_null() || self.contains(hook as usize) {
            return -22;
        }
        let head = (&raw mut *self.head) as usize;
        let previous = self.head._opaque[1] as *mut SpaHook;
        unsafe {
            (*hook)._opaque = [head, previous as usize, funcs as usize, data as usize, 0, 1];
            (*previous)._opaque[0] = hook as usize;
        }
        self.head._opaque[1] = hook as usize;
        0
    }
    pub(super) fn snapshot(&self) -> Vec<(usize, SpaCallbacks, bool)> {
        let head = (&raw const *self.head) as usize;
        let mut next = self.head._opaque[0];
        let mut items = Vec::new();
        while next != head {
            let hook = unsafe { &*(next as *const SpaHook) };
            items.push((
                next,
                SpaCallbacks {
                    funcs: hook._opaque[2] as _,
                    data: hook._opaque[3] as _,
                },
                hook._opaque[5] != 0,
            ));
            next = hook._opaque[0];
        }
        items
    }
    pub(super) fn contains(&self, pointer: usize) -> bool {
        let head = (&raw const *self.head) as usize;
        let mut next = self.head._opaque[0];
        while next != head {
            if next == pointer {
                return true;
            }
            next = unsafe { (*(next as *const SpaHook))._opaque[0] };
        }
        false
    }
    pub(super) unsafe fn initialized(pointer: usize) {
        unsafe {
            (*(pointer as *mut SpaHook))._opaque[5] = 0;
        }
    }
}
impl Drop for Hooks {
    fn drop(&mut self) {
        let head = (&raw mut *self.head) as usize;
        while self.head._opaque[0] != head {
            let hook = self.head._opaque[0] as *mut SpaHook;
            unsafe {
                let next = (*hook)._opaque[0] as *mut SpaHook;
                self.head._opaque[0] = next as usize;
                (*next)._opaque[1] = head;
                (*hook)._opaque = [hook as usize, hook as usize, 0, 0, 0, 0];
            }
        }
    }
}
