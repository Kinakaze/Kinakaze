//! The initial process stack a Linux `_start` expects to be handed.
//!
//! On Linux the kernel, not the program, builds this. `_start` receives no
//! arguments: it reads everything off the stack at a fixed layout and passes it to
//! `__libc_start_main`. Hosting a real binary therefore means synthesizing the
//! same block, because `crt1.o` will read it regardless of who set it up.

/// Auxiliary vector tags.
pub const AT_NULL: u64 = 0;
pub const AT_IGNORE: u64 = 1;
pub const AT_EXECFD: u64 = 2;
pub const AT_PHDR: u64 = 3;
pub const AT_PHENT: u64 = 4;
pub const AT_PHNUM: u64 = 5;
pub const AT_PAGESZ: u64 = 6;
pub const AT_BASE: u64 = 7;
pub const AT_FLAGS: u64 = 8;
pub const AT_ENTRY: u64 = 9;
pub const AT_NOTELF: u64 = 10;
pub const AT_UID: u64 = 11;
pub const AT_EUID: u64 = 12;
pub const AT_GID: u64 = 13;
pub const AT_EGID: u64 = 14;
pub const AT_PLATFORM: u64 = 15;
pub const AT_HWCAP: u64 = 16;
pub const AT_CLKTCK: u64 = 17;
pub const AT_SECURE: u64 = 23;
pub const AT_RANDOM: u64 = 25;
pub const AT_HWCAP2: u64 = 26;
pub const AT_EXECFN: u64 = 31;
pub const AT_SYSINFO_EHDR: u64 = 33;
pub const AT_MINSIGSTKSZ: u64 = 51;

/// What the caller wants placed in the auxiliary vector.
#[derive(Clone, Debug, Default)]
pub struct AuxiliaryValues {
    /// Mapped address of the program headers, for `AT_PHDR`.
    pub program_headers: Option<usize>,
    pub program_header_size: u64,
    pub program_header_count: u64,
    /// The executable's entry point, for `AT_ENTRY`.
    pub entry: Option<usize>,
    /// Load base of the interpreter, for `AT_BASE`. Zero when there is none.
    pub interpreter_base: Option<usize>,
    pub page_size: u64,
    /// Path the executable was invoked as, for `AT_EXECFN`.
    pub executable_path: Option<String>,
}

/// A built stack image, ready to be placed and jumped to.
#[derive(Debug)]
pub struct StackImage {
    /// The whole block, with the stack pointer at the start.
    pub bytes: Vec<u8>,
    /// Byte offset within `bytes` that the stack pointer must be set to.
    pub stack_pointer_offset: usize,
    /// Byte offsets of every word holding a pointer into this same block.
    ///
    /// The block is built as if it were placed at address zero, so these words
    /// start out as bare offsets. `relocate_to` turns them into real pointers.
    pointer_slots: Vec<usize>,
}

impl StackImage {
    /// Rewrites every self-referential word from an offset into a real pointer.
    ///
    /// `build` cannot write the argv and envp pointers directly, because their
    /// targets are strings inside this same buffer and the buffer's final address
    /// is not known until the caller has placed it. So the block is built at a
    /// notional base of zero and every such word holds a plain offset; this adds
    /// the real base to each one.
    ///
    /// Call this exactly once, with the address of `bytes[0]`. A second call would
    /// add the base to words that already contain absolute pointers, leaving the
    /// block pointing at addresses twice as far along as intended, which is not
    /// detectable from the buffer's contents afterwards.
    ///
    /// # Safety
    ///
    /// This is only sound as a step towards handing the block to guest code: it is
    /// the caller's responsibility that `base` is the address `bytes` will actually
    /// live at when the guest reads it, that it is 16-byte aligned so
    /// `stack_pointer_offset` lands on an aligned stack pointer, and that the
    /// buffer is not moved afterwards. Moving `bytes` after relocating invalidates
    /// every pointer in it.
    pub unsafe fn relocate_to(&mut self, base: usize) {
        for &slot in &self.pointer_slots {
            let word = &mut self.bytes[slot..slot + 8];
            let offset = u64::from_le_bytes(word.try_into().expect("slot is 8 bytes"));
            let absolute = (base as u64).wrapping_add(offset);
            word.copy_from_slice(&absolute.to_le_bytes());
        }
    }

    /// The stack pointer the guest must be entered with, once placed at `base`.
    pub fn stack_pointer(&self, base: usize) -> usize {
        base + self.stack_pointer_offset
    }
}

/// Truncates at the first interior NUL and appends the terminator.
///
/// A C string cannot represent an embedded NUL, so there is no faithful encoding
/// of one; truncating keeps the layout intact and loses only the unrepresentable
/// tail, where passing the bytes through would silently shift everything after it.
fn c_string_bytes(value: &str) -> Vec<u8> {
    let bytes = value.as_bytes();
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    let mut out = Vec::with_capacity(end + 1);
    out.extend_from_slice(&bytes[..end]);
    out.push(0);
    out
}

/// Sixteen bytes for `AT_RANDOM`.
///
/// This is NOT cryptographically strong, and is not trying to be. glibc reads this
/// pointer unconditionally to seed the stack canary and the pointer guard, so the
/// contract to satisfy is "sixteen readable bytes that differ between runs".
/// Anything relying on these for real unpredictability would need a proper CSPRNG
/// instead.
fn weak_random_bytes() -> [u8; 16] {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    // The clock alone is not enough: on Windows its granularity is coarse enough
    // that two builds in quick succession can read the same instant, and the tests
    // require successive builds to differ.
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0);
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    let local = 0u8;
    let stack_address = &local as *const u8 as u64;

    let mut state = nanos
        ^ (u64::from(std::process::id()).wrapping_mul(0x9E37_79B9_7F4A_7C15))
        ^ stack_address.rotate_left(17)
        ^ sequence.wrapping_mul(0xD1B5_4A32_D192_ED03);

    let mut bytes = [0u8; 16];
    for chunk in bytes.chunks_mut(8) {
        // SplitMix64's finalizer, to spread the mixed-in bits across all 64.
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut word = state;
        word = (word ^ (word >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        word = (word ^ (word >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        word ^= word >> 31;
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    bytes
}

fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

/// Builds the initial stack block.
///
/// The layout, from the stack pointer upward, is fixed by the ABI:
///
/// ```text
///   argc            (8 bytes)
///   argv[0..argc]   pointers
///   NULL
///   envp[0..]       pointers
///   NULL
///   auxv            pairs of (tag, value)
///   AT_NULL, 0
///   ... string data and the AT_RANDOM bytes ...
/// ```
///
/// The stack pointer must be 16-byte aligned at entry, as the SysV ABI requires.
/// The block is built at a notional base of zero with `stack_pointer_offset` itself
/// 16-byte aligned, so **the caller must place `bytes` at a 16-byte aligned
/// address** for that to hold; the alignment cannot be fixed up later, since the
/// pointers stored in the block are relative to the start of the buffer.
///
/// The returned image holds offsets, not addresses. It is unusable by the guest
/// until `StackImage::relocate_to` has been called with its real base.
pub fn build(
    arguments: &[String],
    environment: &[String],
    auxiliary: &AuxiliaryValues,
) -> StackImage {
    let argument_strings: Vec<Vec<u8>> = arguments
        .iter()
        .map(String::as_str)
        .map(c_string_bytes)
        .collect();
    let environment_strings: Vec<Vec<u8>> = environment
        .iter()
        .map(String::as_str)
        .map(c_string_bytes)
        .collect();
    let executable_string = auxiliary.executable_path.as_deref().map(c_string_bytes);

    // Which auxv entries appear, in order. The value of every entry that points
    // into this block is filled in below, once the data area's offsets are known.
    let mut auxv: Vec<(u64, u64)> = Vec::new();
    if let Some(headers) = auxiliary.program_headers {
        // AT_PHDR and friends describe the already-mapped image, so these are real
        // addresses from the caller rather than offsets into this block.
        auxv.push((AT_PHDR, headers as u64));
        auxv.push((AT_PHENT, auxiliary.program_header_size));
        auxv.push((AT_PHNUM, auxiliary.program_header_count));
    }
    let page_size = if auxiliary.page_size == 0 {
        4096
    } else {
        auxiliary.page_size
    };
    auxv.push((AT_PAGESZ, page_size));
    if let Some(base) = auxiliary.interpreter_base {
        auxv.push((AT_BASE, base as u64));
    }
    if let Some(entry) = auxiliary.entry {
        auxv.push((AT_ENTRY, entry as u64));
    }
    auxv.push((AT_FLAGS, 0));
    // Not setuid, so glibc may honour the environment normally. A nonzero value
    // here would make it scrub LD_* and friends.
    auxv.push((AT_SECURE, 0));
    // Windows has no ids that map onto these, and presenting as root is the least
    // surprising choice for a hosted guest that only ever sees itself.
    auxv.push((AT_UID, 0));
    auxv.push((AT_EUID, 0));
    auxv.push((AT_GID, 0));
    auxv.push((AT_EGID, 0));
    auxv.push((AT_CLKTCK, 100));
    // Zero rather than a guess: a wrong bit here sends glibc down an ifunc path
    // for an instruction set the host may not have. Zero is always truthful, and
    // costs only the baseline implementations of the string and memory routines.
    auxv.push((AT_HWCAP, 0));
    // AT_RANDOM's value is an offset into the data area, patched in below.
    let random_entry = auxv.len();
    auxv.push((AT_RANDOM, 0));
    let execfn_entry = executable_string.as_ref().map(|_| {
        auxv.push((AT_EXECFN, 0));
        auxv.len() - 1
    });
    // Deliberately no AT_SYSINFO_EHDR: there is no vDSO in this process, and
    // advertising one would have glibc resolve `clock_gettime` and friends to
    // addresses inside an image that does not exist.
    auxv.push((AT_NULL, 0));

    // The vector region: argc, argv + NULL, envp + NULL, then the auxv pairs.
    let word_count =
        1 + argument_strings.len() + 1 + environment_strings.len() + 1 + auxv.len() * 2;
    let vector_bytes = word_count * 8;

    // The vectors are laid out first and the strings after them, so the amount of
    // padding needed to keep the stack pointer aligned does not depend on how many
    // arguments there are: offset zero is 16-byte aligned whatever follows it. The
    // kernel pads at the front because it works downward from a fixed stack top;
    // building a freestanding buffer instead moves the slack to the data area,
    // which is padded up so the AT_RANDOM bytes start aligned as well.
    let stack_pointer_offset = 0;
    let data_start = align_up(stack_pointer_offset + vector_bytes, 16);

    let mut data = Vec::new();
    // AT_RANDOM first, so its sixteen bytes inherit the data area's alignment.
    let random_offset = data_start + data.len();
    data.extend_from_slice(&weak_random_bytes());

    let mut argument_offsets = Vec::with_capacity(argument_strings.len());
    for string in &argument_strings {
        argument_offsets.push(data_start + data.len());
        data.extend_from_slice(string);
    }
    let mut environment_offsets = Vec::with_capacity(environment_strings.len());
    for string in &environment_strings {
        environment_offsets.push(data_start + data.len());
        data.extend_from_slice(string);
    }
    let execfn_offset = executable_string.as_ref().map(|string| {
        let offset = data_start + data.len();
        data.extend_from_slice(string);
        offset
    });

    auxv[random_entry].1 = random_offset as u64;
    if let (Some(entry), Some(offset)) = (execfn_entry, execfn_offset) {
        auxv[entry].1 = offset as u64;
    }

    let mut bytes = vec![0u8; data_start];
    let mut pointer_slots = Vec::new();
    let mut cursor = stack_pointer_offset;

    fn put(bytes: &mut [u8], cursor: &mut usize, value: u64) {
        bytes[*cursor..*cursor + 8].copy_from_slice(&value.to_le_bytes());
        *cursor += 8;
    }

    put(&mut bytes, &mut cursor, argument_strings.len() as u64);
    for &offset in &argument_offsets {
        pointer_slots.push(cursor);
        put(&mut bytes, &mut cursor, offset as u64);
    }
    put(&mut bytes, &mut cursor, 0);
    for &offset in &environment_offsets {
        pointer_slots.push(cursor);
        put(&mut bytes, &mut cursor, offset as u64);
    }
    put(&mut bytes, &mut cursor, 0);
    for &(tag, value) in &auxv {
        put(&mut bytes, &mut cursor, tag);
        // Only the entries whose value is an offset into this block get relocated;
        // AT_PHDR, AT_BASE and AT_ENTRY are already absolute.
        if tag == AT_RANDOM || tag == AT_EXECFN {
            pointer_slots.push(cursor);
        }
        put(&mut bytes, &mut cursor, value);
    }
    debug_assert_eq!(cursor, stack_pointer_offset + vector_bytes);

    bytes.extend_from_slice(&data);

    StackImage {
        bytes,
        stack_pointer_offset,
        pointer_slots,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// What a C runtime would recover from the block by walking it.
    struct Walked {
        arguments: Vec<Vec<u8>>,
        environment: Vec<Vec<u8>>,
        auxv: Vec<(u64, u64)>,
    }

    /// A 16-byte aligned home for a block, so the alignment claim is real.
    ///
    /// A `Vec<u8>`'s buffer is only byte-aligned, so relocating to `bytes.as_ptr()`
    /// would not exercise the aligned-base contract that `build` documents.
    #[repr(align(16))]
    struct Aligned([u8; 8192]);

    fn place(image: &mut StackImage, home: &mut Aligned) -> usize {
        assert!(image.bytes.len() <= home.0.len(), "test buffer too small");
        let base = home.0.as_ptr() as usize;
        assert_eq!(base % 16, 0, "test home must be aligned");
        // SAFETY: relocate once, to the address the bytes are about to be copied
        // to, and the copy is what the walk below reads through.
        unsafe { image.relocate_to(base) };
        home.0[..image.bytes.len()].copy_from_slice(&image.bytes);
        base
    }

    /// Reads the block exactly as `_start` does: words off the stack, pointers
    /// followed into the data area, each vector ended by its NULL.
    ///
    /// # Safety
    ///
    /// `stack_pointer` must be a relocated block that is still in place.
    unsafe fn walk(stack_pointer: usize) -> Walked {
        unsafe fn word(address: usize) -> u64 {
            unsafe { (address as *const u64).read_unaligned() }
        }
        unsafe fn c_string(address: usize) -> Vec<u8> {
            let mut out = Vec::new();
            let mut cursor = address as *const u8;
            loop {
                let byte = unsafe { cursor.read() };
                if byte == 0 {
                    return out;
                }
                out.push(byte);
                cursor = unsafe { cursor.add(1) };
            }
        }

        unsafe {
            let argc = word(stack_pointer) as usize;
            let mut cursor = stack_pointer + 8;

            let mut arguments = Vec::with_capacity(argc);
            for _ in 0..argc {
                let pointer = word(cursor) as usize;
                assert_ne!(pointer, 0, "argv entry before argc is NULL");
                arguments.push(c_string(pointer));
                cursor += 8;
            }
            assert_eq!(word(cursor), 0, "argv is not NULL-terminated at argc");
            cursor += 8;

            let mut environment = Vec::new();
            while word(cursor) != 0 {
                environment.push(c_string(word(cursor) as usize));
                cursor += 8;
            }
            cursor += 8;

            let mut auxv = Vec::new();
            loop {
                let tag = word(cursor);
                let value = word(cursor + 8);
                cursor += 16;
                auxv.push((tag, value));
                if tag == AT_NULL {
                    break;
                }
            }

            Walked {
                arguments,
                environment,
                auxv,
            }
        }
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn round_trips_arguments_environment_and_auxv() {
        let arguments = strings(&["/bin/echo", "hello", "world"]);
        let environment = strings(&["PATH=/usr/bin", "HOME=/root"]);
        let auxiliary = AuxiliaryValues {
            program_headers: Some(0x40_0040),
            program_header_size: 56,
            program_header_count: 11,
            entry: Some(0x40_1000),
            interpreter_base: Some(0x7f00_0000),
            page_size: 4096,
            executable_path: Some("/bin/echo".to_string()),
        };

        let mut image = build(&arguments, &environment, &auxiliary);
        let mut home = Aligned([0; 8192]);
        let base = place(&mut image, &mut home);
        let walked = unsafe { walk(image.stack_pointer(base)) };

        assert_eq!(walked.arguments.len(), 3);
        for (recovered, original) in walked.arguments.iter().zip(&arguments) {
            assert_eq!(recovered.as_slice(), original.as_bytes());
        }
        assert_eq!(walked.environment.len(), 2);
        for (recovered, original) in walked.environment.iter().zip(&environment) {
            assert_eq!(recovered.as_slice(), original.as_bytes());
        }

        let tags: BTreeMap<u64, u64> = walked.auxv.iter().copied().collect();
        assert_eq!(tags[&AT_PHDR], 0x40_0040);
        assert_eq!(tags[&AT_PHENT], 56);
        assert_eq!(tags[&AT_PHNUM], 11);
        assert_eq!(tags[&AT_ENTRY], 0x40_1000);
        assert_eq!(tags[&AT_BASE], 0x7f00_0000);
        assert_eq!(tags[&AT_PAGESZ], 4096);
        assert_eq!(tags[&AT_CLKTCK], 100);
        assert_eq!(tags[&AT_SECURE], 0);
        assert_eq!(tags[&AT_FLAGS], 0);
        assert_eq!(tags[&AT_UID], 0);
        assert_eq!(tags[&AT_EUID], 0);
        assert_eq!(tags[&AT_GID], 0);
        assert_eq!(tags[&AT_EGID], 0);
        assert!(tags.contains_key(&AT_HWCAP));
        // A bogus vDSO would send glibc jumping into unmapped memory.
        assert!(!tags.contains_key(&AT_SYSINFO_EHDR));

        let execfn = tags[&AT_EXECFN] as usize;
        let recovered = unsafe {
            let mut out = Vec::new();
            let mut cursor = execfn as *const u8;
            while cursor.read() != 0 {
                out.push(cursor.read());
                cursor = cursor.add(1);
            }
            out
        };
        assert_eq!(recovered, b"/bin/echo");
    }

    #[test]
    fn defaults_page_size_when_zero() {
        let auxiliary = AuxiliaryValues::default();
        let image = build(&[], &[], &auxiliary);
        let mut home = Aligned([0; 8192]);
        let mut image = image;
        let base = place(&mut image, &mut home);
        let walked = unsafe { walk(image.stack_pointer(base)) };
        let tags: BTreeMap<u64, u64> = walked.auxv.iter().copied().collect();
        assert_eq!(tags[&AT_PAGESZ], 4096);
        // Absent inputs must not produce entries pointing nowhere.
        assert!(!tags.contains_key(&AT_PHDR));
        assert!(!tags.contains_key(&AT_BASE));
        assert!(!tags.contains_key(&AT_ENTRY));
        assert!(!tags.contains_key(&AT_EXECFN));
    }

    #[test]
    fn stack_pointer_is_aligned_for_every_argument_count() {
        // Both parities of argc and of envp, since each shifts the vector region by
        // one word and only an even total lands on 16 by accident.
        for argument_count in 0..5 {
            for environment_count in 0..5 {
                let arguments = strings(&["a", "bb", "ccc", "dddd", "eeeee"][..argument_count]);
                let environment =
                    strings(&["A=1", "B=2", "C=3", "D=4", "E=5"][..environment_count]);
                let image = build(&arguments, &environment, &AuxiliaryValues::default());

                assert_eq!(
                    image.stack_pointer_offset % 16,
                    0,
                    "offset unaligned for {argument_count} args, {environment_count} env"
                );
                for base in [0usize, 16, 4096, 0x7fff_0000] {
                    assert_eq!(
                        image.stack_pointer(base) % 16,
                        0,
                        "sp unaligned at base {base:#x}"
                    );
                }

                let mut image = image;
                let mut home = Aligned([0; 8192]);
                let base = place(&mut image, &mut home);
                let stack_pointer = image.stack_pointer(base);
                assert_eq!(stack_pointer % 16, 0, "placed sp unaligned");

                let walked = unsafe { walk(stack_pointer) };
                assert_eq!(walked.arguments.len(), argument_count);
                assert_eq!(walked.environment.len(), environment_count);
            }
        }
    }

    #[test]
    fn random_is_present_inside_the_block_and_varies() {
        let image = build(&[], &[], &AuxiliaryValues::default());
        let block_length = image.bytes.len();

        let mut image = image;
        let mut home = Aligned([0; 8192]);
        let base = place(&mut image, &mut home);
        let walked = unsafe { walk(image.stack_pointer(base)) };
        let tags: BTreeMap<u64, u64> = walked.auxv.iter().copied().collect();

        let random = *tags.get(&AT_RANDOM).expect("AT_RANDOM must be emitted");
        assert_ne!(random, 0, "glibc dereferences AT_RANDOM unconditionally");
        let offset = random as usize - base;
        assert!(
            offset + 16 <= block_length,
            "AT_RANDOM's 16 bytes must lie inside the block"
        );

        let first = home.0[offset..offset + 16].to_vec();

        let mut second_image = build(&[], &[], &AuxiliaryValues::default());
        let mut second_home = Aligned([0; 8192]);
        let second_base = place(&mut second_image, &mut second_home);
        let second_walked = unsafe { walk(second_image.stack_pointer(second_base)) };
        let second_tags: BTreeMap<u64, u64> = second_walked.auxv.iter().copied().collect();
        let second_offset = second_tags[&AT_RANDOM] as usize - second_base;
        let second = second_home.0[second_offset..second_offset + 16].to_vec();

        assert_ne!(first, second, "two builds must not share a canary seed");
    }

    #[test]
    fn auxv_ends_at_at_null() {
        let image = build(
            &strings(&["prog"]),
            &strings(&["K=V"]),
            &AuxiliaryValues::default(),
        );
        let mut image = image;
        let mut home = Aligned([0; 8192]);
        let base = place(&mut image, &mut home);
        let walked = unsafe { walk(image.stack_pointer(base)) };

        let (last_tag, last_value) = *walked.auxv.last().expect("auxv is never empty");
        assert_eq!(last_tag, AT_NULL);
        assert_eq!(last_value, 0);
        assert_eq!(
            walked
                .auxv
                .iter()
                .filter(|(tag, _)| *tag == AT_NULL)
                .count(),
            1,
            "nothing may follow the terminator"
        );
    }

    #[test]
    fn interior_nul_is_truncated_without_shifting_the_layout() {
        let arguments = vec![
            "keep".to_string(),
            "before\0after".to_string(),
            "tail".to_string(),
        ];
        let mut image = build(
            &arguments,
            &strings(&["E=v\0hidden"]),
            &AuxiliaryValues::default(),
        );
        let mut home = Aligned([0; 8192]);
        let base = place(&mut image, &mut home);
        let walked = unsafe { walk(image.stack_pointer(base)) };

        // argc still counts the truncated argument, and the ones after it survive.
        assert_eq!(walked.arguments.len(), 3);
        assert_eq!(walked.arguments[0], b"keep");
        assert_eq!(walked.arguments[1], b"before");
        assert_eq!(walked.arguments[2], b"tail");
        assert_eq!(walked.environment.len(), 1);
        assert_eq!(walked.environment[0], b"E=v");
    }
}
