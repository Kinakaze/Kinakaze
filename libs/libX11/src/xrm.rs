//! X Resource Manager (Xrm) implementation.
//!
//! Provides standard Quark dictionaries, resource databases, parsing, and query functions.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_void};
use std::os::raw::{c_char, c_int, c_uint};
use std::sync::{Mutex, OnceLock};

pub type XrmQuark = c_int;
pub type XrmQuarkList = *mut XrmQuark;
pub type XrmString = *const c_char;
pub type XrmName = XrmQuark;
pub type XrmNameList = *mut XrmName;
pub type XrmClass = XrmQuark;
pub type XrmClassList = *mut XrmClass;
pub type XrmRepresentation = XrmQuark;
pub type XrmBinding = c_int;
pub type XrmBindingList = *mut XrmBinding;

pub const XrmBindTightly: XrmBinding = 0;
pub const XrmBindLoosely: XrmBinding = 1;

pub const NULLQUARK: XrmQuark = 0;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct XrmValue {
    pub size: c_uint,
    pub addr: *mut c_char,
}

struct QuarkRegistry {
    str_to_quark: HashMap<String, XrmQuark>,
    quark_to_str: HashMap<XrmQuark, CString>,
    next_quark: XrmQuark,
}

impl QuarkRegistry {
    fn new() -> Self {
        Self {
            str_to_quark: HashMap::new(),
            quark_to_str: HashMap::new(),
            next_quark: 1,
        }
    }

    fn string_to_quark(&mut self, name: &str) -> XrmQuark {
        if let Some(&q) = self.str_to_quark.get(name) {
            return q;
        }
        let q = self.next_quark;
        self.next_quark += 1;
        let cs = CString::new(name).unwrap_or_default();
        self.str_to_quark.insert(name.to_string(), q);
        self.quark_to_str.insert(q, cs);
        q
    }

    fn quark_to_string(&self, q: XrmQuark) -> Option<*const c_char> {
        self.quark_to_str.get(&q).map(|cs| cs.as_ptr())
    }
}

fn quark_registry() -> &'static Mutex<QuarkRegistry> {
    static REG: OnceLock<Mutex<QuarkRegistry>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(QuarkRegistry::new()))
}

#[derive(Clone, Debug)]
struct ResourceEntry {
    bindings: Vec<XrmBinding>,
    quarks: Vec<XrmQuark>,
    representation: String,
    value: CString,
}

#[derive(Clone, Debug, Default)]
pub struct XrmDatabaseRec {
    entries: Vec<ResourceEntry>,
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmInitialize")]
pub unsafe extern "sysv64" fn XrmInitialize() {
    if crate::trace_enabled() {
        crate::diagnostic!("[libX11] XrmInitialize begin");
    }
    let _ = quark_registry();
    if crate::trace_enabled() {
        crate::diagnostic!("[libX11] XrmInitialize complete");
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmStringToQuark")]
pub unsafe extern "sysv64" fn XrmStringToQuark(string: *const c_char) -> XrmQuark {
    if string.is_null() {
        return NULLQUARK;
    }
    let s = match unsafe { CStr::from_ptr(string) }.to_str() {
        Ok(v) => v,
        Err(_) => return NULLQUARK,
    };
    if let Ok(mut reg) = quark_registry().lock() {
        reg.string_to_quark(s)
    } else {
        NULLQUARK
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmPermStringToQuark")]
pub unsafe extern "sysv64" fn XrmPermStringToQuark(string: *const c_char) -> XrmQuark {
    unsafe { XrmStringToQuark(string) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmQuarkToString")]
pub unsafe extern "sysv64" fn XrmQuarkToString(quark: XrmQuark) -> *const c_char {
    if quark <= NULLQUARK {
        return core::ptr::null();
    }
    if let Ok(reg) = quark_registry().lock() {
        reg.quark_to_string(quark).unwrap_or(core::ptr::null())
    } else {
        core::ptr::null()
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmUniqueQuark")]
pub unsafe extern "sysv64" fn XrmUniqueQuark() -> XrmQuark {
    if let Ok(mut reg) = quark_registry().lock() {
        let q = reg.next_quark;
        reg.next_quark += 1;
        let name = format!("_XrmUniqueQuark_{}", q);
        let cs = CString::new(name.as_str()).unwrap_or_default();
        reg.str_to_quark.insert(name, q);
        reg.quark_to_str.insert(q, cs);
        q
    } else {
        NULLQUARK
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmStringToBindingQuarkList")]
pub unsafe extern "sysv64" fn XrmStringToBindingQuarkList(
    string: *const c_char,
    bindings_return: XrmBindingList,
    quarks_return: XrmQuarkList,
) {
    if string.is_null() || bindings_return.is_null() || quarks_return.is_null() {
        return;
    }
    let s = match unsafe { CStr::from_ptr(string) }.to_str() {
        Ok(v) => v,
        Err(_) => return,
    };

    let mut b_out = Vec::new();
    let mut q_out = Vec::new();

    let mut chars = s.chars().peekable();
    let mut current_binding = XrmBindTightly;

    while let Some(&c) = chars.peek() {
        if c == '.' {
            current_binding = XrmBindTightly;
            chars.next();
        } else if c == '*' {
            current_binding = XrmBindLoosely;
            chars.next();
        } else {
            let mut name = String::new();
            while let Some(&ch) = chars.peek() {
                if ch == '.' || ch == '*' || ch == ':' {
                    break;
                }
                name.push(ch);
                chars.next();
            }
            if !name.is_empty() {
                let q = if let Ok(mut reg) = quark_registry().lock() {
                    reg.string_to_quark(&name)
                } else {
                    NULLQUARK
                };
                b_out.push(current_binding);
                q_out.push(q);
                current_binding = XrmBindTightly;
            }
        }
    }

    for (i, (&b, &q)) in b_out.iter().zip(q_out.iter()).enumerate() {
        unsafe {
            *bindings_return.add(i) = b;
            *quarks_return.add(i) = q;
        }
    }
    unsafe {
        *quarks_return.add(b_out.len()) = NULLQUARK;
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmStringToQuarkList")]
pub unsafe extern "sysv64" fn XrmStringToQuarkList(
    string: *const c_char,
    quarks_return: XrmQuarkList,
) {
    if string.is_null() || quarks_return.is_null() {
        return;
    }
    let mut bindings = [0; 64];
    unsafe { XrmStringToBindingQuarkList(string, bindings.as_mut_ptr(), quarks_return) };
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmGetDatabase")]
pub unsafe extern "sysv64" fn XrmGetDatabase(dpy: *mut crate::Display) -> *mut XrmDatabaseRec {
    if dpy.is_null() {
        return core::ptr::null_mut();
    }
    unsafe { (*dpy).db as *mut XrmDatabaseRec }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmSetDatabase")]
pub unsafe extern "sysv64" fn XrmSetDatabase(
    dpy: *mut crate::Display,
    database: *mut XrmDatabaseRec,
) {
    if !dpy.is_null() {
        unsafe { (*dpy).db = database as *mut core::ffi::c_void };
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmDestroyDatabase")]
pub unsafe extern "sysv64" fn XrmDestroyDatabase(database: *mut XrmDatabaseRec) {
    if !database.is_null() {
        unsafe { drop(Box::from_raw(database)) };
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmGetStringDatabase")]
pub unsafe extern "sysv64" fn XrmGetStringDatabase(data: *const c_char) -> *mut XrmDatabaseRec {
    let db = Box::new(XrmDatabaseRec::default());
    let db_ptr = Box::into_raw(db);
    if data.is_null() {
        return db_ptr;
    }
    let s = match unsafe { CStr::from_ptr(data) }.to_str() {
        Ok(v) => v,
        Err(_) => return db_ptr,
    };

    for line in s.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('!') || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, val)) = trimmed.split_once(':') {
            let key = key.trim();
            let val = val.trim();
            let mut bindings = [0; 64];
            let mut quarks = [0; 64];
            let c_key = CString::new(key).unwrap_or_default();
            unsafe {
                XrmStringToBindingQuarkList(
                    c_key.as_ptr(),
                    bindings.as_mut_ptr(),
                    quarks.as_mut_ptr(),
                );
            }
            let mut q_vec = Vec::new();
            let mut b_vec = Vec::new();
            let mut i = 0;
            while quarks[i] != NULLQUARK && i < 63 {
                b_vec.push(bindings[i]);
                q_vec.push(quarks[i]);
                i += 1;
            }
            if !q_vec.is_empty() {
                let entry = ResourceEntry {
                    bindings: b_vec,
                    quarks: q_vec,
                    representation: "String".to_string(),
                    value: CString::new(val).unwrap_or_default(),
                };
                unsafe { (*db_ptr).entries.push(entry) };
            }
        }
    }

    db_ptr
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmGetFileDatabase")]
pub unsafe extern "sysv64" fn XrmGetFileDatabase(filename: *const c_char) -> *mut XrmDatabaseRec {
    if filename.is_null() {
        return core::ptr::null_mut();
    }
    let p = match unsafe { CStr::from_ptr(filename) }.to_str() {
        Ok(v) => v,
        Err(_) => return core::ptr::null_mut(),
    };
    if let Ok(content) = std::fs::read_to_string(p) {
        let cs = CString::new(content).unwrap_or_default();
        unsafe { XrmGetStringDatabase(cs.as_ptr()) }
    } else {
        core::ptr::null_mut()
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmPutFileDatabase")]
pub unsafe extern "sysv64" fn XrmPutFileDatabase(
    database: *mut XrmDatabaseRec,
    filename: *const c_char,
) {
    if database.is_null() || filename.is_null() {
        return;
    }
    let p = match unsafe { CStr::from_ptr(filename) }.to_str() {
        Ok(v) => v,
        Err(_) => return,
    };
    let mut out = String::new();
    unsafe {
        for entry in &(*database).entries {
            for (b, q) in entry.bindings.iter().zip(entry.quarks.iter()) {
                if *b == XrmBindLoosely {
                    out.push('*');
                } else {
                    out.push('.');
                }
                if let Ok(reg) = quark_registry().lock() {
                    if let Some(s) = reg.quark_to_str.get(q) {
                        out.push_str(&s.to_string_lossy());
                    }
                }
            }
            out.push_str(": ");
            out.push_str(&entry.value.to_string_lossy());
            out.push('\n');
        }
    }
    let _ = std::fs::write(p, out);
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmMergeDatabases")]
pub unsafe extern "sysv64" fn XrmMergeDatabases(
    source_db: *mut XrmDatabaseRec,
    target_db: *mut *mut XrmDatabaseRec,
) {
    if source_db.is_null() || target_db.is_null() {
        return;
    }
    if (*target_db).is_null() {
        unsafe { *target_db = source_db };
        return;
    }
    unsafe {
        for entry in &(*source_db).entries {
            (**target_db).entries.push(entry.clone());
        }
        drop(Box::from_raw(source_db));
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmCombineDatabase")]
pub unsafe extern "sysv64" fn XrmCombineDatabase(
    source_db: *mut XrmDatabaseRec,
    target_db: *mut *mut XrmDatabaseRec,
    _override: Bool,
) {
    unsafe { XrmMergeDatabases(source_db, target_db) };
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmCombineFileDatabase")]
pub unsafe extern "sysv64" fn XrmCombineFileDatabase(
    filename: *const c_char,
    target_db: *mut *mut XrmDatabaseRec,
    _override: Bool,
) -> c_int {
    let src = unsafe { XrmGetFileDatabase(filename) };
    if !src.is_null() {
        unsafe { XrmMergeDatabases(src, target_db) };
        1
    } else {
        0
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmQGetResource")]
pub unsafe extern "sysv64" fn XrmQGetResource(
    database: *mut XrmDatabaseRec,
    quark_name: XrmNameList,
    _quark_class: XrmClassList,
    quark_type_return: *mut XrmRepresentation,
    value_return: *mut XrmValue,
) -> Bool {
    if database.is_null() || quark_name.is_null() || value_return.is_null() {
        return 0;
    }
    let mut names = Vec::new();
    let mut i = 0;
    unsafe {
        while *quark_name.add(i) != NULLQUARK && i < 64 {
            names.push(*quark_name.add(i));
            i += 1;
        }
    }
    if names.is_empty() {
        return 0;
    }

    unsafe {
        // Search in reverse so later merged entries have precedence
        for entry in (*database).entries.iter().rev() {
            if entry.quarks.last() == names.last() {
                let string_quark = if let Ok(mut reg) = quark_registry().lock() {
                    reg.string_to_quark("String")
                } else {
                    NULLQUARK
                };
                if !quark_type_return.is_null() {
                    *quark_type_return = string_quark;
                }
                (*value_return).size = (entry.value.as_bytes().len() + 1) as c_uint;
                (*value_return).addr = entry.value.as_ptr() as *mut c_char;
                return 1;
            }
        }
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmGetResource")]
pub unsafe extern "sysv64" fn XrmGetResource(
    database: *mut XrmDatabaseRec,
    str_name: *const c_char,
    str_class: *const c_char,
    str_type_return: *mut *mut c_char,
    value_return: *mut XrmValue,
) -> Bool {
    if database.is_null() || str_name.is_null() || value_return.is_null() {
        return 0;
    }
    let mut names = [0; 64];
    let mut classes = [0; 64];
    unsafe {
        XrmStringToQuarkList(str_name, names.as_mut_ptr());
        if !str_class.is_null() {
            XrmStringToQuarkList(str_class, classes.as_mut_ptr());
        }
    }
    let mut rep: XrmRepresentation = NULLQUARK;
    let res = unsafe {
        XrmQGetResource(
            database,
            names.as_mut_ptr(),
            classes.as_mut_ptr(),
            &mut rep,
            value_return,
        )
    };
    if res != 0 && !str_type_return.is_null() {
        static STRING_TYPE: &[u8] = b"String\0";
        unsafe { *str_type_return = STRING_TYPE.as_ptr() as *mut c_char };
    }
    res
}

pub type XrmSearchList = *mut *mut c_void;

#[unsafe(export_name = "kinakaze_engine_libX11_XrmQGetSearchList")]
pub unsafe extern "sysv64" fn XrmQGetSearchList(
    _database: *mut XrmDatabaseRec,
    _names: XrmNameList,
    _classes: XrmClassList,
    search_list_return: XrmSearchList,
    _list_length: c_int,
) -> Bool {
    if !search_list_return.is_null() {
        unsafe { *search_list_return = core::ptr::null_mut() };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmQGetSearchResource")]
pub unsafe extern "sysv64" fn XrmQGetSearchResource(
    _search_list: XrmSearchList,
    _name: XrmName,
    _class: XrmClass,
    quark_type_return: *mut XrmRepresentation,
    value_return: *mut XrmValue,
) -> Bool {
    if !value_return.is_null() {
        unsafe {
            (*value_return).size = 0;
            (*value_return).addr = core::ptr::null_mut();
        }
    }
    if !quark_type_return.is_null() {
        unsafe { *quark_type_return = NULLQUARK };
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmPutStringResource")]
pub unsafe extern "sysv64" fn XrmPutStringResource(
    database: *mut *mut XrmDatabaseRec,
    spec: *const c_char,
    value: *const c_char,
) {
    if database.is_null() || spec.is_null() || value.is_null() {
        return;
    }
    if (*database).is_null() {
        unsafe { *database = Box::into_raw(Box::new(XrmDatabaseRec::default())) };
    }
    let mut bindings = [0; 64];
    let mut quarks = [0; 64];
    unsafe {
        XrmStringToBindingQuarkList(spec, bindings.as_mut_ptr(), quarks.as_mut_ptr());
    }
    let val_cstr = unsafe { CStr::from_ptr(value) }.to_owned();
    let mut q_vec = Vec::new();
    let mut b_vec = Vec::new();
    let mut i = 0;
    while quarks[i] != NULLQUARK && i < 63 {
        b_vec.push(bindings[i]);
        q_vec.push(quarks[i]);
        i += 1;
    }
    if !q_vec.is_empty() {
        let entry = ResourceEntry {
            bindings: b_vec,
            quarks: q_vec,
            representation: "String".to_string(),
            value: val_cstr,
        };
        unsafe { (**database).entries.push(entry) };
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmPutLineResource")]
pub unsafe extern "sysv64" fn XrmPutLineResource(
    database: *mut *mut XrmDatabaseRec,
    line: *const c_char,
) {
    if database.is_null() || line.is_null() {
        return;
    }
    let temp_db = unsafe { XrmGetStringDatabase(line) };
    if !temp_db.is_null() {
        unsafe { XrmMergeDatabases(temp_db, database) };
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmQPutResource")]
pub unsafe extern "sysv64" fn XrmQPutResource(
    database: *mut *mut XrmDatabaseRec,
    bindings: XrmBindingList,
    quarks: XrmQuarkList,
    _representation: XrmRepresentation,
    value: *mut XrmValue,
) {
    if database.is_null() || bindings.is_null() || quarks.is_null() || value.is_null() {
        return;
    }
    if (*database).is_null() {
        unsafe { *database = Box::into_raw(Box::new(XrmDatabaseRec::default())) };
    }
    let mut q_vec = Vec::new();
    let mut b_vec = Vec::new();
    let mut i = 0;
    unsafe {
        while *quarks.add(i) != NULLQUARK && i < 63 {
            b_vec.push(*bindings.add(i));
            q_vec.push(*quarks.add(i));
            i += 1;
        }
    }
    let val_cstr = if unsafe { (*value).size > 0 && !(*value).addr.is_null() } {
        unsafe { CStr::from_ptr((*value).addr) }.to_owned()
    } else {
        CString::default()
    };
    if !q_vec.is_empty() {
        let entry = ResourceEntry {
            bindings: b_vec,
            quarks: q_vec,
            representation: "String".to_string(),
            value: val_cstr,
        };
        unsafe { (**database).entries.push(entry) };
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmQPutStringResource")]
pub unsafe extern "sysv64" fn XrmQPutStringResource(
    database: *mut *mut XrmDatabaseRec,
    bindings: XrmBindingList,
    quarks: XrmQuarkList,
    value: *const c_char,
) {
    if value.is_null() {
        return;
    }
    let mut val: XrmValue = XrmValue {
        size: unsafe { libc_strlen(value) as c_uint + 1 },
        addr: value as *mut c_char,
    };
    unsafe { XrmQPutResource(database, bindings, quarks, NULLQUARK, &mut val) };
}

#[repr(C)]
pub struct XrmOptionDescRec {
    pub option: *const c_char,
    pub specifier: *const c_char,
    pub arg_kind: c_int,
    pub value: *mut c_void,
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmParseCommand")]
pub unsafe extern "sysv64" fn XrmParseCommand(
    _database: *mut *mut XrmDatabaseRec,
    _table: *const XrmOptionDescRec,
    _table_count: c_int,
    _name: *const c_char,
    _argc: *mut c_int,
    _argv: *mut *mut c_char,
) {
    // Command parsing stub; retains argc/argv as-is.
}

#[unsafe(export_name = "kinakaze_engine_libX11_XrmEnumerateDatabase")]
pub unsafe extern "sysv64" fn XrmEnumerateDatabase(
    _database: *mut XrmDatabaseRec,
    _name_prefix: XrmNameList,
    _class_prefix: XrmClassList,
    _mode: c_int,
    _proc: *mut c_void,
    _closure: *mut c_void,
) -> Bool {
    1
}

unsafe fn libc_strlen(s: *const c_char) -> usize {
    let mut len = 0;
    while unsafe { *s.add(len) } != 0 {
        len += 1;
    }
    len
}

type Bool = c_int;
