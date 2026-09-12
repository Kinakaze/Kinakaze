//! POSIX zone data. Publication happens only on a time conversion or tzset call.
//! The small fork record preserves values and COPY redirects without querying
//! the clock or environment in the freshly loaded child runtime.

use super::{
    Rules, SECONDS_PER_DAY, ZoneAbbrev, civil_from_days, published_abbrev, zone_information_for,
};
use crate::copied::CopiedValue;
use core::ffi::c_char;
use std::sync::Mutex;

#[unsafe(no_mangle)]
pub static kinakaze_abi_timezone: CopiedValue<i64> = CopiedValue::new(0);
#[unsafe(no_mangle)]
pub static kinakaze_abi_daylight: CopiedValue<i32> = CopiedValue::new(0);
#[unsafe(no_mangle)]
pub static kinakaze_abi_tzname: CopiedValue<[*mut c_char; 2]> =
    CopiedValue::new([c"GMT".as_ptr().cast_mut(); 2]);

static PUBLICATION: Mutex<()> = Mutex::new(());

pub(super) fn publish(rules: Rules, utc: i64) {
    let (west, daylight, standard, summer) = match rules {
        Rules::Fixed { offset, abbrev } => (-offset, false, abbrev, abbrev),
        Rules::Posix {
            standard,
            standard_abbrev,
            daylight_abbrev,
            ..
        } => (-standard, true, standard_abbrev, daylight_abbrev),
        Rules::Host => {
            let (year, _, _) = civil_from_days(utc.div_euclid(SECONDS_PER_DAY));
            match zone_information_for(year) {
                Some(info) => {
                    let west = (i64::from(info.bias) + i64::from(info.standard_bias)) * 60;
                    let daylight = info.standard_date.month != 0 && info.daylight_date.month != 0;
                    let standard = ZoneAbbrev::numeric(-west);
                    let summer = if daylight {
                        ZoneAbbrev::numeric(
                            -(i64::from(info.bias) + i64::from(info.daylight_bias)) * 60,
                        )
                    } else {
                        standard
                    };
                    (west, daylight, standard, summer)
                }
                None => (0, false, ZoneAbbrev::new("UTC"), ZoneAbbrev::new("UTC")),
            }
        }
    };
    let names = [
        published_abbrev(standard).cast_mut(),
        published_abbrev(summer).cast_mut(),
    ];
    let _guard = PUBLICATION
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    // POSIX forbids concurrent direct guest access during timezone publication.
    unsafe {
        kinakaze_abi_timezone.set(west);
        kinakaze_abi_daylight.set(i32::from(daylight));
        kinakaze_abi_tzname.set(names);
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ForkState {
    magic: [u8; 8],
    west: i64,
    daylight: i32,
    reserved: u32,
    names: [usize; 2],
    targets: [usize; 3],
}
const _: () = assert!(core::mem::size_of::<ForkState>() == 64);

unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    let size = core::mem::size_of::<ForkState>();
    if output.is_null() {
        return size as isize;
    }
    if capacity < size {
        return -22;
    }
    let _guard = PUBLICATION
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    unsafe {
        output.cast::<ForkState>().write_unaligned(ForkState {
            magic: *b"CRYTZF01",
            west: kinakaze_abi_timezone.get(),
            daylight: kinakaze_abi_daylight.get(),
            reserved: 0,
            names: kinakaze_abi_tzname.get().map(|pointer| pointer as usize),
            targets: [
                kinakaze_abi_timezone.target() as usize,
                kinakaze_abi_daylight.target() as usize,
                kinakaze_abi_tzname.target() as usize,
            ],
        });
    }
    size as isize
}

unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length != core::mem::size_of::<ForkState>() {
        return 22;
    }
    let state = unsafe { input.cast::<ForkState>().read_unaligned() };
    if state.magic != *b"CRYTZF01" || state.reserved != 0 {
        return 22;
    }
    unsafe {
        kinakaze_abi_timezone.redirect(state.targets[0] as *mut i64);
        kinakaze_abi_daylight.redirect(state.targets[1] as *mut i32);
        kinakaze_abi_tzname.redirect(state.targets[2] as *mut [*mut c_char; 2]);
        kinakaze_abi_timezone.set(state.west);
        kinakaze_abi_daylight.set(state.daylight);
        kinakaze_abi_tzname.set(state.names.map(|pointer| pointer as *mut c_char));
    }
    0
}

extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 30,
        key: 0x4352_5954_5a46_3031,
        prepare: None,
        snapshot: Some(snapshot),
        parent: None,
        child: Some(child),
    });
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static REGISTER: extern "C" fn() = register;
