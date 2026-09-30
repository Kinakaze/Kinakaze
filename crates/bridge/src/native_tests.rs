use super::*;

fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn fixture() -> Vec<u8> {
    let mut bytes = vec![0; 0x600];
    bytes[..2].copy_from_slice(b"MZ");
    put32(&mut bytes, 0x3c, 0x80);
    bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
    put16(&mut bytes, 0x84, 0x8664);
    put16(&mut bytes, 0x86, 2);
    put16(&mut bytes, 0x94, 240);
    put16(&mut bytes, 0x96, 0x2000);
    put16(&mut bytes, 0x98, 0x20b);
    put32(&mut bytes, 0x98 + 108, 1);
    put32(&mut bytes, 0x98 + 112, 0x1000);
    put32(&mut bytes, 0x98 + 116, 0x180);
    for (index, rva, raw, flags) in [
        (0, 0x1000, 0x200, 0x6000_0020),
        (1, 0x2000, 0x400, 0xc000_0040),
    ] {
        let section = 0x188 + index * 40;
        put32(&mut bytes, section + 8, 0x200);
        put32(&mut bytes, section + 12, rva);
        put32(&mut bytes, section + 16, 0x200);
        put32(&mut bytes, section + 20, raw);
        put32(&mut bytes, section + 36, flags);
    }
    for (offset, value) in [
        (12, 0x1120),
        (20, 3),
        (24, 3),
        (28, 0x1040),
        (32, 0x1050),
        (36, 0x1060),
    ] {
        put32(&mut bytes, 0x200 + offset, value);
    }
    for (index, address, name, text) in [
        (0, 0x11c0, 0x1090, "_Rinternal"),
        (1, 0x1150, 0x10a0, "forwarded"),
        (2, 0x2010, 0x10b0, "object"),
    ] {
        put32(&mut bytes, 0x240 + index * 4, address);
        put32(&mut bytes, 0x250 + index * 4, name);
        put16(&mut bytes, 0x260 + index * 2, index as u16);
        let offset = (name - 0x1000 + 0x200) as usize;
        bytes[offset..offset + text.len()].copy_from_slice(text.as_bytes());
    }
    bytes[0x320..0x32d].copy_from_slice(b"libfixture.so");
    bytes[0x350..0x35c].copy_from_slice(b"KERNEL32.Foo");
    bytes
}

#[test]
fn borrowed_exports_preserve_functions_objects_forwarders_and_owned_lifetime() {
    let bytes = fixture();
    let borrowed = borrowed_exports(&bytes).unwrap();
    assert_eq!(borrowed.name, "libfixture.so");
    let actual: Vec<_> = borrowed
        .symbols
        .iter()
        .map(|symbol| (symbol.name, symbol.object))
        .collect();
    assert_eq!(
        actual,
        [
            ("_Rinternal", Some(false)),
            ("forwarded", None),
            ("object", Some(true))
        ]
    );
    for symbol in &borrowed.symbols {
        assert!(bytes.as_ptr_range().contains(&symbol.name.as_ptr()));
    }
    let owned = exports(&bytes).unwrap();
    drop(bytes);
    assert_eq!(owned.name, "libfixture.so");
    assert_eq!(owned.symbols[2].name, "object");
    assert_eq!(owned.symbols[2].object, Some(true));
}

#[test]
fn malformed_exports_are_rejected_even_when_the_name_is_rust_only() {
    let bytes = fixture();
    for length in 0..bytes.len() {
        assert!(
            borrowed_exports(&bytes[..length]).is_err(),
            "length={length}"
        );
    }
    for variant in 0..6 {
        let mut bad = bytes.clone();
        match variant {
            0 => put32(&mut bad, 0x250, 0x10b0), // unsorted/duplicate name
            1 => put16(&mut bad, 0x260, 3),      // invalid ordinal
            2 => put32(&mut bad, 0x240, 0),      // null export
            3 => put32(&mut bad, 0x240, 0x9000), // unbacked export
            4 => bad[0x290] = 0xff,              // non-UTF8 Rust name
            _ => put32(&mut bad, 0x188 + 36, 0x2000_0020), // unreadable code
        }
        assert!(borrowed_exports(&bad).is_err(), "variant={variant}");
        assert!(exports(&bad).is_err(), "variant={variant}");
    }
}

#[test]
fn export_names_and_addresses_may_alternate_between_sections() {
    let mut bytes = fixture();
    put32(&mut bytes, 0x250, 0x2020);
    bytes[0x420..0x42a].copy_from_slice(b"_Rinternal");
    put32(&mut bytes, 0x258, 0x2040);
    bytes[0x440..0x446].copy_from_slice(b"object");
    put32(&mut bytes, 0x240, 0x2010);
    put32(&mut bytes, 0x244, 0x11c0);
    let symbols = borrowed_exports(&bytes).unwrap();
    assert_eq!(
        symbols
            .symbols
            .iter()
            .map(|s| (s.name, s.object))
            .collect::<Vec<_>>(),
        [
            ("_Rinternal", Some(true)),
            ("forwarded", Some(false)),
            ("object", Some(true))
        ]
    );
    // A previously valid section must not hide a later out-of-bounds name.
    put32(&mut bytes, 0x258, 0x21ff);
    bytes[0x5ff] = b'x';
    assert!(borrowed_exports(&bytes).is_err());
}

#[test]
fn export_names_keep_the_length_cap_and_cannot_cross_sections() {
    let mut bytes = fixture();
    bytes.resize(0x2400, 0);
    let section = 0x188 + 40;
    put32(&mut bytes, section + 8, 0x2000);
    put32(&mut bytes, section + 16, 0x2000);
    put32(&mut bytes, 0x250, 0x2100);
    bytes[0x500..0x1500].fill(b'a');
    bytes[0x500] = b'_';
    assert_eq!(
        borrowed_exports(&bytes).unwrap().symbols[0].name.len(),
        4096
    );
    bytes[0x1500] = b'a';
    assert!(borrowed_exports(&bytes).is_err());
    bytes[0x1500] = 0;
    // The terminator exists in the file, but lies outside the name's section.
    put32(&mut bytes, section + 8, 0x1100);
    put32(&mut bytes, section + 16, 0x1100);
    assert!(borrowed_exports(&bytes).is_err());
}

#[test]
fn command_facade_identity_survives_rebuilds_but_rejects_noncode_exports() {
    let mut bytes = fixture();
    assert!(matches_command_facade(
        &bytes,
        "libfixture.so",
        &["_Rinternal"]
    ));
    put32(&mut bytes, 0x88, 123456789); // COFF timestamp from another build.
    assert!(matches_command_facade(
        &bytes,
        "libfixture.so",
        &["_Rinternal"]
    ));
    for names in [&[][..], &["missing"], &["object"], &["forwarded"]] {
        assert!(!matches_command_facade(&bytes, "libfixture.so", names));
    }
    assert!(!matches_command_facade(&bytes, "other.so", &["_Rinternal"]));
    for length in 0..bytes.len() {
        assert!(!matches_command_facade(
            &bytes[..length],
            "libfixture.so",
            &["_Rinternal"]
        ));
    }
    put16(&mut bytes, 0x260, 3); // Malformed ordinal in an otherwise matching DLL.
    assert!(!matches_command_facade(
        &bytes,
        "libfixture.so",
        &["_Rinternal"]
    ));
}
