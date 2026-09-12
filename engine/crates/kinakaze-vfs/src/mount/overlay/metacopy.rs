//! Linux metacopy xattr format and fs-verity digest validation.
use super::*;

impl Node {
    pub(super) fn metacopy_marker(&self) -> Result<Option<Vec<u8>>, i32> {
        if self.flags() & features::VERITY == 0 {
            return Ok(Some(Vec::new()));
        }
        let descriptor = crate::fs::verity::descriptor(self.data_object().raw())?;
        let Some(descriptor) = descriptor else {
            return Ok(if self.flags() & features::VERITY_REQUIRE != 0 {
                None
            } else {
                Some(Vec::new())
            });
        };
        let digest = descriptor.digest();
        let mut bytes = vec![0, (4 + digest.len()) as u8, 0, descriptor.algorithm() as u8];
        bytes.extend_from_slice(&digest);
        Ok(Some(bytes))
    }

    pub(super) fn verify_metacopy(&self) -> Result<(), i32> {
        if !self.is_metacopy() {
            return Ok(());
        }
        for entry in self.entries.iter().filter(|entry| entry.metacopy.is_some()) {
            let marker = entry.metacopy.as_ref().unwrap();
            let digest = if marker.is_empty() {
                None
            } else {
                if marker.len() < 4
                    || marker[0] != 0
                    || marker[1] as usize != marker.len()
                    || marker[2] != 0
                {
                    return Err(EIO);
                }
                match marker[3] {
                    0 if marker.len() == 4 => None,
                    1 if marker.len() == 36 => Some((1, &marker[4..])),
                    2 if marker.len() == 68 => Some((2, &marker[4..])),
                    _ => return Err(EIO),
                }
            };
            if self.flags() & features::VERITY == 0 {
                continue;
            }
            let Some((algorithm, digest)) = digest else {
                if self.flags() & features::VERITY_REQUIRE != 0 {
                    return Err(EIO);
                }
                continue;
            };
            let descriptor = crate::fs::verity::descriptor(self.data_object().raw())?.ok_or(EIO)?;
            if descriptor.algorithm() != algorithm || descriptor.digest() != digest {
                return Err(EIO);
            }
        }
        Ok(())
    }
}
