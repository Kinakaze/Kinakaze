//! One-use activation of a worker parked before RuntimeOpenV1.
use serde::{Deserialize, Serialize};

pub const CAPACITY: usize = 65_536;
const MAGIC: &[u8; 8] = b"KZEXEC01";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Worker {
    pub handles: [u64; 3], // process, activation event, control mapping
    pub pid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    pub executable: String,
    pub cwd: String,
}

impl Launch {
    fn valid(&self) -> bool {
        [&self.executable, &self.cwd].iter().all(|path| {
            !path.contains('\0') && path.len() <= 16_384 && std::path::Path::new(path).is_absolute()
        })
    }

    pub fn encode(&self) -> Option<Vec<u8>> {
        if !self.valid() {
            return None;
        }
        let payload = serde_json::to_vec(self).ok()?;
        if payload.len() > CAPACITY - 16 {
            return None;
        }
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&payload);
        Some(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 16 || &bytes[..8] != MAGIC || bytes[12..16] != [0; 4] {
            return None;
        }
        let length = u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize;
        if length > CAPACITY - 16 {
            return None;
        }
        let launch: Self = serde_json::from_slice(bytes.get(16..16 + length)?).ok()?;
        launch.valid().then_some(launch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activation_is_bounded_and_keeps_unicode_paths() {
        let root = std::env::current_dir().unwrap();
        let launch = Launch {
            executable: root.join("测试 executable").to_str().unwrap().into(),
            cwd: root.to_str().unwrap().into(),
        };
        let frame = launch.encode().unwrap();
        assert_eq!(Launch::decode(&frame), Some(launch.clone()));
        for length in 0..frame.len() {
            assert!(Launch::decode(&frame[..length]).is_none());
        }
        let mut invalid = frame.clone();
        invalid[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Launch::decode(&invalid).is_none());
        invalid = frame;
        invalid[12] = 1;
        assert!(Launch::decode(&invalid).is_none());
        for path in ["relative".into(), "\0".into(), "x".repeat(CAPACITY)] {
            assert!(
                Launch {
                    executable: path,
                    ..launch.clone()
                }
                .encode()
                .is_none()
            );
        }
    }
}
