//! Client-side XKB records. Their arrays are owned by the guest allocator and
//! can be released by XFree as required by the Xlib ABI.
use super::*;
use core::{mem, ptr};
use kinakaze_alloc::guest;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Mods {
    pub mask: u8,
    pub real_mods: u8,
    pub vmods: u16,
}
#[repr(C)]
pub struct TypeEntry {
    pub active: Bool,
    pub level: u8,
    pub mods: Mods,
}
#[repr(C)]
pub struct KeyType {
    pub mods: Mods,
    pub num_levels: u8,
    pub map_count: u8,
    pub map: *mut TypeEntry,
    pub preserve: *mut Mods,
    pub name: usize,
    pub level_names: *mut usize,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SymMap {
    pub types: [u8; 4],
    pub group_info: u8,
    pub width: u8,
    pub offset: u16,
}
#[repr(C)]
pub struct ClientMap {
    pub size_types: u8,
    pub num_types: u8,
    pub types: *mut KeyType,
    pub size_syms: u16,
    pub num_syms: u16,
    pub syms: *mut usize,
    pub key_sym_map: *mut SymMap,
    pub modmap: *mut u8,
}
#[repr(C)]
pub struct ServerMap {
    pub num_acts: u16,
    pub size_acts: u16,
    pub acts: *mut [u8; 8],
    pub behaviors: *mut [u8; 2],
    pub key_acts: *mut u16,
    pub explicit: *mut u8,
    pub vmods: [u8; 16],
    pub vmodmap: *mut u16,
}
#[repr(C)]
pub struct Names {
    pub keycodes: usize,
    pub geometry: usize,
    pub symbols: usize,
    pub types: usize,
    pub compat: usize,
    pub vmods: [usize; 16],
    pub indicators: [usize; 32],
    pub groups: [usize; 4],
    pub keys: *mut [u8; 4],
    pub key_aliases: *mut [u8; 8],
    pub radio_groups: *mut usize,
    pub phys_symbols: usize,
    pub num_keys: u8,
    pub num_key_aliases: u8,
    pub num_rg: u16,
}
#[repr(C)]
pub struct SymInterpret {
    sym: usize,
    flags: u8,
    matching: u8,
    mods: u8,
    virtual_mod: u8,
    act: [u8; 8],
}
#[repr(C)]
pub struct CompatMap {
    sym_interpret: *mut SymInterpret,
    groups: [Mods; 4],
    num_si: u16,
    size_si: u16,
}
#[repr(C)]
pub struct IndicatorMap {
    flags: u8,
    which_groups: u8,
    groups: u8,
    which_mods: u8,
    mods: Mods,
    ctrls: u32,
}
#[repr(C)]
pub struct Indicators {
    phys_indicators: u64,
    maps: [IndicatorMap; 32],
}
#[repr(C)]
pub struct Controls {
    mk_dflt_btn: u8,
    num_groups: u8,
    groups_wrap: u8,
    internal: Mods,
    ignore_lock: Mods,
    enabled_ctrls: u32,
    repeat_delay: u16,
    repeat_interval: u16,
    slow_keys_delay: u16,
    debounce_delay: u16,
    mk_delay: u16,
    mk_interval: u16,
    mk_time_to_max: u16,
    mk_max_speed: u16,
    mk_curve: i16,
    ax_options: u16,
    ax_timeout: u16,
    axt_opts_mask: u16,
    axt_opts_values: u16,
    axt_ctrls_mask: u32,
    axt_ctrls_values: u32,
    per_key_repeat: [u8; 32],
}

pub(crate) unsafe fn load_controls(d: *mut Display, x: *mut XkbDescRec) -> Status {
    if x.is_null() {
        return 2;
    }
    if !super::server::valid_device(unsafe { (*x).device_spec }) {
        return super::server::ERROR as i32;
    }
    let status = unsafe { XkbAllocControls(x, 0) };
    if status != 0 {
        return status;
    }
    let mut keyboard = unsafe { mem::zeroed::<crate::keyboard::XKeyboardState>() };
    unsafe {
        crate::XGetKeyboardControl(d, &mut keyboard);
    }
    let controls = unsafe { &mut *(*x).ctrls.cast::<Controls>() };
    controls.num_groups = 1;
    controls.mk_dflt_btn = 1;
    controls.enabled_ctrls = u32::from(keyboard.global_auto_repeat != 0);
    controls.repeat_delay = 660;
    controls.repeat_interval = 40;
    controls.per_key_repeat = keyboard.auto_repeats;
    0
}
pub(crate) unsafe fn load_indicators(which: u64, x: *mut XkbDescRec) -> Status {
    if x.is_null() || which > u32::MAX as u64 {
        return 2;
    }
    let status = unsafe { XkbAllocIndicatorMaps(x) };
    if status != 0 {
        return status;
    }
    let indicators = unsafe { &mut *(*x).indicators.cast::<Indicators>() };
    indicators.phys_indicators = 7;
    for i in 0..32 {
        if which & (1 << i) != 0 {
            let mask = match i {
                0 => 2,
                1 => 16,
                2 => 32,
                _ => 0,
            };
            indicators.maps[i] = IndicatorMap {
                flags: 0,
                which_groups: 0,
                groups: 0,
                which_mods: 4,
                mods: Mods {
                    mask,
                    real_mods: mask,
                    vmods: 0,
                },
                ctrls: 0,
            };
        }
    }
    0
}

unsafe fn alloc<T>(count: usize) -> *mut T {
    let Some(bytes) = mem::size_of::<T>().checked_mul(count) else {
        return ptr::null_mut();
    };
    let out = unsafe { guest::malloc(bytes) }.cast::<T>();
    if !out.is_null() {
        unsafe { ptr::write_bytes(out, 0, count) };
    }
    out
}
unsafe fn ensure<T>(slot: &mut *mut T, count: usize) -> bool {
    if slot.is_null() {
        *slot = unsafe { alloc(count) };
    }
    !slot.is_null()
}
unsafe fn grow<T>(slot: &mut *mut T, old: usize, new: usize) -> bool {
    if new <= old {
        return true;
    }
    let Some(bytes) = mem::size_of::<T>().checked_mul(new) else {
        return false;
    };
    let out = unsafe { guest::reallocate((*slot).cast(), 16, bytes) }.cast::<T>();
    if out.is_null() {
        return false;
    }
    unsafe { ptr::write_bytes(out.add(old), 0, new - old) };
    *slot = out;
    true
}
unsafe fn release<T>(slot: &mut *mut T) {
    unsafe { guest::free((*slot).cast()) };
    *slot = ptr::null_mut();
}
fn valid_keys(x: &XkbDescRec) -> bool {
    x.min_key_code >= 8 && x.max_key_code >= x.min_key_code
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbAllocKeyboard")]
pub unsafe extern "sysv64" fn XkbAllocKeyboard() -> *mut XkbDescRec {
    let x: *mut XkbDescRec = unsafe { alloc(1) };
    if !x.is_null() {
        unsafe { (*x).device_spec = 0x100 };
    }
    x
}
#[unsafe(export_name = "kinakaze_engine_libX11_XkbAllocClientMap")]
pub unsafe extern "sysv64" fn XkbAllocClientMap(
    x: *mut XkbDescRec,
    which: c_uint,
    types: c_uint,
) -> Status {
    if x.is_null() {
        return 8;
    }
    if types > 255 {
        return 2;
    }
    unsafe {
        let x = &mut *x;
        if which & 6 != 0 && !valid_keys(x) {
            return 2;
        }
        if !ensure(&mut x.map, mem::size_of::<ClientMap>()) {
            return 11;
        }
        let m = &mut *x.map.cast::<ClientMap>();
        if which & 1 != 0 && types != 0 {
            if types < 4 {
                return 2;
            }
            if !grow(&mut m.types, m.size_types as usize, types as usize) {
                return 11;
            }
            m.size_types = m.size_types.max(types as u8);
        }
        if which & 2 != 0 {
            if m.syms.is_null() {
                let size = (x.max_key_code as usize - x.min_key_code as usize + 1) * 15 / 10;
                m.syms = alloc(size);
                if m.syms.is_null() {
                    return 11;
                }
                m.size_syms = size as u16;
                m.num_syms = 1; // Offset zero is the shared NoSymbol entry.
            }
            if !ensure(&mut m.key_sym_map, x.max_key_code as usize + 1) {
                return 11;
            }
        }
        if which & 4 != 0 && !ensure(&mut m.modmap, x.max_key_code as usize + 1) {
            return 11;
        }
        0
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XkbAllocServerMap")]
pub unsafe extern "sysv64" fn XkbAllocServerMap(
    x: *mut XkbDescRec,
    which: c_uint,
    actions: c_uint,
) -> Status {
    if x.is_null() {
        return 8;
    }
    if actions > 65534 {
        return 2;
    }
    unsafe {
        let x = &mut *x;
        if which & (8 | 16 | 32 | 128) != 0 && !valid_keys(x) {
            return 2;
        }
        let fresh = x.server.is_null();
        if !ensure(&mut x.server, mem::size_of::<ServerMap>()) {
            return 11;
        }
        let s = &mut *x.server.cast::<ServerMap>();
        if fresh {
            s.vmods.fill(0);
        }
        let count = x.max_key_code as usize + 1;
        if which & 8 != 0 && !ensure(&mut s.explicit, count) {
            return 11;
        }
        if which & 16 != 0 {
            if !ensure(&mut s.key_acts, count) {
                return 11;
            }
            if which & 16 != 0 {
                let needed = s.num_acts.max(1) as usize + actions.max(1) as usize;
                if needed > 65535 {
                    return 11;
                }
                if !grow(&mut s.acts, s.size_acts as usize, needed) {
                    return 11;
                }
                s.size_acts = s.size_acts.max(needed as u16);
                s.num_acts = s.num_acts.max(1);
            }
        }
        if which & 32 != 0 && !ensure(&mut s.behaviors, count) {
            return 11;
        }
        if which & 128 != 0 && !ensure(&mut s.vmodmap, count) {
            return 11;
        }
        0
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XkbAllocNames")]
pub unsafe extern "sysv64" fn XkbAllocNames(
    x: *mut XkbDescRec,
    which: c_uint,
    groups: c_int,
    aliases: c_int,
) -> Status {
    if x.is_null() {
        return 8;
    }
    if !(0..=65535).contains(&groups) || !(0..=255).contains(&aliases) {
        return 2;
    }
    unsafe {
        let x = &mut *x;
        if !ensure(&mut x.names, mem::size_of::<Names>()) {
            return 11;
        }
        let n = &mut *x.names.cast::<Names>();
        if which & (1 << 7) != 0 && !x.map.is_null() {
            let m = &mut *x.map.cast::<ClientMap>();
            for i in 0..m.num_types as usize {
                let t = &mut *m.types.add(i);
                if t.num_levels != 0 && !ensure(&mut t.level_names, t.num_levels as usize) {
                    return 11;
                }
            }
        }
        if which & (1 << 9) != 0 {
            if !valid_keys(x) {
                return 2;
            }
            if !ensure(&mut n.keys, x.max_key_code as usize + 1) {
                return 11;
            }
        }
        if which & (1 << 10) != 0 && aliases > 0 {
            if !grow(
                &mut n.key_aliases,
                n.num_key_aliases as usize,
                aliases as usize,
            ) {
                return 11;
            }
            n.num_key_aliases = aliases as u8;
        }
        if which & (1 << 13) != 0 && groups > 0 {
            if !grow(&mut n.radio_groups, n.num_rg as usize, groups as usize) {
                return 11;
            }
            n.num_rg = groups as u16;
        }
        0
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XkbAllocCompatMap")]
pub unsafe extern "sysv64" fn XkbAllocCompatMap(
    x: *mut XkbDescRec,
    _which: c_uint,
    count: c_uint,
) -> Status {
    if x.is_null() {
        return 8;
    }
    if count > 65535 {
        return 2;
    }
    unsafe {
        if !ensure(&mut (*x).compat, mem::size_of::<CompatMap>()) {
            return 11;
        }
        let c = &mut *(*x).compat.cast::<CompatMap>();
        if !grow(&mut c.sym_interpret, c.size_si as usize, count as usize) {
            return 11;
        }
        c.size_si = c.size_si.max(count as u16);
        0
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XkbAllocControls")]
pub unsafe extern "sysv64" fn XkbAllocControls(x: *mut XkbDescRec, _which: c_uint) -> Status {
    if x.is_null() {
        return 8;
    }
    if unsafe { ensure(&mut (*x).ctrls, mem::size_of::<Controls>()) } {
        0
    } else {
        11
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XkbAllocIndicatorMaps")]
pub unsafe extern "sysv64" fn XkbAllocIndicatorMaps(x: *mut XkbDescRec) -> Status {
    if x.is_null() {
        return 8;
    }
    if unsafe { ensure(&mut (*x).indicators, mem::size_of::<Indicators>()) } {
        0
    } else {
        11
    }
}

pub unsafe fn free_client_map(x: *mut XkbDescRec, which: c_uint, all: Bool) {
    if x.is_null() || unsafe { (*x).map.is_null() } {
        return;
    }
    unsafe {
        let m = &mut *(*x).map.cast::<ClientMap>();
        let which = if all != 0 { 7 } else { which };
        if which & 1 != 0 {
            for i in 0..m.num_types as usize {
                let t = &mut *m.types.add(i);
                release(&mut t.map);
                release(&mut t.preserve);
                release(&mut t.level_names);
            }
            release(&mut m.types);
            m.num_types = 0;
            m.size_types = 0;
        }
        if which & 2 != 0 {
            release(&mut m.syms);
            release(&mut m.key_sym_map);
            m.num_syms = 0;
            m.size_syms = 0;
        }
        if which & 4 != 0 {
            release(&mut m.modmap);
        }
        if all != 0 {
            release(&mut (*x).map);
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XkbFreeServerMap")]
pub unsafe extern "sysv64" fn XkbFreeServerMap(x: *mut XkbDescRec, which: c_uint, all: Bool) {
    if x.is_null() || unsafe { (*x).server.is_null() } {
        return;
    }
    unsafe {
        let s = &mut *(*x).server.cast::<ServerMap>();
        let which = if all != 0 { 0xf8 } else { which };
        if which & 8 != 0 {
            release(&mut s.explicit);
        }
        if which & 16 != 0 {
            release(&mut s.acts);
            release(&mut s.key_acts);
            s.num_acts = 0;
            s.size_acts = 0;
        }
        if which & 32 != 0 {
            release(&mut s.behaviors);
        }
        if which & 64 != 0 {
            s.vmods.fill(0);
        }
        if which & 128 != 0 {
            release(&mut s.vmodmap);
        }
        if all != 0 {
            release(&mut (*x).server);
        }
    }
}
pub unsafe fn free_names(x: *mut XkbDescRec, which: c_uint, all: Bool) {
    if x.is_null() || unsafe { (*x).names.is_null() } {
        return;
    }
    unsafe {
        let n = &mut *(*x).names.cast::<Names>();
        let which = if all != 0 { 0x3fff } else { which };
        if which & (1 << 7) != 0 && !(*x).map.is_null() {
            let m = &mut *(*x).map.cast::<ClientMap>();
            for i in 0..m.num_types as usize {
                release(&mut (*m.types.add(i)).level_names);
            }
        }
        if which & (1 << 9) != 0 {
            release(&mut n.keys);
            n.num_keys = 0;
        }
        if which & (1 << 10) != 0 {
            release(&mut n.key_aliases);
            n.num_key_aliases = 0;
        }
        if which & (1 << 13) != 0 {
            release(&mut n.radio_groups);
            n.num_rg = 0;
        }
        if all != 0 {
            release(&mut (*x).names);
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XkbFreeCompatMap")]
pub unsafe extern "sysv64" fn XkbFreeCompatMap(x: *mut XkbDescRec, which: c_uint, all: Bool) {
    if x.is_null() || unsafe { (*x).compat.is_null() } {
        return;
    }
    unsafe {
        let c = &mut *(*x).compat.cast::<CompatMap>();
        if all != 0 || which & 1 != 0 {
            release(&mut c.sym_interpret);
            c.num_si = 0;
            c.size_si = 0;
        }
        if all != 0 || which & 2 != 0 {
            c.groups.fill(Mods::default());
        }
        if all != 0 {
            release(&mut (*x).compat);
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XkbFreeControls")]
pub unsafe extern "sysv64" fn XkbFreeControls(x: *mut XkbDescRec, _which: c_uint, all: Bool) {
    if !x.is_null() && all != 0 {
        unsafe { release(&mut (*x).ctrls) };
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XkbFreeIndicatorMaps")]
pub unsafe extern "sysv64" fn XkbFreeIndicatorMaps(x: *mut XkbDescRec) {
    if !x.is_null() {
        unsafe { release(&mut (*x).indicators) };
    }
}
pub unsafe fn free_keyboard(x: *mut XkbDescRec, which: c_uint, all: Bool) {
    if x.is_null() {
        return;
    }
    unsafe {
        let which = if all != 0 { 127 } else { which };
        if which & 1 != 0 {
            free_client_map(x, 7, 1);
        }
        if which & 2 != 0 {
            XkbFreeServerMap(x, 0xf8, 1);
        }
        if which & 4 != 0 {
            XkbFreeCompatMap(x, 3, 1);
        }
        if which & 8 != 0 {
            XkbFreeIndicatorMaps(x);
        }
        if which & 16 != 0 {
            free_names(x, 0x3fff, 1);
        }
        if which & 32 != 0 {
            super::geometry::XkbFreeGeometry((*x).geom.cast(), 0x3f, 1);
            (*x).geom = ptr::null_mut();
        }
        if which & 64 != 0 {
            XkbFreeControls(x, !0, 1);
        }
        if all != 0 {
            guest::free(x.cast());
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbVirtualModsToReal")]
pub unsafe extern "sysv64" fn XkbVirtualModsToReal(
    x: *mut XkbDescRec,
    virtual_mask: c_uint,
    result: *mut c_uint,
) -> Bool {
    if x.is_null() || result.is_null() {
        return 0;
    }
    if virtual_mask == 0 {
        unsafe {
            *result = 0;
        }
        return 1;
    }
    if unsafe { (*x).server.is_null() } {
        return 0;
    }
    unsafe {
        *result = 0;
        let s = &*(*x).server.cast::<ServerMap>();
        for i in 0..16 {
            if virtual_mask & (1 << i) != 0 {
                *result |= s.vmods[i] as c_uint;
            }
        }
    }
    1
}

// Each resize gets a new contiguous span, preserving existing values. Offsets
// are 16-bit in the public ABI, so capacity/overflow checks precede mutation.
#[unsafe(export_name = "kinakaze_engine_libX11_XkbResizeKeySyms")]
pub unsafe extern "sysv64" fn XkbResizeKeySyms(
    x: *mut XkbDescRec,
    key: c_int,
    needed: c_int,
) -> *mut usize {
    if x.is_null() || needed < 0 || unsafe { (*x).map.is_null() } {
        return ptr::null_mut();
    }
    unsafe {
        if key < (*x).min_key_code as c_int || key > (*x).max_key_code as c_int {
            return ptr::null_mut();
        }
        let m = &mut *(*x).map.cast::<ClientMap>();
        if m.key_sym_map.is_null() || m.syms.is_null() {
            return ptr::null_mut();
        }
        let k = &mut *m.key_sym_map.add(key as usize);
        let old = k.width as usize * (k.group_info & 15) as usize;
        if needed == 0 {
            k.offset = 0;
            return m.syms;
        }
        if old >= needed as usize {
            return m.syms.add(k.offset as usize);
        }
        let total = m.num_syms as usize + needed as usize;
        if total > u16::MAX as usize {
            return ptr::null_mut();
        }
        if !grow(&mut m.syms, m.size_syms as usize, total) {
            return ptr::null_mut();
        }
        m.size_syms = m.size_syms.max(total as u16);
        let out = m.syms.add(m.num_syms as usize);
        ptr::write_bytes(out, 0, needed as usize);
        ptr::copy(m.syms.add(k.offset as usize), out, old);
        k.offset = m.num_syms;
        m.num_syms = total as u16;
        out
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XkbResizeKeyActions")]
pub unsafe extern "sysv64" fn XkbResizeKeyActions(
    x: *mut XkbDescRec,
    key: c_int,
    needed: c_int,
) -> *mut [u8; 8] {
    if x.is_null() || needed < 0 || unsafe { (*x).server.is_null() || (*x).map.is_null() } {
        return ptr::null_mut();
    }
    unsafe {
        if key < (*x).min_key_code as c_int || key > (*x).max_key_code as c_int {
            return ptr::null_mut();
        }
        let s = &mut *(*x).server.cast::<ServerMap>();
        let m = &*(*x).map.cast::<ClientMap>();
        if s.key_acts.is_null() || m.key_sym_map.is_null() {
            return ptr::null_mut();
        }
        let offset = &mut *s.key_acts.add(key as usize);
        if needed == 0 {
            *offset = 0;
            return ptr::null_mut();
        }
        let k = &*m.key_sym_map.add(key as usize);
        let old = if *offset == 0 {
            0
        } else {
            k.width as usize * (k.group_info & 15) as usize
        };
        if old >= needed as usize {
            return s.acts.add(*offset as usize);
        }
        let first = s.num_acts.max(1) as usize;
        let total = first + needed as usize;
        if total > u16::MAX as usize || !grow(&mut s.acts, s.size_acts as usize, total) {
            return ptr::null_mut();
        }
        s.size_acts = s.size_acts.max(total as u16);
        let out = s.acts.add(first);
        ptr::write_bytes(out, 0, needed as usize);
        if old > 0 {
            ptr::copy(s.acts.add(*offset as usize), out, old);
        }
        *offset = first as u16;
        s.num_acts = total as u16;
        out
    }
}

#[repr(C)]
pub struct MapChanges {
    changed: u16,
    min_key_code: u8,
    max_key_code: u8,
    first_type: u8,
    num_types: u8,
    first_key_sym: u8,
    num_key_syms: u8,
    first_key_act: u8,
    num_key_acts: u8,
    first_key_behavior: u8,
    num_key_behaviors: u8,
    first_key_explicit: u8,
    num_key_explicit: u8,
    first_modmap_key: u8,
    num_modmap_keys: u8,
    first_vmodmap_key: u8,
    num_vmodmap_keys: u8,
    pad: u8,
    vmods: u16,
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbChangeTypesOfKey")]
pub unsafe extern "sysv64" fn XkbChangeTypesOfKey(
    x: *mut XkbDescRec,
    key: c_int,
    groups: c_int,
    changed_groups: c_uint,
    new_types: *const c_int,
    changes: *mut MapChanges,
) -> Status {
    if x.is_null() || !(0..=4).contains(&groups) || changed_groups & 15 == 0 {
        return 8;
    }
    unsafe {
        if key < (*x).min_key_code as c_int
            || key > (*x).max_key_code as c_int
            || (*x).map.is_null()
        {
            return 8;
        }
        let m = &mut *(*x).map.cast::<ClientMap>();
        if m.types.is_null() || m.key_sym_map.is_null() {
            return 8;
        }
        let previous = *m.key_sym_map.add(key as usize);
        let old_groups = (previous.group_info & 15) as usize;
        let old_width = previous.width as usize;
        let mut types = [0u8; 4];
        let mut width = 0usize;
        for i in 0..groups as usize {
            let t = if changed_groups & (1 << i) != 0 {
                if new_types.is_null() {
                    return 8;
                }
                *new_types.add(i)
            } else if i < old_groups {
                previous.types[i] as c_int
            } else if old_groups > 0 {
                previous.types[0] as c_int
            } else {
                1
            };
            if t < 0 || t >= m.num_types as c_int {
                return 8;
            }
            types[i] = t as u8;
            width = width.max((*m.types.add(t as usize)).num_levels as usize);
        }
        if old_groups > 4 || (old_width * old_groups != 0 && m.syms.is_null()) {
            return 8;
        }
        let mut copy_widths = [0usize; 4];
        for i in 0..old_groups.min(groups as usize) {
            if previous.types[i] >= m.num_types {
                return 8;
            }
            copy_widths[i] = old_width
                .min((*m.types.add(previous.types[i] as usize)).num_levels as usize)
                .min((*m.types.add(types[i] as usize)).num_levels as usize);
        }
        let old_syms = if old_width * old_groups == 0 {
            Vec::new()
        } else {
            core::slice::from_raw_parts(
                m.syms.add(previous.offset as usize),
                old_width * old_groups,
            )
            .to_vec()
        };
        let mut old_actions = Vec::new();
        let mut action_target = ptr::null_mut();
        if !(*x).server.is_null() {
            let s = &mut *(*x).server.cast::<ServerMap>();
            if !s.key_acts.is_null() && *s.key_acts.add(key as usize) != 0 {
                old_actions = core::slice::from_raw_parts(
                    s.acts.add(*s.key_acts.add(key as usize) as usize),
                    old_width * old_groups,
                )
                .to_vec();
                action_target = XkbResizeKeyActions(x, key, (width * groups as usize) as c_int);
                if groups != 0 && width != 0 && action_target.is_null() {
                    return 11;
                }
            }
        }
        let target = XkbResizeKeySyms(x, key, (width * groups as usize) as c_int);
        if target.is_null() {
            return 11;
        }
        ptr::write_bytes(target, 0, width * groups as usize);
        if !action_target.is_null() {
            ptr::write_bytes(action_target, 0, width * groups as usize);
        }
        for i in 0..old_groups.min(groups as usize) {
            ptr::copy_nonoverlapping(
                old_syms.as_ptr().add(i * old_width),
                target.add(i * width),
                copy_widths[i],
            );
            if !action_target.is_null() {
                ptr::copy_nonoverlapping(
                    old_actions.as_ptr().add(i * old_width),
                    action_target.add(i * width),
                    copy_widths[i],
                );
            }
        }
        let k = &mut *m.key_sym_map.add(key as usize);
        k.types = types;
        k.width = width as u8;
        k.group_info = (previous.group_info & 0xf0) | groups as u8;
        if !(*x).ctrls.is_null() {
            let c = &mut *(*x).ctrls.cast::<Controls>();
            c.num_groups = c.num_groups.max(groups as u8);
        }
        if !changes.is_null() {
            let c = &mut *changes;
            let end = if c.changed & 2 != 0 {
                (c.first_key_sym as usize + c.num_key_syms as usize).max(key as usize + 1)
            } else {
                key as usize + 1
            };
            c.first_key_sym = if c.changed & 2 != 0 {
                c.first_key_sym.min(key as u8)
            } else {
                key as u8
            };
            c.num_key_syms = (end - c.first_key_sym as usize) as u8;
            c.changed |= 2;
        }
        0
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbTranslateKeyCode")]
pub unsafe extern "sysv64" fn XkbTranslateKeyCode(
    x: *mut XkbDescRec,
    key: u8,
    modifiers: c_uint,
    consumed: *mut c_uint,
    result: *mut usize,
) -> Bool {
    unsafe {
        if !result.is_null() {
            *result = 0;
        }
        if !consumed.is_null() {
            *consumed = 0;
        }
        if x.is_null()
            || result.is_null()
            || key < (*x).min_key_code
            || key > (*x).max_key_code
            || (*x).map.is_null()
        {
            return 0;
        }
        let m = &*(*x).map.cast::<ClientMap>();
        if m.key_sym_map.is_null() || m.types.is_null() || m.syms.is_null() {
            return 0;
        }
        let k = &*m.key_sym_map.add(key as usize);
        let count = (k.group_info & 15) as usize;
        if count == 0 || count > 4 {
            return 0;
        }
        let mut group = ((modifiers >> 13) & 3) as usize;
        if group >= count {
            group = match k.group_info & 0xc0 {
                0x40 => count - 1,
                0x80 => {
                    let g = ((k.group_info >> 4) & 3) as usize;
                    if g < count { g } else { 0 }
                }
                _ => group % count,
            };
        }
        if k.types[group] >= m.num_types {
            return 0;
        }
        let t = &*m.types.add(k.types[group] as usize);
        let mut level = 0usize;
        let mut preserved = 0u8;
        for i in 0..t.map_count as usize {
            let e = &*t.map.add(i);
            if e.active != 0 && modifiers & t.mods.mask as c_uint == e.mods.mask as c_uint {
                level = e.level as usize;
                if !t.preserve.is_null() {
                    preserved = (*t.preserve.add(i)).mask;
                }
                break;
            }
        }
        if level >= k.width as usize {
            return 0;
        }
        let at = k.offset as usize + group * k.width as usize + level;
        if at >= m.num_syms as usize {
            return 0;
        }
        *result = *m.syms.add(at);
        if !consumed.is_null() {
            *consumed = (t.mods.mask & !preserved) as c_uint;
        }
        (*result != 0) as Bool
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changing_group_width_preserves_symbols_and_translation_consumes_modifiers() {
        unsafe {
            let x = XkbAllocKeyboard();
            (*x).min_key_code = 8;
            (*x).max_key_code = 255;
            let mut consumed = 123;
            assert_eq!(XkbVirtualModsToReal(x, 0, &raw mut consumed), 1);
            assert_eq!(consumed, 0);
            assert_eq!(XkbAllocClientMap(x, 3, 4), 0);
            let m = &mut *(*x).map.cast::<ClientMap>();
            m.num_types = 4;
            (*m.types).num_levels = 1;
            let shifted = &mut *m.types.add(1);
            shifted.num_levels = 2;
            shifted.mods.mask = 1;
            shifted.map_count = 1;
            shifted.map = alloc(1);
            shifted.map.write(TypeEntry {
                active: 1,
                level: 1,
                mods: Mods {
                    mask: 1,
                    ..Mods::default()
                },
            });
            let syms = XkbResizeKeySyms(x, 10, 2);
            *syms = 0x61;
            *syms.add(1) = 0x62;
            let k = &mut *m.key_sym_map.add(10);
            k.width = 1;
            k.group_info = 2;
            let mut changes: MapChanges = mem::zeroed();
            assert_eq!(
                XkbChangeTypesOfKey(x, 10, 2, 3, [1, 1].as_ptr(), &raw mut changes),
                0
            );
            let k = &*m.key_sym_map.add(10);
            assert_eq!(k.width, 2);
            let syms = m.syms.add(k.offset as usize);
            assert_eq!(core::slice::from_raw_parts(syms, 4), &[0x61, 0, 0x62, 0]);
            *syms.add(3) = 0x42;
            let mut result = 0;
            assert_eq!(
                XkbTranslateKeyCode(x, 10, (1 << 13) | 1, &raw mut consumed, &raw mut result),
                1
            );
            assert_eq!((result, consumed), (0x42, 1));
            assert_eq!(
                (changes.changed, changes.first_key_sym, changes.num_key_syms),
                (2, 10, 1)
            );
            free_keyboard(x, 0, 1);
        }
    }
    #[test]
    fn records_match_linux_x86_64_xkb_headers() {
        assert_eq!(mem::size_of::<XkbDescRec>(), 72);
        assert_eq!(mem::size_of::<TypeEntry>(), 12);
        assert_eq!(mem::size_of::<KeyType>(), 40);
        assert_eq!(mem::size_of::<ClientMap>(), 48);
        assert_eq!(mem::size_of::<ServerMap>(), 64);
        assert_eq!(mem::size_of::<Names>(), 496);
        assert_eq!(mem::size_of::<Controls>(), 84);
        assert_eq!(mem::size_of::<Indicators>(), 392);
        assert_eq!(mem::size_of::<CompatMap>(), 32);
        assert_eq!(mem::size_of::<MapChanges>(), 22);
    }
    #[test]
    fn maps_preserve_other_keys_and_release_selected_components() {
        unsafe {
            let x = XkbAllocKeyboard();
            assert!(!x.is_null());
            (*x).min_key_code = 8;
            (*x).max_key_code = 255;
            assert_eq!(XkbAllocClientMap(x, 7, 4), 0);
            assert_eq!(XkbAllocServerMap(x, 0xf8, 4), 0);
            assert_eq!(XkbAllocNames(x, (1 << 9) | (1 << 10) | (1 << 13), 3, 2), 0);
            assert_eq!(XkbAllocControls(x, !0), 0);
            assert_eq!(XkbAllocCompatMap(x, 3, 2), 0);
            assert_eq!(XkbAllocIndicatorMaps(x), 0);
            let m = &mut *(*x).map.cast::<ClientMap>();
            *XkbResizeKeySyms(x, 10, 1) = 0x61;
            (*m.key_sym_map.add(10)).width = 1;
            (*m.key_sym_map.add(10)).group_info = 1;
            *XkbResizeKeySyms(x, 11, 1) = 0x62;
            (*m.key_sym_map.add(11)).width = 1;
            (*m.key_sym_map.add(11)).group_info = 1;
            let enlarged = XkbResizeKeySyms(x, 10, 500);
            assert!(!enlarged.is_null());
            assert_eq!(*enlarged, 0x61);
            assert_eq!(*enlarged.add(499), 0);
            assert_eq!(*m.syms.add((*m.key_sym_map.add(11)).offset as usize), 0x62);
            let old_offset = (*m.key_sym_map.add(10)).offset;
            assert!(XkbResizeKeySyms(x, 10, 65535).is_null());
            assert_eq!((*m.key_sym_map.add(10)).offset, old_offset);
            let s = &mut *(*x).server.cast::<ServerMap>();
            s.vmods[3] = 4;
            let mut mods = 0;
            assert_eq!(XkbVirtualModsToReal(x, 1 << 3, &raw mut mods), 1);
            assert_eq!(mods, 4);
            free_keyboard(x, 64, 0);
            assert!((*x).ctrls.is_null());
            assert!(!(*x).map.is_null());
            free_keyboard(x, 0, 1);
        }
    }
}
