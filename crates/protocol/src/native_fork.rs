//! A bounded bootstrap description. No guest state or ambient handles are pooled.
use serde::{Deserialize, Serialize};

pub const MAGIC: u64 = 0x4352_5942_4f4f_5432;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Module {
    pub base: u64,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub tls_slots: u64,
    pub modules: Vec<Module>,
}

impl Spec {
    pub fn valid(&self) -> bool {
        !self.modules.is_empty()
            && self.modules.len() <= 128
            && self.modules.iter().map(|m| m.path.len()).sum::<usize>() <= 16_384
            && self.modules.iter().all(|m| {
                m.base != 0
                    && m.base <= isize::MAX as u64
                    && m.base % 65536 == 0
                    && !m.path.is_empty()
                    && m.path.len() <= 4096
                    && !m.path.contains('\0')
            })
            && self.modules.windows(2).all(|m| m[0].base < m[1].base)
    }

    pub fn manifest(&self) -> Option<Vec<u8>> {
        if !self.valid() {
            return None;
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&(self.modules.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&self.tls_slots.to_le_bytes());
        for module in &self.modules {
            let path: Vec<u16> = module.path.encode_utf16().chain(Some(0)).collect();
            bytes.extend_from_slice(&module.base.to_le_bytes());
            bytes.extend_from_slice(&(path.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
            for unit in path {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            bytes.resize(bytes.len().next_multiple_of(8), 0);
        }
        let length = bytes.len() as u64;
        bytes[8..16].copy_from_slice(&length.to_le_bytes());
        Some(bytes)
    }
}

/// Four exclusive capabilities duplicated into the requesting native process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Worker {
    pub handles: [u64; 4], // process, main thread, ready event, activation event
    pub pid: u32,
    pub tid: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifest_is_bounded_and_preserves_layout_and_unicode_paths() {
        let mut spec = Spec {
            tls_slots: 0x57,
            modules: vec![Module {
                base: 0x10000,
                path: "C:\\测试\\libc.so".into(),
            }],
        };
        let bytes = spec.manifest().unwrap();
        let word = |i| u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
        assert_eq!(word(0), MAGIC);
        assert_eq!(word(8), bytes.len() as u64);
        assert_eq!(word(24), spec.tls_slots);
        assert_eq!(word(32), 0x10000);
        let units = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
        let path: Vec<u16> = bytes[48..48 + units * 2]
            .chunks_exact(2)
            .map(|u| u16::from_le_bytes(u.try_into().unwrap()))
            .collect();
        assert_eq!(path.last(), Some(&0));
        assert_eq!(
            String::from_utf16(&path[..path.len() - 1]).unwrap(),
            spec.modules[0].path
        );
        spec.modules.push(spec.modules[0].clone());
        assert!(spec.manifest().is_none());
        spec.modules.pop();
        spec.modules[0].path.push('\0');
        assert!(spec.manifest().is_none());
        spec.modules[0].path = "x".repeat(4097);
        assert!(spec.manifest().is_none());
        spec.modules.clear();
        assert!(spec.manifest().is_none());
    }
}
