//! Fork serializes loader metadata; each child owns fresh Rust collections.
//! Guest mappings and retained handle slots are adopted through runtime APIs.
use super::*;
use std::os::windows::ffi::{OsStrExt, OsStringExt};

const MAGIC: usize = 0x4c494e4b464f524b;

fn invalid() -> LinkError {
    LinkError::InvalidProvider("invalid linker fork state".into())
}

#[derive(Default)]
pub(super) struct Writer(pub Vec<u8>);
impl Writer {
    pub fn word(&mut self, value: usize) {
        self.0.extend_from_slice(&(value as u64).to_le_bytes());
    }
    pub fn bytes(&mut self, value: &[u8]) {
        self.word(value.len());
        self.0.extend_from_slice(value);
    }
    pub fn text(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }
    fn path(&mut self, value: &Path) {
        self.word(value.as_os_str().encode_wide().count());
        for unit in value.as_os_str().encode_wide() {
            self.0.extend_from_slice(&unit.to_le_bytes());
        }
    }
    fn paths(&mut self, values: &[PathBuf]) {
        self.word(values.len());
        for value in values {
            self.path(value);
        }
    }
    fn optional(&mut self, value: Option<usize>) {
        self.word(value.unwrap_or(usize::MAX));
    }
    fn provider(&mut self, value: Provider) {
        match value {
            Provider::Elf(id) => {
                self.word(0);
                self.word(id.0);
            }
            Provider::Dll(id) => {
                self.word(1);
                self.word(id);
            }
        }
    }
    pub fn handle(&mut self, value: RuntimeHandle) {
        match value {
            RuntimeHandle::Main => {
                self.word(0);
                self.word(0);
            }
            RuntimeHandle::Elf(id) => {
                self.word(1);
                self.word(id.0);
            }
            RuntimeHandle::Dll(id) => {
                self.word(2);
                self.word(id);
            }
        }
    }
}

pub(super) struct Reader<'a>(pub &'a [u8]);
impl<'a> Reader<'a> {
    pub fn take(&mut self, size: usize) -> Result<&'a [u8], LinkError> {
        if size > self.0.len() {
            return Err(invalid());
        }
        let (value, rest) = self.0.split_at(size);
        self.0 = rest;
        Ok(value)
    }
    pub fn word(&mut self) -> Result<usize, LinkError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()) as usize)
    }
    pub fn count(&mut self) -> Result<usize, LinkError> {
        let count = self.word()?;
        if count > self.0.len() / 8 {
            return Err(invalid());
        }
        Ok(count)
    }
    pub fn bytes(&mut self) -> Result<&'a [u8], LinkError> {
        let size = self.word()?;
        self.take(size)
    }
    pub fn text(&mut self) -> Result<String, LinkError> {
        String::from_utf8(self.bytes()?.to_vec()).map_err(|_| invalid())
    }
    fn path(&mut self) -> Result<PathBuf, LinkError> {
        let size = self.word()?.checked_mul(2).ok_or_else(invalid)?;
        let units: Vec<_> = self
            .take(size)?
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        Ok(std::ffi::OsString::from_wide(&units).into())
    }
    fn paths(&mut self) -> Result<Vec<PathBuf>, LinkError> {
        (0..self.count()?).map(|_| self.path()).collect()
    }
    fn optional(&mut self) -> Result<Option<usize>, LinkError> {
        let v = self.word()?;
        Ok((v != usize::MAX).then_some(v))
    }
    pub fn boolean(&mut self) -> Result<bool, LinkError> {
        match self.word()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid()),
        }
    }
    fn provider(&mut self) -> Result<Provider, LinkError> {
        let kind = self.word()?;
        let id = self.word()?;
        match kind {
            0 => Ok(Provider::Elf(ObjectId(id))),
            1 => Ok(Provider::Dll(id)),
            _ => Err(invalid()),
        }
    }
    pub fn handle(&mut self) -> Result<RuntimeHandle, LinkError> {
        let kind = self.word()?;
        let id = self.word()?;
        match kind {
            0 if id == 0 => Ok(RuntimeHandle::Main),
            1 => Ok(RuntimeHandle::Elf(ObjectId(id))),
            2 => Ok(RuntimeHandle::Dll(id)),
            _ => Err(invalid()),
        }
    }
}

impl Linker {
    pub fn snapshot_fork(&self) -> Result<Vec<u8>, LinkError> {
        let mut out = Writer::default();
        out.word(MAGIC);
        out.word(4);
        out.path(
            self.registry
                .restore_source
                .as_deref()
                .ok_or_else(invalid)?,
        );
        out.path(&self.paths.host_directory);
        out.paths(&self.paths.extra);
        out.word(self.paths.process_namespace as usize);
        out.paths(&self.library_paths);
        out.paths(&self.system_paths);
        for entry in self.c_allocator {
            out.word(entry);
        }
        out.word(self.objects.len());
        for object in &self.objects {
            out.text(&object.name);
            out.path(&object.path);
            for part in object.bytes.fork_parts() {
                out.word(part);
            }
            out.word(object.allocation as usize);
            out.word(object.allocation_len);
            out.word(object._mapping.fork_slot());
            out.0.extend_from_slice(&object.load_bias.to_le_bytes());
            out.word(object.symbol_count as usize);
            out.optional(object.tls_module);
            out.word(object.dependencies.len());
            for id in &object.dependencies {
                out.word(id.0);
            }
            for value in [
                object.initialized,
                object.preinitialized,
                object.nodelete,
                object.is_executable,
                object.self_relocating,
            ] {
                out.word(value as usize);
            }
            out.word(object.references);
        }
        out.word(self.provider_dependencies.len());
        for providers in &self.provider_dependencies {
            out.word(providers.len());
            for value in providers {
                out.provider(*value);
            }
        }
        out.word(self.scope.dlls.len());
        for dll in &self.scope.dlls {
            out.text(&dll.name);
            out.word(dll.base());
            out.word(dll.references);
            out.word(dll.nodelete as usize);
        }
        out.word(self.scope.order.len());
        for id in &self.scope.order {
            out.word(id.0);
        }
        out.word(self.scope.providers.len());
        for value in &self.scope.providers {
            out.provider(*value);
        }
        out.word(self.scope.globals.len());
        for value in &self.scope.globals {
            out.word(*value as usize);
        }
        out.word(self.builtins.len());
        for (name, address) in &self.builtins {
            out.text(name);
            out.word(*address);
        }
        out.word(self.pending_ifuncs.len());
        for value in &self.pending_ifuncs {
            out.word(value.target);
            out.word(value.resolver);
        }
        out.word(self.initialization_order.len());
        for id in &self.initialization_order {
            out.word(id.0);
        }
        out.word(self.finalizers.is_some() as usize);
        if let Some(pending) = &self.finalizers {
            out.word(pending.len());
            for address in pending {
                out.word(*address);
            }
        }
        out.word(matches!(self.unresolved_policy, crate::UnresolvedPolicy::Trap) as usize);
        for value in [
            self.thread_pointer_slot,
            self.thread_pointer_teb_slot,
            self.host_transition_teb_slot,
            self.thread_pointer_scratch_slot,
        ] {
            out.optional(value.map(|v| v as usize));
        }
        out.word(self.patched_canary_sites);
        out.word(self.unpatched_sites);
        out.word(self.patched_syscall_sites);
        out.word(self.runtime_handles.len());
        for (value, target) in &self.runtime_handles {
            out.word(*value);
            out.handle(*target);
        }
        out.word(self.next_runtime_handle);
        self.runtime_maps.snapshot(&mut out);
        Ok(out.0)
    }

    /// Rebuild private metadata around the mappings restored by runtime.
    ///
    /// # Safety
    /// This is a one-time ownership transfer in a fresh fork child after its
    /// native images, guest mappings and retained handle slots were restored.
    pub unsafe fn restore_fork(payload: &[u8]) -> Result<Self, LinkError> {
        let mut input = Reader(payload);
        if input.word()? != MAGIC || input.word()? != 4 {
            return Err(invalid());
        }
        let registry = crate::provider::restore_registry(&input.path()?)?;
        let paths = SearchPaths {
            host_directory: input.path()?,
            extra: input.paths()?,
            process_namespace: input.boolean()?,
        };
        let mut linker = Self::new(paths);
        linker.registry = registry;
        linker.library_paths = input.paths()?;
        linker.system_paths = input.paths()?.try_into().map_err(|_| invalid())?;
        linker.c_allocator = [input.word()?, input.word()?, input.word()?];
        for _ in 0..input.count()? {
            let name = input.text()?;
            let path = input.path()?;
            let bytes = unsafe {
                crate::ImmutableBytes::adopt_fork([input.word()?, input.word()?, input.word()?])?
            };
            let base = input.word()?;
            let allocation_len = input.word()?;
            let slot = input.word()?;
            let load_bias = i128::from_le_bytes(input.take(16)?.try_into().unwrap());
            let symbol_count = u32::try_from(input.word()?).map_err(|_| invalid())?;
            let tls_module = input.optional()?;
            let dependencies = (0..input.count()?)
                .map(|_| input.word().map(ObjectId))
                .collect::<Result<Vec<_>, _>>()?;
            let initialized = input.boolean()?;
            let preinitialized = input.boolean()?;
            let nodelete = input.boolean()?;
            let is_executable = input.boolean()?;
            let self_relocating = input.boolean()?;
            let references = input.word()?;
            let elf = ElfFile::parse(&bytes)?;
            let dynamic = elf.dynamic_info()?.unwrap_or_default();
            let versions = kinakaze_elf::version::VersionTable::parse(elf, &dynamic)?;
            let mapping = unsafe {
                crate::object::mapping::ImageMapping::adopt_fork(base, allocation_len, slot)?
            };
            linker.objects.push(MappedObject {
                name,
                path,
                bytes,
                allocation: base as _,
                allocation_len,
                _mapping: mapping,
                load_bias,
                dynamic,
                versions,
                symbol_count,
                tls_module,
                dependencies,
                initialized,
                preinitialized,
                references,
                nodelete,
                is_executable,
                self_relocating,
            });
        }
        for _ in 0..input.count()? {
            linker.provider_dependencies.push(
                (0..input.count()?)
                    .map(|_| input.provider())
                    .collect::<Result<_, _>>()?,
            );
        }
        for _ in 0..input.count()? {
            let name = input.text()?;
            let base = input.word()?;
            let image = linker.registry.get(&name).ok_or_else(invalid)?;
            if image.base() != base {
                return Err(invalid());
            }
            let mut dll = DllProvider::from_registered(image);
            dll.references = input.word()?;
            dll.nodelete = input.boolean()?;
            linker.scope.dlls.push(dll);
        }
        linker.scope.order = (0..input.count()?)
            .map(|_| input.word().map(ObjectId))
            .collect::<Result<_, _>>()?;
        linker.scope.providers = (0..input.count()?)
            .map(|_| input.provider())
            .collect::<Result<_, _>>()?;
        linker.scope.globals = (0..input.count()?)
            .map(|_| input.boolean())
            .collect::<Result<_, _>>()?;
        for _ in 0..input.count()? {
            let name = input.text()?;
            let address = input.word()?;
            if linker.builtins.insert(name, address).is_some() {
                return Err(invalid());
            }
        }
        for _ in 0..input.count()? {
            linker.pending_ifuncs.push(PendingIfunc {
                target: input.word()?,
                resolver: input.word()?,
            });
        }
        linker.initialization_order = (0..input.count()?)
            .map(|_| input.word().map(ObjectId))
            .collect::<Result<_, _>>()?;
        linker.finalizers = if input.boolean()? {
            Some(
                (0..input.count()?)
                    .map(|_| input.word())
                    .collect::<Result<_, _>>()?,
            )
        } else {
            None
        };
        linker.unresolved_policy = if input.boolean()? {
            crate::UnresolvedPolicy::Trap
        } else {
            crate::UnresolvedPolicy::Fail
        };
        let mut slot = || {
            input
                .optional()?
                .map(|v| u32::try_from(v).map_err(|_| invalid()))
                .transpose()
        };
        linker.thread_pointer_slot = slot()?;
        linker.thread_pointer_teb_slot = slot()?;
        linker.host_transition_teb_slot = slot()?;
        linker.thread_pointer_scratch_slot = slot()?;
        linker.patched_canary_sites = input.word()?;
        linker.unpatched_sites = input.word()?;
        linker.patched_syscall_sites = input.word()?;
        for _ in 0..input.count()? {
            let value = input.word()?;
            let target = input.handle()?;
            if linker.runtime_handles.insert(value, target).is_some() {
                return Err(invalid());
            }
        }
        linker.next_runtime_handle = input.word()?;
        linker.runtime_maps = unsafe { introspection::RuntimeMaps::restore(&mut input)? };
        let valid_provider = |value: &Provider| match value {
            Provider::Elf(id) => id.0 < linker.objects.len(),
            Provider::Dll(id) => *id < linker.scope.dlls.len(),
        };
        if !input.0.is_empty()
            || linker.provider_dependencies.len() != linker.objects.len()
            || linker.scope.providers.len() != linker.scope.globals.len()
            || !linker.scope.providers.iter().all(valid_provider)
            || !linker
                .provider_dependencies
                .iter()
                .flatten()
                .all(valid_provider)
            || linker
                .scope
                .order
                .iter()
                .chain(&linker.initialization_order)
                .any(|id| id.0 >= linker.objects.len())
            || linker.objects.iter().any(|object| {
                object
                    .dependencies
                    .iter()
                    .any(|id| id.0 >= linker.objects.len())
            })
            || linker.runtime_handles.values().any(|target| match target {
                RuntimeHandle::Main => false,
                RuntimeHandle::Elf(id) => id.0 >= linker.objects.len(),
                RuntimeHandle::Dll(id) => *id >= linker.scope.dlls.len(),
            })
        {
            return Err(invalid());
        }
        Ok(linker)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reader_rejects_lengths_tags_and_truncated_words() {
        assert!(Reader(&[0; 7]).word().is_err());
        assert!(Reader(&u64::MAX.to_le_bytes()).bytes().is_err());
        assert!(Reader(&[2, 0, 0, 0, 0, 0, 0, 0]).boolean().is_err());
        let mut writer = Writer::default();
        writer.word(4);
        writer.word(0);
        assert!(Reader(&writer.0).handle().is_err());
        let mut writer = Writer::default();
        writer.path(Path::new("C:/客体/lib"));
        assert_eq!(Reader(&writer.0).path().unwrap(), Path::new("C:/客体/lib"));
    }
}
