//! Client-owned XKB geometry. All exposed pointers use the guest allocator.
use super::*;
use core::ffi::CStr;
use core::{mem, ptr};
use kinakaze_alloc::guest;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn geometry_growth_keeps_owned_strings_and_internal_pointers() {
        unsafe {
            assert_eq!(mem::size_of::<Geometry>(), 112);
            assert_eq!(mem::size_of::<Section>(), 64);
            assert_eq!(mem::size_of::<Row>(), 32);
            assert_eq!(mem::size_of::<Shape>(), 48);
            assert_eq!(mem::size_of::<Doodad>(), 40);
            assert_eq!(mem::size_of::<Overlay>(), 40);
            let x = super::super::data::XkbAllocKeyboard();
            let sizes = Sizes {
                which: 0x3f,
                properties: 1,
                colors: 1,
                shapes: 1,
                sections: 1,
                doodads: 1,
                key_aliases: 1,
            };
            assert_eq!(XkbAllocGeometry(x, &sizes), 0);
            let g = (*x).geom.cast::<Geometry>();
            let c = XkbAddGeomColor(g, c"black".as_ptr(), 0);
            (*g).label_color = c;
            (*g).base_color = c;
            assert!(!XkbAddGeomColor(g, c"white".as_ptr(), 0xffffff).is_null());
            assert_eq!((*g).label_color, (*g).colors);
            assert_eq!(CStr::from_ptr((*(*g).base_color).spec), c"black");
            let shape = XkbAddGeomShape(g, 7, 1);
            let outline = XkbAddGeomOutline(shape, 4);
            (*shape).primary = outline;
            (*shape).approx = outline;
            assert_eq!(XkbAddGeomShape(g, 7, 400), shape);
            assert_eq!((*shape).primary, (*shape).outlines);
            assert_eq!((*(*shape).primary).sz_points, 4);
            let section = XkbAddGeomSection(g, 8, 1, 0, 1);
            let row = XkbAddGeomRow(section, 2);
            assert!(!XkbAddGeomKey(row).is_null());
            let overlay = XkbAddGeomOverlay(section, 9, 1);
            assert!(!XkbAddGeomOverlayRow(overlay, 0, 2).is_null());
            assert!(!XkbAddGeomSection(g, 10, 0, 0, 0).is_null());
            assert_eq!((*overlay).section_under, (*g).sections);
            assert_eq!(
                XkbAllocGeometry(
                    x,
                    &Sizes {
                        colors: 400,
                        sections: 400,
                        ..sizes
                    }
                ),
                0
            );
            assert_eq!((*g).label_color, (*g).colors);
            assert_eq!((*overlay).section_under, (*g).sections);
            super::super::data::free_keyboard(x, 32, 0);
            assert!((*x).geom.is_null());
            super::super::data::free_keyboard(x, 0, 1);
        }
    }
}

macro_rules! record {
    ($name:ident { $($field:ident: $ty:ty),* $(,)? }) => {
        #[repr(C)]
        pub struct $name { $(pub $field: $ty),* }
    };
}
record!(Property { name: *mut c_char, value: *mut c_char });
record!(Color { pixel: u32, spec: *mut c_char });
record!(Point { x: i16, y: i16 });
record!(Bounds {
    x1: i16,
    y1: i16,
    x2: i16,
    y2: i16
});
record!(Outline { num_points: u16, sz_points: u16, corner_radius: u16, points: *mut Point });
record!(Shape { name: usize, num_outlines: u16, sz_outlines: u16, outlines: *mut Outline,
    approx: *mut Outline, primary: *mut Outline, bounds: Bounds });
record!(Key {
    name: [u8; 4],
    gap: i16,
    shape_ndx: u8,
    color_ndx: u8
});
record!(Row { top: i16, left: i16, num_keys: u16, sz_keys: u16, vertical: c_int,
    keys: *mut Key, bounds: Bounds });
record!(Doodad {
    name: usize,
    kind: u8,
    priority: u8,
    top: i16,
    left: i16,
    angle: i16,
    data: [u64; 3]
});
record!(Section { name: usize, priority: u8, top: i16, left: i16, width: u16, height: u16,
    angle: i16, num_rows: u16, num_doodads: u16, num_overlays: u16, sz_rows: u16,
    sz_doodads: u16, sz_overlays: u16, rows: *mut Row, doodads: *mut Doodad,
    bounds: Bounds, overlays: *mut Overlay });
record!(OverlayKey {
    over: [u8; 4],
    under: [u8; 4]
});
record!(OverlayRow { row_under: u16, num_keys: u16, sz_keys: u16, keys: *mut OverlayKey });
record!(Overlay { name: usize, section_under: *mut Section, num_rows: u16, sz_rows: u16,
    rows: *mut OverlayRow, bounds: *mut Bounds });
record!(Geometry { name: usize, width_mm: u16, height_mm: u16, label_font: *mut c_char,
    label_color: *mut Color, base_color: *mut Color,
    sz_properties: u16, sz_colors: u16, sz_shapes: u16, sz_sections: u16, sz_doodads: u16, sz_key_aliases: u16,
    num_properties: u16, num_colors: u16, num_shapes: u16, num_sections: u16, num_doodads: u16, num_key_aliases: u16,
    properties: *mut Property, colors: *mut Color, shapes: *mut Shape, sections: *mut Section,
    doodads: *mut Doodad, key_aliases: *mut [u8; 8] });
record!(Sizes {
    which: u32,
    properties: u16,
    colors: u16,
    shapes: u16,
    sections: u16,
    doodads: u16,
    key_aliases: u16
});

unsafe fn reserve<T>(data: &mut *mut T, capacity: &mut u16, needed: usize) -> bool {
    if needed <= usize::from(*capacity) {
        return true;
    }
    if needed > u16::MAX as usize {
        return false;
    }
    let out =
        unsafe { guest::reallocate((*data).cast(), 16, needed * mem::size_of::<T>()) }.cast::<T>();
    if out.is_null() {
        return false;
    }
    unsafe {
        ptr::write_bytes(
            out.add(usize::from(*capacity)),
            0,
            needed - usize::from(*capacity),
        );
    }
    *data = out;
    *capacity = needed as u16;
    true
}
unsafe fn append<T>(data: &mut *mut T, capacity: &mut u16, count: &mut u16) -> *mut T {
    if !unsafe { reserve(data, capacity, usize::from(*count) + 1) } {
        return ptr::null_mut();
    }
    let out = unsafe { (*data).add(usize::from(*count)) };
    *count += 1;
    out
}
unsafe fn string(value: *const c_char) -> *mut c_char {
    if value.is_null() {
        return ptr::null_mut();
    }
    let bytes = unsafe { CStr::from_ptr(value) }.to_bytes_with_nul();
    let out = unsafe { guest::malloc(bytes.len()) }.cast::<c_char>();
    if !out.is_null() {
        unsafe {
            ptr::copy_nonoverlapping(value, out, bytes.len());
        }
    }
    out
}
fn index<T>(base: *mut T, value: *mut T, count: u16) -> Option<usize> {
    if base.is_null() || value.is_null() {
        return None;
    }
    (value as usize)
        .checked_sub(base as usize)
        .filter(|offset| *offset < usize::from(count) * mem::size_of::<T>())
        .map(|offset| offset / mem::size_of::<T>())
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbAddGeomColor")]
pub unsafe extern "sysv64" fn XkbAddGeomColor(
    g: *mut Geometry,
    spec: *const c_char,
    pixel: u32,
) -> *mut Color {
    if g.is_null() || spec.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        let g = &mut *g;
        for i in 0..usize::from(g.num_colors) {
            let color = g.colors.add(i);
            if !(*color).spec.is_null() && CStr::from_ptr((*color).spec) == CStr::from_ptr(spec) {
                return color;
            }
        }
        let text = string(spec);
        if text.is_null() {
            return ptr::null_mut();
        }
        let label = index(g.colors, g.label_color, g.num_colors);
        let base = index(g.colors, g.base_color, g.num_colors);
        let out = append(&mut g.colors, &mut g.sz_colors, &mut g.num_colors);
        if out.is_null() {
            guest::free(text.cast());
            return out;
        }
        if let Some(i) = label {
            g.label_color = g.colors.add(i);
        }
        if let Some(i) = base {
            g.base_color = g.colors.add(i);
        }
        out.write(Color { pixel, spec: text });
        out
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbAddGeomProperty")]
pub unsafe extern "sysv64" fn XkbAddGeomProperty(
    g: *mut Geometry,
    name: *const c_char,
    value: *const c_char,
) -> *mut Property {
    if g.is_null() || name.is_null() || value.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        let g = &mut *g;
        let text = string(value);
        if text.is_null() {
            return ptr::null_mut();
        }
        for i in 0..usize::from(g.num_properties) {
            let entry = g.properties.add(i);
            if !(*entry).name.is_null() && CStr::from_ptr((*entry).name) == CStr::from_ptr(name) {
                guest::free((*entry).value.cast());
                (*entry).value = text;
                return entry;
            }
        }
        let key = string(name);
        if key.is_null() {
            guest::free(text.cast());
            return ptr::null_mut();
        }
        let out = append(
            &mut g.properties,
            &mut g.sz_properties,
            &mut g.num_properties,
        );
        if out.is_null() {
            guest::free(key.cast());
            guest::free(text.cast());
            return out;
        }
        out.write(Property {
            name: key,
            value: text,
        });
        out
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbAddGeomOutline")]
pub unsafe extern "sysv64" fn XkbAddGeomOutline(shape: *mut Shape, points: c_int) -> *mut Outline {
    if shape.is_null() || !(0..=65535).contains(&points) {
        return ptr::null_mut();
    }
    unsafe {
        let shape = &mut *shape;
        let mut value: Outline = mem::zeroed();
        if !reserve(&mut value.points, &mut value.sz_points, points as usize) {
            return ptr::null_mut();
        }
        let approx = index(shape.outlines, shape.approx, shape.num_outlines);
        let primary = index(shape.outlines, shape.primary, shape.num_outlines);
        let out = append(
            &mut shape.outlines,
            &mut shape.sz_outlines,
            &mut shape.num_outlines,
        );
        if out.is_null() {
            guest::free(value.points.cast());
            return out;
        }
        if let Some(i) = approx {
            shape.approx = shape.outlines.add(i);
        }
        if let Some(i) = primary {
            shape.primary = shape.outlines.add(i);
        }
        out.write(value);
        out
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbAddGeomShape")]
pub unsafe extern "sysv64" fn XkbAddGeomShape(
    g: *mut Geometry,
    name: usize,
    outlines: c_int,
) -> *mut Shape {
    if g.is_null() || name == 0 || !(0..=65535).contains(&outlines) {
        return ptr::null_mut();
    }
    unsafe {
        let g = &mut *g;
        for i in 0..usize::from(g.num_shapes) {
            let out = g.shapes.add(i);
            if (*out).name == name {
                let approx = index((*out).outlines, (*out).approx, (*out).num_outlines);
                let primary = index((*out).outlines, (*out).primary, (*out).num_outlines);
                if !reserve(
                    &mut (*out).outlines,
                    &mut (*out).sz_outlines,
                    outlines as usize,
                ) {
                    return ptr::null_mut();
                }
                if let Some(i) = approx {
                    (*out).approx = (*out).outlines.add(i);
                }
                if let Some(i) = primary {
                    (*out).primary = (*out).outlines.add(i);
                }
                return out;
            }
        }
        let mut value: Shape = mem::zeroed();
        value.name = name;
        if !reserve(
            &mut value.outlines,
            &mut value.sz_outlines,
            outlines as usize,
        ) {
            return ptr::null_mut();
        }
        let out = append(&mut g.shapes, &mut g.sz_shapes, &mut g.num_shapes);
        if out.is_null() {
            guest::free(value.outlines.cast());
            return out;
        }
        out.write(value);
        out
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbAddGeomKey")]
pub unsafe extern "sysv64" fn XkbAddGeomKey(row: *mut Row) -> *mut Key {
    if row.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        let row = &mut *row;
        append(&mut row.keys, &mut row.sz_keys, &mut row.num_keys)
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbAddGeomRow")]
pub unsafe extern "sysv64" fn XkbAddGeomRow(section: *mut Section, keys: c_int) -> *mut Row {
    if section.is_null() || !(0..=65535).contains(&keys) {
        return ptr::null_mut();
    }
    unsafe {
        let s = &mut *section;
        let mut value: Row = mem::zeroed();
        if !reserve(&mut value.keys, &mut value.sz_keys, keys as usize) {
            return ptr::null_mut();
        }
        let out = append(&mut s.rows, &mut s.sz_rows, &mut s.num_rows);
        if out.is_null() {
            guest::free(value.keys.cast());
            return out;
        }
        out.write(value);
        out
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbAddGeomSection")]
pub unsafe extern "sysv64" fn XkbAddGeomSection(
    g: *mut Geometry,
    name: usize,
    rows: c_int,
    doodads: c_int,
    overlays: c_int,
) -> *mut Section {
    if g.is_null()
        || name == 0
        || [rows, doodads, overlays]
            .iter()
            .any(|v| !(0..=65535).contains(v))
    {
        return ptr::null_mut();
    }
    unsafe {
        let g = &mut *g;
        let existing =
            (0..usize::from(g.num_sections)).find(|i| (*g.sections.add(*i)).name == name);
        let out = if let Some(i) = existing {
            g.sections.add(i)
        } else {
            let out = append(&mut g.sections, &mut g.sz_sections, &mut g.num_sections);
            if out.is_null() {
                return out;
            }
            (*out).name = name;
            // Moving the section array must preserve overlay back-pointers.
            for i in 0..usize::from(g.num_sections) {
                let section = g.sections.add(i);
                for j in 0..usize::from((*section).num_overlays) {
                    (*(*section).overlays.add(j)).section_under = section;
                }
            }
            out
        };
        let s = &mut *out;
        if !reserve(&mut s.rows, &mut s.sz_rows, rows as usize)
            || !reserve(&mut s.doodads, &mut s.sz_doodads, doodads as usize)
            || !reserve(&mut s.overlays, &mut s.sz_overlays, overlays as usize)
        {
            return ptr::null_mut();
        }
        out
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbAddGeomOverlay")]
pub unsafe extern "sysv64" fn XkbAddGeomOverlay(
    section: *mut Section,
    name: usize,
    rows: c_int,
) -> *mut Overlay {
    if section.is_null() || name == 0 || !(0..=65535).contains(&rows) {
        return ptr::null_mut();
    }
    unsafe {
        let s = &mut *section;
        let existing =
            (0..usize::from(s.num_overlays)).find(|i| (*s.overlays.add(*i)).name == name);
        let out = if let Some(i) = existing {
            s.overlays.add(i)
        } else {
            append(&mut s.overlays, &mut s.sz_overlays, &mut s.num_overlays)
        };
        if out.is_null() {
            return out;
        }
        (*out).name = name;
        (*out).section_under = section;
        if !reserve(&mut (*out).rows, &mut (*out).sz_rows, rows as usize) {
            return ptr::null_mut();
        }
        out
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbAddGeomOverlayRow")]
pub unsafe extern "sysv64" fn XkbAddGeomOverlayRow(
    overlay: *mut Overlay,
    row_under: c_int,
    keys: c_int,
) -> *mut OverlayRow {
    if overlay.is_null() || !(0..=65535).contains(&keys) || row_under < 0 {
        return ptr::null_mut();
    }
    unsafe {
        let o = &mut *overlay;
        if o.section_under.is_null() || row_under >= i32::from((*o.section_under).num_rows) {
            return ptr::null_mut();
        }
        let existing =
            (0..usize::from(o.num_rows)).find(|i| (*o.rows.add(*i)).row_under == row_under as u16);
        let out = if let Some(i) = existing {
            o.rows.add(i)
        } else {
            append(&mut o.rows, &mut o.sz_rows, &mut o.num_rows)
        };
        if out.is_null() {
            return out;
        }
        (*out).row_under = row_under as u16;
        if !reserve(&mut (*out).keys, &mut (*out).sz_keys, keys as usize) {
            return ptr::null_mut();
        }
        out
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbAddGeomDoodad")]
pub unsafe extern "sysv64" fn XkbAddGeomDoodad(
    g: *mut Geometry,
    section: *mut Section,
    name: usize,
) -> *mut Doodad {
    if g.is_null() || name == 0 {
        return ptr::null_mut();
    }
    unsafe {
        let (data, capacity, count) = if section.is_null() {
            let g = &mut *g;
            (&mut g.doodads, &mut g.sz_doodads, &mut g.num_doodads)
        } else {
            let s = &mut *section;
            (&mut s.doodads, &mut s.sz_doodads, &mut s.num_doodads)
        };
        for i in 0..usize::from(*count) {
            let out = (*data).add(i);
            if (*out).name == name {
                return out;
            }
        }
        let out = append(data, capacity, count);
        if !out.is_null() {
            (*out).name = name;
        }
        out
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbAllocGeometry")]
pub unsafe extern "sysv64" fn XkbAllocGeometry(
    xkb: *mut XkbDescRec,
    sizes: *const Sizes,
) -> Status {
    if xkb.is_null() || sizes.is_null() {
        return 2;
    }
    unsafe {
        if (*xkb).geom.is_null() {
            (*xkb).geom = guest::malloc(mem::size_of::<Geometry>()).cast();
            if (*xkb).geom.is_null() {
                return 11;
            }
            ptr::write_bytes((*xkb).geom.cast::<Geometry>(), 0, 1);
        }
        let g = &mut *(*xkb).geom.cast::<Geometry>();
        let s = &*sizes;
        let label = index(g.colors, g.label_color, g.num_colors);
        let base = index(g.colors, g.base_color, g.num_colors);
        let ok = (s.which & 1 == 0
            || reserve(
                &mut g.properties,
                &mut g.sz_properties,
                s.properties as usize,
            ))
            && (s.which & 2 == 0 || reserve(&mut g.colors, &mut g.sz_colors, s.colors as usize))
            && (s.which & 4 == 0 || reserve(&mut g.shapes, &mut g.sz_shapes, s.shapes as usize))
            && (s.which & 8 == 0
                || reserve(&mut g.sections, &mut g.sz_sections, s.sections as usize))
            && (s.which & 16 == 0
                || reserve(&mut g.doodads, &mut g.sz_doodads, s.doodads as usize))
            && (s.which & 32 == 0
                || reserve(
                    &mut g.key_aliases,
                    &mut g.sz_key_aliases,
                    s.key_aliases as usize,
                ));
        if let Some(i) = label {
            g.label_color = g.colors.add(i);
        }
        if let Some(i) = base {
            g.base_color = g.colors.add(i);
        }
        for i in 0..g.num_sections as usize {
            let section = g.sections.add(i);
            for j in 0..(*section).num_overlays as usize {
                (*(*section).overlays.add(j)).section_under = section;
            }
        }
        if ok {
            0
        } else {
            XkbFreeGeometry((*xkb).geom.cast(), 0x3f, 1);
            (*xkb).geom = ptr::null_mut();
            11
        }
    }
}

unsafe fn free_doodads(data: *mut Doodad, count: u16) {
    unsafe {
        for i in 0..usize::from(count) {
            let d = &*data.add(i);
            if d.kind == 3 {
                guest::free(d.data[1] as *mut u8);
                guest::free(d.data[2] as *mut u8);
            }
            if d.kind == 5 {
                guest::free(d.data[1] as *mut u8);
            }
        }
        guest::free(data.cast());
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbFreeGeometry")]
pub unsafe extern "sysv64" fn XkbFreeGeometry(
    geometry: *mut Geometry,
    which: c_uint,
    free_map: Bool,
) {
    if geometry.is_null() {
        return;
    }
    unsafe {
        let g = &mut *geometry;
        let which = if free_map != 0 { 0x3f } else { which };
        if which & 1 != 0 {
            for i in 0..usize::from(g.num_properties) {
                let p = &*g.properties.add(i);
                guest::free(p.name.cast());
                guest::free(p.value.cast());
            }
            guest::free(g.properties.cast());
            g.properties = ptr::null_mut();
            g.num_properties = 0;
            g.sz_properties = 0;
        }
        if which & 2 != 0 {
            for i in 0..usize::from(g.num_colors) {
                guest::free((*g.colors.add(i)).spec.cast());
            }
            guest::free(g.colors.cast());
            g.colors = ptr::null_mut();
            g.num_colors = 0;
            g.sz_colors = 0;
            g.label_color = ptr::null_mut();
            g.base_color = ptr::null_mut();
        }
        if which & 4 != 0 {
            for i in 0..usize::from(g.num_shapes) {
                let s = &*g.shapes.add(i);
                for j in 0..usize::from(s.num_outlines) {
                    guest::free((*s.outlines.add(j)).points.cast());
                }
                guest::free(s.outlines.cast());
            }
            guest::free(g.shapes.cast());
            g.shapes = ptr::null_mut();
            g.num_shapes = 0;
            g.sz_shapes = 0;
        }
        if which & 8 != 0 {
            for i in 0..usize::from(g.num_sections) {
                let s = &*g.sections.add(i);
                for j in 0..usize::from(s.num_rows) {
                    guest::free((*s.rows.add(j)).keys.cast());
                }
                guest::free(s.rows.cast());
                free_doodads(s.doodads, s.num_doodads);
                for j in 0..usize::from(s.num_overlays) {
                    let o = &*s.overlays.add(j);
                    for k in 0..usize::from(o.num_rows) {
                        guest::free((*o.rows.add(k)).keys.cast());
                    }
                    guest::free(o.rows.cast());
                }
                guest::free(s.overlays.cast());
            }
            guest::free(g.sections.cast());
            g.sections = ptr::null_mut();
            g.num_sections = 0;
            g.sz_sections = 0;
        }
        if which & 16 != 0 {
            free_doodads(g.doodads, g.num_doodads);
            g.doodads = ptr::null_mut();
            g.num_doodads = 0;
            g.sz_doodads = 0;
        }
        if which & 32 != 0 {
            guest::free(g.key_aliases.cast());
            g.key_aliases = ptr::null_mut();
            g.num_key_aliases = 0;
            g.sz_key_aliases = 0;
        }
        if free_map != 0 {
            guest::free(g.label_font.cast());
            guest::free(geometry.cast());
        }
    }
}
