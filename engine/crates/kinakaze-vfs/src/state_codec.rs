//! Bounded little-endian encoding for pagefile-backed VFS state.
use crate::EIO;
pub(crate) fn word(out: &mut impl Extend<u8>, value: u64) {
    out.extend(value.to_le_bytes());
}
pub(crate) fn bytes(out: &mut impl Extend<u8>, value: &[u8]) {
    word(out, value.len() as u64);
    out.extend(value.iter().copied());
}
pub(crate) struct Reader<'a>(pub &'a [u8]);
impl<'a> Reader<'a> {
    pub fn word(&mut self) -> Result<u64, i32> {
        let value = self.0.get(..8).ok_or(EIO)?;
        self.0 = &self.0[8..];
        Ok(u64::from_le_bytes(value.try_into().unwrap()))
    }
    pub fn bytes(&mut self) -> Result<&'a [u8], i32> {
        let len = usize::try_from(self.word()?).map_err(|_| EIO)?;
        let value = self.0.get(..len).ok_or(EIO)?;
        self.0 = &self.0[len..];
        Ok(value)
    }
    pub fn text(&mut self) -> Result<String, i32> {
        String::from_utf8(self.bytes()?.to_vec()).map_err(|_| EIO)
    }
    pub fn end(self) -> Result<(), i32> {
        if self.0.is_empty() { Ok(()) } else { Err(EIO) }
    }
}
