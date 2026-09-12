//! Describe the hosted X output's refresh interval using the primary scanout.
use super::{Screen, XRRModeInfo};
use windows_sys::Win32::Devices::Display::*;

fn primary_rate() -> Option<(u32, u32)> {
    // Topology may change between sizing and reading the arrays.
    for _ in 0..3 {
        let (mut paths, mut modes) = (0, 0);
        if unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths, &mut modes) }
            != 0
            || paths == 0
            || paths > 4096
            || modes > 16384
        {
            return None;
        }
        let mut path_info = vec![DISPLAYCONFIG_PATH_INFO::default(); paths as usize];
        let mut mode_info = vec![DISPLAYCONFIG_MODE_INFO::default(); modes as usize];
        let status = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut paths,
                path_info.as_mut_ptr(),
                &mut modes,
                mode_info.as_mut_ptr(),
                core::ptr::null_mut(),
            )
        };
        if status == 122 {
            continue;
        } // ERROR_INSUFFICIENT_BUFFER
        if status != 0 {
            return None;
        }
        for path in &path_info[..paths as usize] {
            let index = unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize;
            let Some(source) = mode_info.get(index).filter(|_| index < modes as usize) else {
                continue;
            };
            if source.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
                continue;
            }
            let source = unsafe { source.Anonymous.sourceMode };
            if source.position.x != 0 || source.position.y != 0 {
                continue;
            }
            let rate = path.targetInfo.refreshRate;
            if rate.Numerator > 0 && rate.Denominator > 0 {
                return Some((rate.Numerator, rate.Denominator));
            }
        }
        return None;
    }
    None
}

pub(super) fn apply(screen: Screen, mode: &mut XRRModeInfo) {
    let fallback = if screen.rate > 1 {
        screen.rate as u32
    } else {
        60
    };
    let (numerator, denominator) = primary_rate().unwrap_or((fallback, 1));
    // This is a virtual X screen, not a programmable physical connector.
    // Its logical totals need no blanking interval; preserve the host's exact
    // rational refresh rate instead of exposing all-zero, unusable timings.
    mode.hSyncStart = screen.width;
    mode.hSyncEnd = screen.width;
    mode.hTotal = screen.width;
    mode.vSyncStart = screen.height;
    mode.vSyncEnd = screen.height;
    mode.vTotal = screen.height;
    mode.dotClock = (u64::from(screen.width) * u64::from(screen.height) * u64::from(numerator)
        + u64::from(denominator) / 2)
        / u64::from(denominator);
}
