//! Shared admission policy for init's immutable executable-image sections.
//! These are optional cache limits, not wire-format or executable-size limits.

// Includes the small compression/metadata libraries repeatedly loaded by dpkg.
pub const MIN_SIZE: usize = 32 * 1024;
pub const MAX_SIZE: usize = 64 * 1024 * 1024;

pub fn eligible(length: usize) -> bool {
    (MIN_SIZE..=MAX_SIZE).contains(&length)
}
