//! Symbol resolution: the global scope and the rules for searching it.
//!
//! The order is the load order, which is the breadth-first walk of the
//! `DT_NEEDED` graph starting at the executable. The first *global* definition
//! found along that order wins, and that is what makes interposition work: an
//! object earlier in the order shadows a later one's definition of the same name.
//!
//! Weak definitions are the exception. A weak definition is a placeholder, so it
//! is only accepted after the whole scope has been searched without finding a
//! strong one.

use std::path::PathBuf;

use kinakaze_elf::{DynamicSymbol, STB_WEAK};

use crate::LinkError;
use crate::object::{MappedObject, ObjectId};

/// Where a symbol was found and what it resolves to.
#[derive(Clone, Copy, Debug)]
pub struct Resolution {
    /// The object that provided the definition, or `None` for a DLL export.
    pub owner: Option<ObjectId>,
    /// The final address, absent for a TLS symbol which has no address.
    pub address: Option<usize>,
    /// The size of the definition, needed by COPY relocations.
    pub size: u64,
    /// TLS module id, when the definition is thread-local.
    pub tls_module: Option<usize>,
    /// Offset within the TLS block, when the definition is thread-local.
    pub tls_offset: Option<usize>,
    /// True when this came from an `STT_GNU_IFUNC` symbol, so the address is a
    /// resolver that must be called rather than used directly.
    pub is_ifunc: bool,
    /// True when nothing defined the symbol and this address is a trap.
    ///
    /// Callers that would *dereference* the address rather than merely store it
    /// have to check this: a trapped data symbol points at an inaccessible guard
    /// page, so reading through it faults by design.
    pub is_trap: bool,
}

impl Resolution {
    pub fn from_address(address: usize) -> Self {
        Self {
            owner: None,
            address: Some(address),
            size: 0,
            tls_module: None,
            tls_offset: None,
            is_ifunc: false,
            is_trap: false,
        }
    }

    /// A resolution standing in for a symbol nothing defined.
    pub fn trap(address: usize) -> Self {
        Self {
            is_trap: true,
            ..Self::from_address(address)
        }
    }
}

/// A registered, already-bound ELF facade and its paired Windows DLL.
/// The historical internal type name remains to keep runtime handles stable.
pub struct DllProvider {
    image: std::sync::Arc<dyn crate::ProviderImage>,
    symbol_indices: std::collections::HashMap<String, usize>,
    pub path: PathBuf,
    pub name: String,
    pub(crate) references: usize,
    pub(crate) nodelete: bool,
}

impl DllProvider {
    pub fn from_registered(image: std::sync::Arc<dyn crate::ProviderImage>) -> Self {
        Self {
            symbol_indices: image
                .symbols()
                .iter()
                .enumerate()
                .map(|(index, symbol)| (symbol.name.clone(), index))
                .collect(),
            path: std::fs::canonicalize(image.path())
                .unwrap_or_else(|_| image.path().to_path_buf()),
            name: image.soname().to_owned(),
            image,
            references: 1,
            nodelete: false,
        }
    }

    pub fn add_reference(&mut self) -> Result<(), LinkError> {
        self.references = self
            .references
            .checked_add(1)
            .ok_or(LinkError::AddressOverflow)?;
        Ok(())
    }

    pub fn release_reference(&mut self) {
        self.references = self.references.saturating_sub(1);
    }
    pub fn is_loaded(&self) -> bool {
        self.references != 0 || self.nodelete
    }
    pub fn mark_nodelete(&mut self) {
        self.nodelete = true;
    }
    pub fn base(&self) -> usize {
        self.image.base()
    }
    pub fn image(&self) -> &dyn crate::ProviderImage {
        self.image.as_ref()
    }

    pub fn symbol(&self, name: &str) -> Result<Option<usize>, LinkError> {
        Ok(self.resolve(name, None)?.and_then(|symbol| symbol.address))
    }

    pub fn resolve(
        &self,
        name: &str,
        version: Option<&str>,
    ) -> Result<Option<Resolution>, LinkError> {
        if !self.is_loaded() {
            return Ok(None);
        }
        Ok(self
            .symbol_indices
            .get(name)
            .and_then(|index| self.image.symbols().get(*index))
            .filter(|symbol| symbol.matches_version(version))
            .map(|symbol| Resolution {
                size: symbol.size,
                ..Resolution::from_address(symbol.address)
            }))
    }

    pub fn redirect_copy(
        &self,
        name: &str,
        address: usize,
        size: u64,
        source: usize,
    ) -> Result<(), LinkError> {
        if let Some(symbol) = self
            .symbol_indices
            .get(name)
            .and_then(|index| self.image.symbols().get(*index))
            .filter(|symbol| symbol.address == source)
        {
            if symbol.kind != crate::ProviderSymbolKind::Object {
                return Err(LinkError::BadCopyRelocation {
                    symbol: name.to_owned(),
                });
            }
            self.image.redirect_copy(name, address, size)?;
        }
        Ok(())
    }
}

/// One position in the process-wide ELF/facade symbol search order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Provider {
    Elf(ObjectId),
    Dll(usize),
}

/// The native ELF and bound facade objects used for symbol resolution.
#[derive(Default)]
pub struct Scope {
    /// Load order, which defines search order.
    pub order: Vec<ObjectId>,
    /// Bound facade providers indexed by [`Provider::Dll`].
    pub dlls: Vec<DllProvider>,
    /// Unified load order used for lookup. Keeping bound facade providers in
    /// this sequence gives them the same interposition rules as native ELF `.so`
    /// objects instead of maintaining a hard-coded second tier.
    pub providers: Vec<Provider>,
    /// Whether each provider is visible through the process-wide global scope.
    /// Local providers remain available through their own `dlopen` handle, but
    /// RTLD_DEFAULT and unrelated future objects cannot see them.
    pub globals: Vec<bool>,
}

impl Scope {
    pub fn push_provider(&mut self, provider: Provider) {
        self.providers.push(provider);
        // Startup DT_NEEDED objects form the initial global scope. A runtime
        // dlopen may demote the newly appended range before it is relocated.
        self.globals.push(true);
    }

    pub fn set_global(&mut self, provider: Provider, global: bool) {
        for (index, candidate) in self.providers.iter().enumerate() {
            if *candidate == provider {
                self.globals[index] = global;
            }
        }
    }

    /// Resolves a reference against the whole scope.
    ///
    /// `version` is the version the reference demands, if any.
    pub fn resolve(
        &self,
        objects: &[MappedObject],
        name: &str,
        version: Option<&str>,
    ) -> Result<Option<Resolution>, LinkError> {
        self.resolve_filtered(objects, name, version, None, 0)
    }

    /// Implements RTLD_NEXT: start strictly after the ELF object containing the
    /// call site, then search only globally visible providers.
    pub fn resolve_after(
        &self,
        objects: &[MappedObject],
        caller: ObjectId,
        name: &str,
        version: Option<&str>,
    ) -> Result<Option<Resolution>, LinkError> {
        let start = self
            .providers
            .iter()
            .position(|provider| *provider == Provider::Elf(caller))
            .map_or(0, |index| index + 1);
        self.resolve_filtered(objects, name, version, None, start)
    }

    /// Resolves a reference while ignoring one object's own definitions.
    ///
    /// `R_X86_64_COPY` needs this. The relocation exists because the executable
    /// allocated its *own* storage for a library's variable, and that storage is
    /// itself a definition in the executable's symbol table — so an ordinary lookup
    /// finds the destination and reports it as the source. Copying a variable onto
    /// itself is not merely useless: the source is never initialized, so `stdout`
    /// would stay null instead of pointing at the library's stream.
    pub fn resolve_excluding(
        &self,
        objects: &[MappedObject],
        name: &str,
        version: Option<&str>,
        skip: Option<ObjectId>,
    ) -> Result<Option<Resolution>, LinkError> {
        self.resolve_filtered(objects, name, version, skip, 0)
    }

    fn resolve_filtered(
        &self,
        objects: &[MappedObject],
        name: &str,
        version: Option<&str>,
        skip: Option<ObjectId>,
        start: usize,
    ) -> Result<Option<Resolution>, LinkError> {
        let mut weak_fallback: Option<Resolution> = None;
        static SYMBOL_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let trace_stdio = *SYMBOL_TRACE
            .get_or_init(|| std::env::var_os("KINAKAZE_SYMBOL_TRACE").is_some())
            && matches!(
                name,
                "fopen" | "fopen64" | "__getdelim" | "getdelim" | "fclose"
            );

        for (index, provider) in self.providers.iter().enumerate().skip(start) {
            if !self.globals.get(index).copied().unwrap_or(true) {
                continue;
            }
            match *provider {
                Provider::Elf(id) => {
                    if Some(id) == skip {
                        continue;
                    }
                    let object = &objects[id.0];
                    let Some(symbol) = object.lookup_versioned(name, version)? else {
                        continue;
                    };
                    let resolution = definition_of(objects, id, symbol)?;
                    // A weak definition is a placeholder: keep looking for a
                    // strong one and only fall back if the whole scope has none.
                    if symbol.binding() == STB_WEAK {
                        if weak_fallback.is_none() {
                            weak_fallback = Some(resolution);
                        }
                        continue;
                    }
                    if trace_stdio {
                        eprintln!(
                            "kinakaze symbol: {name} -> ELF[{}] {} address={:#x}",
                            id.0,
                            object.name,
                            resolution.address.unwrap_or_default()
                        );
                    }
                    return Ok(Some(resolution));
                }
                Provider::Dll(index) => {
                    let Some(dll) = self.dlls.get(index) else {
                        continue;
                    };
                    if let Some(resolution) = dll.resolve(name, version)? {
                        let address = resolution.address.unwrap_or_default();
                        if trace_stdio {
                            eprintln!(
                                "kinakaze symbol: {name} -> DLL[{index}] {} address={address:#x}",
                                dll.name
                            );
                        }
                        return Ok(Some(resolution));
                    }
                }
            }
        }

        Ok(weak_fallback)
    }

    /// Resolves within one object first, then the global scope.
    ///
    /// This is the `DF_SYMBOLIC` lookup order: the object's own definitions take
    /// precedence over anything the scope could interpose.
    pub fn resolve_symbolic(
        &self,
        objects: &[MappedObject],
        owner: ObjectId,
        name: &str,
        version: Option<&str>,
    ) -> Result<Option<Resolution>, LinkError> {
        let object = &objects[owner.0];
        if let Some(symbol) = object.lookup(name)?
            && version_matches(object, symbol, version)
        {
            return Ok(Some(definition_of(objects, owner, symbol)?));
        }
        self.resolve(objects, name, version)
    }
}

/// Decides whether a definition satisfies a reference's version requirement.
///
/// Native ELF retains ELF version lookup semantics. Registered facades use
/// `ProviderSymbol::matches_version` and require an explicit supported version.
pub fn version_matches(
    object: &MappedObject,
    symbol: DynamicSymbol<'_>,
    wanted: Option<&str>,
) -> bool {
    let Some(wanted) = wanted else {
        // An unversioned reference must not bind to a hidden definition: hidden
        // means "only reachable by asking for this version explicitly".
        return !object.definition_is_hidden(symbol.index);
    };
    match object.definition_version(symbol.index) {
        Some(available) => available == wanted,
        // No version information for this definition. Accept it.
        None => true,
    }
}

/// Builds a resolution from a definition found in a specific object.
fn definition_of(
    objects: &[MappedObject],
    owner: ObjectId,
    symbol: DynamicSymbol<'_>,
) -> Result<Resolution, LinkError> {
    let object = &objects[owner.0];
    if object.is_tls(symbol) {
        return Ok(Resolution {
            owner: Some(owner),
            address: None,
            size: symbol.size,
            tls_module: object.tls_module,
            // A TLS symbol's value is its offset inside its module's block, not
            // an address, so the load bias must not be applied.
            tls_offset: Some(
                usize::try_from(symbol.value).map_err(|_| LinkError::AddressOverflow)?,
            ),
            is_ifunc: false,
            is_trap: false,
        });
    }
    Ok(Resolution {
        owner: Some(owner),
        address: Some(object.definition_address(symbol)?),
        size: symbol.size,
        tls_module: None,
        tls_offset: None,
        is_ifunc: object.is_ifunc(symbol),
        is_trap: false,
    })
}
