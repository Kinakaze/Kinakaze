//! Xlib views of the same native keyboard map returned over the XCB protocol.
use super::{data::*, server, *};
use kinakaze_alloc::guest;

pub unsafe fn load_map(display: *mut Display, x: *mut XkbDescRec, which: u32) -> i32 {
    if x.is_null() || which & !255 != 0 {
        return 2;
    }
    unsafe {
        (*x).dpy = display;
        (*x).device_spec = server::DEVICE as u16;
        (*x).min_key_code = server::MIN_KEY;
        (*x).max_key_code = server::MAX_KEY;
        let status = XkbAllocClientMap(x, which & 7, 4);
        if status != 0 {
            return status;
        }
        let m = &mut *(*x).map.cast::<ClientMap>();
        if which & 1 != 0 {
            for i in 0..4 {
                let t = &mut *m.types.add(i);
                let entries = server::type_entries(i as u8);
                let storage =
                    guest::malloc(entries.len() * size_of::<TypeEntry>()).cast::<TypeEntry>();
                if !entries.is_empty() && storage.is_null() {
                    return 11;
                }
                guest::free(t.map.cast());
                guest::free(t.preserve.cast());
                t.preserve = core::ptr::null_mut();
                guest::free(t.level_names.cast());
                t.level_names = core::ptr::null_mut();
                t.map = storage;
                t.num_levels = server::TYPE_LEVELS[i];
                t.map_count = entries.len() as u8;
                let mask = server::type_mask(i as u8);
                t.mods = Mods {
                    mask,
                    real_mods: mask,
                    vmods: 0,
                };
                t.name = server::atom(server::TYPE_NAMES[i]) as usize;
                for (j, &(mods, level)) in entries.iter().enumerate() {
                    storage.add(j).write(TypeEntry {
                        active: 1,
                        level,
                        mods: Mods {
                            mask: mods,
                            real_mods: mods,
                            vmods: 0,
                        },
                    });
                }
            }
            m.num_types = 4;
        }
        if which & 2 != 0 {
            let syms = guest::malloc(513 * size_of::<usize>()).cast::<usize>();
            if syms.is_null() {
                return 11;
            }
            core::ptr::write_bytes(syms, 0, 513);
            guest::free(m.syms.cast());
            m.syms = syms;
            m.size_syms = 513;
            m.num_syms = 1;
            for k in 8..=255 {
                let typ = server::key_type(k as u8);
                let width = server::TYPE_LEVELS[typ as usize];
                let out = m.syms.add(m.num_syms as usize);
                for (i, sym) in server::symbols(k as u8)[..width as usize]
                    .iter()
                    .enumerate()
                {
                    out.add(i).write(*sym as usize);
                }
                let key = &mut *m.key_sym_map.add(k as usize);
                key.offset = m.num_syms;
                key.width = width;
                key.group_info = 1;
                key.types = [typ, 0, 0, 0];
                m.num_syms += width as u16;
            }
        }
        if which & 4 != 0 {
            for k in 8..=255 {
                m.modmap.add(k).write(server::modifier(k as u8));
            }
        }
        if which & !7 != 0 {
            let status = XkbAllocServerMap(x, which & !7, 32);
            if status != 0 {
                return status;
            }
            if which & 16 != 0 && !m.key_sym_map.is_null() {
                for k in 8..=255 {
                    let mask = server::modifier(k as u8);
                    if mask == 0 {
                        continue;
                    }
                    let a = XkbResizeKeyActions(x, k, 1);
                    if a.is_null() {
                        return 11;
                    }
                    a.write([
                        if mask & 0x32 != 0 { 3 } else { 1 },
                        0,
                        mask,
                        mask,
                        0,
                        0,
                        0,
                        0,
                    ]);
                }
            }
        }
    }
    0
}
pub unsafe fn get_map(display: *mut Display, which: u32, device: u32) -> *mut XkbDescRec {
    if device > 65535 || !server::valid_device(device as u16) {
        return core::ptr::null_mut();
    }
    unsafe {
        let x = XkbAllocKeyboard();
        if x.is_null() {
            return x;
        }
        if load_map(display, x, which) != 0 {
            free_keyboard(x, 255, 1);
            return core::ptr::null_mut();
        }
        x
    }
}
pub unsafe fn names(which: u32, x: *mut XkbDescRec) -> i32 {
    if x.is_null() || which & !0x3fff != 0 {
        return 2;
    }
    unsafe {
        let status = XkbAllocNames(x, which, 0, 0);
        if status != 0 {
            return status;
        }
        let n = &mut *(*x).names.cast::<Names>();
        if which & 1 != 0 {
            n.keycodes = server::atom("evdev") as usize;
        }
        if which & 2 != 0 {
            n.geometry = server::atom("native") as usize;
        }
        if which & 4 != 0 {
            n.symbols = server::atom("us") as usize;
        }
        if which & 8 != 0 {
            n.phys_symbols = server::atom("us") as usize;
        }
        if which & 16 != 0 {
            n.types = server::atom("complete") as usize;
        }
        if which & 32 != 0 {
            n.compat = server::atom("complete") as usize;
        }
        if which & 0xc0 != 0 && !(*x).map.is_null() {
            let m = &mut *(*x).map.cast::<ClientMap>();
            for i in 0..m.num_types.min(4) as usize {
                let t = &mut *m.types.add(i);
                if which & 0x40 != 0 {
                    t.name = server::atom(server::TYPE_NAMES[i]) as usize;
                }
                if which & 0x80 != 0 && !t.level_names.is_null() {
                    for l in 0..t.num_levels as usize {
                        t.level_names.add(l).write(server::atom(if l == 0 {
                            "Base"
                        } else {
                            "Shift"
                        }) as usize);
                    }
                }
            }
        }
        if which & 0x100 != 0 {
            for (i, name) in ["Caps Lock", "Num Lock", "Scroll Lock"].iter().enumerate() {
                n.indicators[i] = server::atom(name) as usize;
            }
        }
        if which & 0x200 != 0 {
            for k in (*x).min_key_code..=(*x).max_key_code {
                n.keys.add(k as usize).write(server::key_name(k));
            }
            n.num_keys = (*x).max_key_code - (*x).min_key_code + 1;
        }
        if which & 0x1000 != 0 {
            n.groups[0] = server::atom("English (US)") as usize;
        }
    }
    0
}
