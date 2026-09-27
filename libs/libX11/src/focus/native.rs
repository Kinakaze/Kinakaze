//! Native focus transitions over the existing shared X11 focus record.
//! Kept independent of HWNDs so ordering can also be tested without a desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NativeFocus {
    pub window: usize,
    pub revert: i32,
    pub time: u32,
}

impl NativeFocus {
    pub fn decode(record: &[u8]) -> Option<Self> {
        (record.len() == 16).then(|| Self {
            window: u64::from_le_bytes(record[..8].try_into().unwrap()) as usize,
            revert: i32::from_le_bytes(record[8..12].try_into().unwrap()),
            time: u32::from_le_bytes(record[12..16].try_into().unwrap()),
        })
    }

    pub fn encode(self) -> Vec<u8> {
        let mut record = (self.window as u64).to_le_bytes().to_vec();
        record.extend_from_slice(&self.revert.to_le_bytes());
        record.extend_from_slice(&self.time.to_le_bytes());
        record
    }

    pub fn changed(self, window: usize, gained: bool, time: u32) -> Option<Self> {
        // Another process may drain its gain before this one drains its loss.
        if (self.time != 0 && (time.wrapping_sub(self.time) as i32) < 0)
            || (!gained && self.window != window)
        {
            return None;
        }
        Some(Self {
            window: if gained { window } else { 0 },
            // An acknowledgement of XSetInputFocus retains its revert policy.
            revert: if gained && self.window == window {
                self.revert
            } else {
                0
            },
            time,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_and_loss_replace_pointer_root_and_clear_focus() {
        let root = NativeFocus {
            window: 1,
            revert: 0,
            time: 0,
        };
        let focused = root.changed(42, true, 100).unwrap();
        assert_eq!(focused.window, 42);
        let unfocused = focused.changed(42, false, 101).unwrap();
        assert_eq!(unfocused.window, 0);
        assert_eq!(unfocused.changed(43, true, 101).unwrap().window, 43);
    }

    #[test]
    fn delayed_loss_cannot_clear_the_next_window_even_in_the_same_tick() {
        let current = NativeFocus {
            window: 42,
            revert: 0,
            time: 100,
        };
        assert!(current.changed(41, false, 99).is_none());
        assert!(current.changed(41, false, 100).is_none());
        assert!(current.changed(41, false, 101).is_none());
    }

    #[test]
    fn explicit_focus_keeps_revert_policy_and_rejects_older_activation() {
        let explicit = NativeFocus {
            window: 42,
            revert: 2,
            time: 100,
        };
        assert!(explicit.changed(41, true, 99).is_none());
        assert_eq!(explicit.changed(42, true, 100), Some(explicit));
        assert_eq!(explicit.changed(43, true, 101).unwrap().revert, 0);
    }

    #[test]
    fn timestamps_remain_ordered_across_server_clock_wraparound() {
        let before = NativeFocus {
            window: 42,
            revert: 0,
            time: u32::MAX - 1,
        };
        let after = before.changed(43, true, 1).unwrap();
        assert!(after.changed(42, true, u32::MAX).is_none());
    }

    #[test]
    fn record_matches_the_existing_shared_focus_layout() {
        let focus = NativeFocus {
            window: 0x12345,
            revert: 2,
            time: 123,
        };
        assert_eq!(NativeFocus::decode(&focus.encode()), Some(focus));
        assert_eq!(NativeFocus::decode(&[0; 15]), None);
    }
}
