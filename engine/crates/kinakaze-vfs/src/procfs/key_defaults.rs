//! Explicit fixed-default key quota interfaces, matching the requested
//! write-discard policy for resource-limit discovery. They do not implement
//! keyrings or enforce quotas. No process-local mutable sysctl state is kept.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Limit {
    RootMaxKeys,
    RootMaxBytes,
    MaxKeys,
    MaxBytes,
}

impl Limit {
    pub(super) const ALL: [Self; 4] = [
        Self::RootMaxKeys,
        Self::RootMaxBytes,
        Self::MaxKeys,
        Self::MaxBytes,
    ];

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::RootMaxKeys => "root_maxkeys",
            Self::RootMaxBytes => "root_maxbytes",
            Self::MaxKeys => "maxkeys",
            Self::MaxBytes => "maxbytes",
        }
    }

    pub(super) fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|limit| limit.name() == name)
    }

    pub(super) fn read(self) -> Vec<u8> {
        // Linux security/keys/key.c defaults. These are configuration defaults,
        // not a claim that the Linux key retention service is implemented.
        match self {
            Self::RootMaxKeys => b"1000000\n".to_vec(),
            Self::RootMaxBytes => b"25000000\n".to_vec(),
            Self::MaxKeys => b"200\n".to_vec(),
            Self::MaxBytes => b"20000\n".to_vec(),
        }
    }

    pub(super) fn write(self, bytes: &[u8], offset: u64) -> Result<usize, i32> {
        if offset != 0 {
            return Err(crate::EINVAL);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| crate::EINVAL)?;
        let value = text.trim().parse::<u32>().map_err(|_| crate::EINVAL)?;
        if value == 0 || value > i32::MAX as u32 {
            return Err(crate::EINVAL);
        }
        // Deliberately discard the request; never report a changed quota.
        Ok(bytes.len())
    }
}
