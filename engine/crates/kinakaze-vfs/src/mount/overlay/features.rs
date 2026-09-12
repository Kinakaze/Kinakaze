//! Persisted feature bits belong to the overlay superblock, not its attachments.
pub const VOLATILE: u64 = 1 << 39;
pub const INDEX: u64 = 1 << 42;
pub const METACOPY: u64 = 1 << 43;
pub const REDIRECT: u64 = 1 << 44;
pub const FOLLOW: u64 = 1 << 45;
pub const XINO: u64 = 1 << 46;
pub const XINO_AUTO: u64 = 1 << 47;
pub const NFS_EXPORT: u64 = 1 << 48;
pub const UUID_OFF: u64 = 1 << 49;
pub const UUID_ON: u64 = 1 << 50;
pub const UUID_NULL: u64 = 1 << 51;
pub const VERITY: u64 = 1 << 52;
pub const VERITY_REQUIRE: u64 = 1 << 53;
pub const DATA_COUNT_MASK: u64 = 0x1ff << 54;
pub const FSYNC_STRICT: u64 = 1 << 63;
pub fn data_count(flags: u64) -> usize {
    ((flags & DATA_COUNT_MASK) >> 54) as usize
}
pub const ALL: u64 = VOLATILE
    | INDEX
    | METACOPY
    | REDIRECT
    | FOLLOW
    | XINO
    | XINO_AUTO
    | NFS_EXPORT
    | UUID_OFF
    | UUID_ON
    | UUID_NULL
    | VERITY
    | VERITY_REQUIRE
    | DATA_COUNT_MASK
    | FSYNC_STRICT;

pub(crate) fn validate(mut flags: u64, writable: bool) -> Result<u64, i32> {
    if flags & VOLATILE != 0 && (flags & FSYNC_STRICT != 0 || !writable)
        || flags & REDIRECT != 0 && flags & FOLLOW == 0
        || flags & METACOPY != 0 && flags & (REDIRECT | FOLLOW) != REDIRECT | FOLLOW
        || flags & VERITY != 0 && flags & METACOPY == 0
        || flags & VERITY_REQUIRE != 0 && flags & VERITY == 0
        || flags & XINO != 0 && flags & XINO_AUTO != 0
        || (flags & (UUID_OFF | UUID_ON | UUID_NULL)).count_ones() > 1
        || flags & super::USER_XATTR_FLAG != 0 && flags & (METACOPY | FOLLOW) != 0
        || flags & NFS_EXPORT != 0
            && (flags & METACOPY != 0
                || data_count(flags) != 0
                || writable && flags & INDEX == 0
                || !writable && flags & FOLLOW != 0)
    {
        return Err(crate::EINVAL);
    }
    if !writable {
        flags &= !INDEX;
        if flags & UUID_ON != 0 {
            flags = (flags & !UUID_ON) | UUID_NULL;
        }
    }
    Ok(flags)
}

pub fn display(flags: u64) -> String {
    format!(
        "index={},metacopy={},redirect_dir={},xino={},nfs_export={},uuid={},verity={},fsync={}{}",
        if flags & INDEX != 0 { "on" } else { "off" },
        if flags & METACOPY != 0 { "on" } else { "off" },
        if flags & REDIRECT != 0 {
            "on"
        } else if flags & FOLLOW != 0 {
            "follow"
        } else {
            "nofollow"
        },
        if flags & XINO_AUTO != 0 {
            "auto"
        } else if flags & XINO != 0 {
            "on"
        } else {
            "off"
        },
        if flags & NFS_EXPORT != 0 { "on" } else { "off" },
        if flags & UUID_OFF != 0 {
            "off"
        } else if flags & UUID_ON != 0 {
            "on"
        } else if flags & UUID_NULL != 0 {
            "null"
        } else {
            "auto"
        },
        if flags & VERITY_REQUIRE != 0 {
            "require"
        } else if flags & VERITY != 0 {
            "on"
        } else {
            "off"
        },
        if flags & VOLATILE != 0 {
            "volatile"
        } else if flags & FSYNC_STRICT != 0 {
            "strict"
        } else {
            "auto"
        },
        if flags & super::USER_XATTR_FLAG != 0 {
            ",userxattr"
        } else {
            ""
        }
    )
}
