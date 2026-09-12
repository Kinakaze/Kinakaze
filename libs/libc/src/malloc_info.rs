//! glibc allocator statistics, computed from the actual guest arena on demand.
#[repr(C)]
#[derive(Default)]
pub struct Mallinfo2 {
    pub arena: usize,
    pub ordblks: usize,
    pub smblks: usize,
    pub hblks: usize,
    pub hblkhd: usize,
    pub usmblks: usize,
    pub fsmblks: usize,
    pub uordblks: usize,
    pub fordblks: usize,
    pub keepcost: usize,
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_mallinfo2() -> Mallinfo2 {
    let stats = kinakaze_alloc::guest::statistics();
    Mallinfo2 {
        arena: stats.arena,
        ordblks: stats.free_blocks,
        uordblks: stats.used,
        fordblks: stats.free,
        keepcost: stats.tail,
        ..Mallinfo2::default()
    }
}

#[repr(C)]
pub struct Mallinfo {
    values: [i32; 10],
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_mallinfo() -> Mallinfo {
    let info = kinakaze_abi_mallinfo2();
    Mallinfo {
        values: [
            info.arena,
            info.ordblks,
            info.smblks,
            info.hblks,
            info.hblkhd,
            info.usmblks,
            info.fsmblks,
            info.uordblks,
            info.fordblks,
            info.keepcost,
        ]
        .map(|v| v as i32),
    }
}
