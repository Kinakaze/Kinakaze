//! Value-only half of the synchronous-fault provider ABI. Keep native exception
//! continuation separate from Linux signal numbers.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Action {
    Unhandled,
    Retry,
    Signal { number: i32, code: i32 },
}

pub(crate) fn classify(exception: u32, address: u64, mapping_result: i32) -> Action {
    let (number, code) = match exception {
        0xc000_0005 => match mapping_result {
            // The provider may request retry only after the mapping transaction
            // has completed or rolled back and this access is permitted again.
            -1 => return Action::Retry,
            7 => (7, 2), // SIGBUS / BUS_ADRERR
            _ => (11, if address < 0x1_0000 { 1 } else { 2 }),
        },
        0xc000_001d => (4, 1), // SIGILL / ILL_ILLOPC
        0xc000_0094 => (8, 1), // SIGFPE / FPE_INTDIV
        0xc000_0095 => (8, 2),
        0xc000_008e => (8, 3),
        0xc000_0091 => (8, 4),
        0xc000_0093 => (8, 5),
        0xc000_008f => (8, 6),
        0xc000_0090 => (8, 7),
        0xc000_008d => (8, 8),
        _ => return Action::Unhandled,
    };
    Action::Signal { number, code }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_completed_mapping_requests_retry() {
        assert_eq!(classify(0xc000_0005, 0x20000, -1), Action::Retry);
        for result in [0, 1, -2, i32::MIN] {
            assert_eq!(
                classify(0xc000_0005, 0x20000, result),
                Action::Signal {
                    number: 11,
                    code: 2
                }
            );
        }
        assert_eq!(
            classify(0xc000_001d, 0x20000, -1),
            Action::Signal { number: 4, code: 1 }
        );
    }

    #[test]
    fn integrity_and_ordinary_access_faults_remain_distinct() {
        assert_eq!(
            classify(0xc000_0005, 0x20000, 7),
            Action::Signal { number: 7, code: 2 }
        );
        assert_eq!(
            classify(0xc000_0005, 0, 0),
            Action::Signal {
                number: 11,
                code: 1
            }
        );
        assert_eq!(classify(0x8000_0003, 0, 7), Action::Unhandled);
    }
}
