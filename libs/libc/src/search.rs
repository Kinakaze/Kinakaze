//! POSIX search trees. Nodes live in the guest arena, so the caller's root and
//! every link retain their meaning across fork without a native heap snapshot.
//! AVL balancing bounds lookup, mutation and traversal stack depth.

use core::ffi::{c_int, c_void};
use core::ptr;
use kinakaze_alloc::guest;
mod hash;
mod linear;

type Compare = unsafe extern "sysv64" fn(*const c_void, *const c_void) -> c_int;

#[repr(C)]
struct Node {
    // POSIX exposes a pointer to this first field, not to the parent link.
    key: *const c_void,
    left: *mut Node,
    right: *mut Node,
    height: u8,
}

unsafe fn height(node: *const Node) -> u8 {
    if node.is_null() {
        0
    } else {
        unsafe { (*node).height }
    }
}

unsafe fn update(node: *mut Node) {
    unsafe { (*node).height = 1 + height((*node).left).max(height((*node).right)) };
}

unsafe fn rotate_left(node: *mut Node) -> *mut Node {
    unsafe {
        let next = (*node).right;
        (*node).right = (*next).left;
        (*next).left = node;
        update(node);
        update(next);
        next
    }
}

unsafe fn rotate_right(node: *mut Node) -> *mut Node {
    unsafe {
        let next = (*node).left;
        (*node).left = (*next).right;
        (*next).right = node;
        update(node);
        update(next);
        next
    }
}

unsafe fn balance(node: *mut Node) -> *mut Node {
    unsafe {
        update(node);
        let left = (*node).left;
        let right = (*node).right;
        if height(left) > height(right) + 1 {
            if height((*left).right) > height((*left).left) {
                (*node).left = rotate_left(left);
            }
            rotate_right(node)
        } else if height(right) > height(left) + 1 {
            if height((*right).left) > height((*right).right) {
                (*node).right = rotate_right(right);
            }
            rotate_left(node)
        } else {
            node
        }
    }
}

unsafe fn insert(link: *mut *mut Node, key: *const c_void, compare: Compare) -> *mut Node {
    unsafe {
        let node = *link;
        if node.is_null() {
            let new = guest::malloc(core::mem::size_of::<Node>()).cast::<Node>();
            if new.is_null() {
                crate::set_errno(crate::ENOMEM);
                return new;
            }
            new.write(Node {
                key,
                left: ptr::null_mut(),
                right: ptr::null_mut(),
                height: 1,
            });
            *link = new;
            return new;
        }
        let order = compare(key, (*node).key);
        if order == 0 {
            return node;
        }
        let child = if order < 0 {
            &raw mut (*node).left
        } else {
            &raw mut (*node).right
        };
        let found = insert(child, key, compare);
        if !found.is_null() {
            *link = balance(node);
        }
        found
    }
}

/// Pointers and callbacks follow search.h; callers synchronize a shared tree.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_tsearch(
    key: *const c_void,
    root: *mut *mut c_void,
    compare: Option<Compare>,
) -> *mut c_void {
    let Some(compare) = compare else {
        return ptr::null_mut();
    };
    if root.is_null() {
        return ptr::null_mut();
    }
    unsafe { insert(root.cast(), key, compare).cast() }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_tfind(
    key: *const c_void,
    root: *const *mut c_void,
    compare: Option<Compare>,
) -> *mut c_void {
    let Some(compare) = compare else {
        return ptr::null_mut();
    };
    if root.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        let mut node = (*root).cast::<Node>();
        while !node.is_null() {
            let order = compare(key, (*node).key);
            if order == 0 {
                return node.cast();
            }
            node = if order < 0 {
                (*node).left
            } else {
                (*node).right
            };
        }
    }
    ptr::null_mut()
}

unsafe fn detach_min(link: *mut *mut Node) -> *mut Node {
    unsafe {
        let node = *link;
        if (*node).left.is_null() {
            *link = (*node).right;
            node
        } else {
            let found = detach_min(&raw mut (*node).left);
            *link = balance(node);
            found
        }
    }
}

unsafe fn delete(
    link: *mut *mut Node,
    key: *const c_void,
    compare: Compare,
    parent: *mut Node,
) -> *mut Node {
    unsafe {
        let node = *link;
        if node.is_null() {
            return ptr::null_mut();
        }
        let order = compare(key, (*node).key);
        if order != 0 {
            let child = if order < 0 {
                &raw mut (*node).left
            } else {
                &raw mut (*node).right
            };
            let found = delete(child, key, compare, node);
            if !found.is_null() {
                *link = balance(node);
            }
            return found;
        }
        if !(*node).left.is_null() && !(*node).right.is_null() {
            let successor = detach_min(&raw mut (*node).right);
            (*node).key = (*successor).key;
            guest::free(successor.cast());
            *link = balance(node);
        } else {
            *link = if (*node).left.is_null() {
                (*node).right
            } else {
                (*node).left
            };
            guest::free(node.cast());
        }
        // For deletion of the root, POSIX leaves the non-null result unspecified.
        // In the last-node case it is only a success token, never dereferenced.
        if parent.is_null() { node } else { parent }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_tdelete(
    key: *const c_void,
    root: *mut *mut c_void,
    compare: Option<Compare>,
) -> *mut c_void {
    let Some(compare) = compare else {
        return ptr::null_mut();
    };
    if root.is_null() {
        return ptr::null_mut();
    }
    unsafe { delete(root.cast(), key, compare, ptr::null_mut()).cast() }
}

unsafe fn walk(
    node: *const Node,
    depth: c_int,
    visit: &mut impl FnMut(*const c_void, c_int, c_int),
) {
    if node.is_null() {
        return;
    }
    unsafe {
        if (*node).left.is_null() && (*node).right.is_null() {
            visit(node.cast(), 3, depth); // leaf
        } else {
            visit(node.cast(), 0, depth); // preorder
            walk((*node).left, depth + 1, visit);
            visit(node.cast(), 1, depth); // postorder: between the children
            walk((*node).right, depth + 1, visit);
            visit(node.cast(), 2, depth); // endorder
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_twalk(
    root: *const c_void,
    action: Option<unsafe extern "sysv64" fn(*const c_void, c_int, c_int)>,
) {
    if let Some(action) = action {
        unsafe {
            walk(root.cast(), 0, &mut |node, order, depth| {
                action(node, order, depth)
            })
        };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_twalk_r(
    root: *const c_void,
    action: Option<unsafe extern "sysv64" fn(*const c_void, c_int, *mut c_void)>,
    context: *mut c_void,
) {
    if let Some(action) = action {
        unsafe {
            walk(root.cast(), 0, &mut |node, order, _| {
                action(node, order, context)
            })
        };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_tdestroy(
    root: *mut c_void,
    free_key: Option<unsafe extern "sysv64" fn(*mut c_void)>,
) {
    if root.is_null() {
        return;
    }
    unsafe {
        let node = root.cast::<Node>();
        kinakaze_abi_tdestroy((*node).left.cast(), free_key);
        kinakaze_abi_tdestroy((*node).right.cast(), free_key);
        if let Some(free_key) = free_key {
            free_key((*node).key.cast_mut());
        }
        guest::free(node.cast());
    }
}
