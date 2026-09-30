//! Compact, pointer-free catalog shared only by an authenticated init session.
use super::*;
use kinakaze_v2_host_win::{ReadOnlySection, ReadOnlySectionView, RemoteTransfer};
const MAGIC: &[u8; 8] = b"KZCAT001";
pub const LIMIT: usize = 2 * 1024 * 1024;
const MAX_IMAGES: usize = 256;

pub struct CatalogSnapshot {
    section: ReadOnlySection,
    catalog: ModuleCatalog,
}
fn put_word(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
fn put_text(bytes: &mut Vec<u8>, value: &str) -> Result<()> {
    put_word(
        bytes,
        value
            .len()
            .try_into()
            .map_err(|_| invalid("catalog string too long"))?,
    );
    bytes.extend_from_slice(value.as_bytes());
    Ok(())
}
impl CatalogSnapshot {
    pub fn new(directory: &Path, catalog: ModuleCatalog) -> Result<Self> {
        if catalog.images.len() > MAX_IMAGES {
            return Err(invalid("too many native images to share"));
        }
        let mut bytes = MAGIC.to_vec();
        put_text(
            &mut bytes,
            directory
                .to_str()
                .ok_or_else(|| invalid("non-UTF8 native directory"))?,
        )?;
        put_word(&mut bytes, catalog.images.len() as u32);
        for image in &catalog.images {
            let module = catalog
                .modules
                .iter()
                .find(|module| Arc::ptr_eq(&module.image, image));
            let filename = image
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| invalid("invalid native filename"))?;
            put_text(&mut bytes, filename)?;
            put_text(
                &mut bytes,
                module.map_or("", |module| module.native_name.as_str()),
            )?;
            let flags = module.map_or(0, |module| {
                1 | u32::from(module.id == 0) * 2
                    | u32::from(image.has_layout) * 4
                    | u32::from(module.lifecycle == ModuleLifecycle::RuntimeApiV1) * 8
            });
            put_word(&mut bytes, flags);
            put_word(&mut bytes, module.map_or(0, |module| module.id));
            put_word(&mut bytes, image.exports.len() as u32);
            for symbol in image.exports() {
                put_text(&mut bytes, symbol.name)?;
                bytes.push(match symbol.object {
                    None => 0,
                    Some(false) => 1,
                    Some(true) => 2,
                });
            }
        }
        if bytes.len() > LIMIT {
            return Err(invalid("native catalog exceeds sharing limit"));
        }
        Ok(Self {
            section: ReadOnlySection::new(&bytes)?,
            catalog,
        })
    }
    pub fn length(&self) -> usize {
        self.section.length()
    }
    pub fn image_count(&self) -> usize {
        self.catalog.images.len()
    }
    pub fn transfer(&self, transfer: &mut RemoteTransfer) -> Result<()> {
        self.section.transfer(transfer)?;
        for image in &self.catalog.images {
            match &image.file {
                NativeBytes::File(file) => file.transfer_pin(transfer)?,
                NativeBytes::Snapshot { .. } => {
                    return Err(invalid("cannot reshare a borrowed catalog"));
                }
            }
        }
        Ok(())
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .cursor
            .checked_add(length)
            .ok_or_else(|| invalid("catalog range overflow"))?;
        let bytes = self
            .bytes
            .get(self.cursor..end)
            .ok_or_else(|| invalid("truncated native catalog"))?;
        self.cursor = end;
        Ok(bytes)
    }
    fn word(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn text(&mut self, limit: usize) -> Result<(u32, &'a str)> {
        let length = self.word()? as usize;
        if length > limit {
            return Err(invalid("native catalog string exceeds limit"));
        }
        let offset = self.cursor as u32;
        let text = std::str::from_utf8(self.take(length)?)
            .map_err(|_| invalid("non-UTF8 catalog string"))?;
        if text.contains('\0') {
            return Err(invalid("NUL in catalog string"));
        }
        Ok((offset, text))
    }
}
impl ModuleCatalog {
    /// # Safety
    /// The immutable section was built by this session's authenticated init,
    /// with exactly one live deny-write/delete file pin per encoded image.
    pub unsafe fn from_snapshot(directory: &Path, view: ReadOnlySectionView) -> Result<Self> {
        let view = Arc::new(view);
        let mut reader = Reader {
            bytes: view.as_slice(),
            cursor: 0,
        };
        if reader.take(8)? != MAGIC || reader.bytes.len() > LIMIT {
            return Err(invalid("invalid catalog header"));
        }
        let (_, source) = reader.text(32768)?;
        if Path::new(source) != directory {
            return Err(invalid("catalog directory mismatch"));
        }
        let count = reader.word()? as usize;
        if count == 0 || count > MAX_IMAGES || count != view.pin_count() {
            return Err(invalid("catalog pin count mismatch"));
        }
        let mut set = Self {
            modules: Vec::new(),
            shared_libraries: Vec::new(),
            images: Vec::with_capacity(count),
        };
        let mut names = HashSet::new();
        let mut ids = HashSet::new();
        for pin in 0..count {
            let (_, filename) = reader.text(4096)?;
            crate::module::validate_filename(filename)?;
            if !names.insert(filename.to_ascii_lowercase()) {
                return Err(invalid("duplicate catalog image"));
            }
            let (_, native_name) = reader.text(4096)?;
            let flags = reader.word()?;
            let id = reader.word()?;
            let exports = reader.word()? as usize;
            if flags & !15 != 0
                || exports > 65536
                || flags & 1 == 0
                    && (flags != 0 || id != 0 || exports != 0 || !native_name.is_empty())
            {
                return Err(invalid("invalid catalog image flags"));
            }
            let mut indexed = Vec::with_capacity(exports);
            let mut previous = None;
            for _ in 0..exports {
                let (start, name) = reader.text(4096)?;
                if name.is_empty() || previous.is_some_and(|old: &str| old >= name) {
                    return Err(invalid("unsorted catalog exports"));
                }
                previous = Some(name);
                let object = match reader.take(1)?[0] {
                    0 => None,
                    1 => Some(false),
                    2 => Some(true),
                    _ => return Err(invalid("invalid catalog export kind")),
                };
                indexed.push(IndexedExport {
                    start,
                    length: name.len() as u32,
                    object,
                });
            }
            let image = Arc::new(NativeImage {
                path: directory.join(filename),
                file: NativeBytes::Snapshot {
                    view: view.clone(),
                    pin,
                },
                exports: indexed,
                has_layout: flags & 4 != 0,
            });
            set.images.push(image.clone());
            if flags & 1 == 0 {
                set.shared_libraries.push(filename.to_owned());
            } else {
                if !ids.insert(id)
                    || (flags & 2 != 0) != (id == 0)
                    || id != 0 && id != module_id(filename)
                    || id == 0 && flags & 8 != 0
                {
                    return Err(invalid("invalid catalog module identity"));
                }
                set.modules.push(DiscoveredModule {
                    id,
                    soname: filename.to_owned(),
                    native_name: native_name.to_owned(),
                    lifecycle: if flags & 8 != 0 {
                        ModuleLifecycle::RuntimeApiV1
                    } else {
                        ModuleLifecycle::None
                    },
                    image,
                });
            }
        }
        if reader.cursor != reader.bytes.len()
            || set.modules.is_empty()
            || set.modules.len() > crate::module::MAX_MODULES
            || !ids.contains(&0)
        {
            return Err(invalid("invalid catalog module set"));
        }
        set.modules.sort_by_key(|module| module.id);
        Ok(set)
    }
}
