//! The linker driver: dependency resolution, load order, and initialization.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use kinakaze_elf::ElfFile;

use crate::LinkError;
use crate::object::{MappedObject, ObjectId};
use crate::relocate::{self, PendingIfunc};
use crate::scope::{DllProvider, Provider, Resolution, Scope};

mod finalizers;
#[cfg(windows)]
mod fork;
mod introspection;
mod loading;
pub(crate) mod profile;
pub use introspection::{GuestLinkMap, RuntimeObjectInfo};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum RuntimeHandle {
    Main,
    Elf(ObjectId),
    Dll(usize),
}

const RTLD_LAZY: i32 = 0x1;
const RTLD_NOW: i32 = 0x2;
const RTLD_NOLOAD: i32 = 0x4;
const RTLD_GLOBAL: i32 = 0x100;
const RTLD_NODELETE: i32 = 0x1000;
const RTLD_KNOWN_FLAGS: i32 = RTLD_LAZY | RTLD_NOW | RTLD_NOLOAD | RTLD_GLOBAL | RTLD_NODELETE;

/// Where dependencies are looked for, in order.
#[derive(Clone, Debug, Default)]
pub struct SearchPaths {
    /// Directory holding the executable and its guest `/usr/lib` tree.
    pub host_directory: PathBuf,
    /// Extra directories, searched after an object's own `DT_RUNPATH`.
    pub extra: Vec<PathBuf>,
    /// Follow the calling guest's current root and mount namespace. Standalone
    /// linker tools can instead resolve against `host_directory` explicitly.
    pub process_namespace: bool,
}

impl SearchPaths {
    pub fn with_host_directory(directory: PathBuf) -> Self {
        Self {
            host_directory: directory,
            extra: Vec::new(),
            process_namespace: false,
        }
    }

    pub fn with_process_namespace(mut self) -> Self {
        self.process_namespace = true;
        self
    }
}

/// The dynamic linker.
pub struct Linker {
    objects: Vec<MappedObject>,
    /// Direct DT_NEEDED providers for each ELF object. This includes PE-backed
    /// `.so` entries, while `MappedObject::dependencies` contains ELF ids only.
    provider_dependencies: Vec<Vec<Provider>>,
    scope: Scope,
    paths: SearchPaths,
    registry: std::sync::Arc<crate::ProviderRegistry>,
    /// `LD_LIBRARY_PATH` is process startup state. Parsing it once also avoids
    /// taking the process environment lock for every `DT_NEEDED` edge.
    library_paths: Vec<PathBuf>,
    /// Guest system directories are invariant for this linker. Retaining them
    /// avoids rebuilding six `PathBuf`s for every dependency lookup.
    system_paths: [PathBuf; 6],
    /// Host-provided symbols, which override everything.
    builtins: HashMap<String, usize>,
    /// Ifunc resolvers deferred until the whole image is linked.
    pending_ifuncs: Vec<PendingIfunc>,
    /// Resolved allocator entries survive fork without rerunning IFUNC code.
    c_allocator: [usize; 3],
    /// Objects whose initializers still have to run, in dependency order.
    initialization_order: Vec<ObjectId>,
    /// Pending destructor addresses, consumed before each call and forked by value.
    finalizers: Option<Vec<usize>>,
    /// What to do about a symbol nothing defines.
    unresolved_policy: crate::trap::UnresolvedPolicy,
    /// The `TlsAlloc` index the canary is redirected to, when patching is enabled.
    thread_pointer_slot: Option<u32>,
    /// Direct TEB slot containing the actual guest thread pointer. Import
    /// trampolines restore FS from here after every host call.
    thread_pointer_teb_slot: Option<u32>,
    /// Direct TEB slot containing host-only syscall/import transition state.
    /// Unlike the guest TP slot, ARCH_SET_FS never changes this value.
    host_transition_teb_slot: Option<u32>,
    /// Per-thread R11 save slot used without touching the SysV red zone.
    thread_pointer_scratch_slot: Option<u32>,
    /// How many canary reads were rewritten, for reporting.
    patched_canary_sites: usize,
    unpatched_sites: usize,
    patched_syscall_sites: usize,
    runtime_handles: HashMap<usize, RuntimeHandle>,
    next_runtime_handle: usize,
    runtime_maps: introspection::RuntimeMaps,
}

impl Linker {
    pub fn new(paths: SearchPaths) -> Self {
        let library_paths = std::env::var("LD_LIBRARY_PATH")
            .ok()
            .map(|value| {
                value
                    .split([':', ';'])
                    .filter(|item| !item.is_empty())
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();
        let system_paths = [
            PathBuf::from("/usr/lib"),
            PathBuf::from("/lib"),
            PathBuf::from("/usr/lib/x86_64-linux-gnu"),
            PathBuf::from("/lib/x86_64-linux-gnu"),
            PathBuf::from("/usr/lib64"),
            PathBuf::from("/lib64"),
        ];
        Self {
            objects: Vec::new(),
            provider_dependencies: Vec::new(),
            scope: Scope::default(),
            paths,
            registry: std::sync::Arc::new(crate::ProviderRegistry::new()),
            library_paths,
            system_paths,
            builtins: HashMap::new(),
            pending_ifuncs: Vec::new(),
            c_allocator: [0; 3],
            initialization_order: Vec::new(),
            finalizers: None,
            unresolved_policy: crate::trap::UnresolvedPolicy::default(),
            thread_pointer_slot: None,
            thread_pointer_teb_slot: None,
            host_transition_teb_slot: None,
            thread_pointer_scratch_slot: None,
            patched_canary_sites: 0,
            unpatched_sites: 0,
            patched_syscall_sites: 0,
            runtime_handles: HashMap::new(),
            // Opaque Linux handles never alias a valid aligned image address.
            next_runtime_handle: 0x1003,
            runtime_maps: introspection::RuntimeMaps::default(),
        }
    }

    /// Supplies already-bound facade providers before the first guest object.
    /// Ordinary workers must validate that their registry includes libc.so.6;
    /// containers may intentionally supply a different registry.
    pub fn register_provider_registry(
        &mut self,
        registry: std::sync::Arc<crate::ProviderRegistry>,
    ) -> Result<(), LinkError> {
        if !self.objects.is_empty() || !self.scope.providers.is_empty() {
            return Err(LinkError::InvalidProvider(
                "registry must be configured before loading objects".into(),
            ));
        }
        self.registry = registry;
        Ok(())
    }

    /// Redirects the guest's `%fs:` thread-pointer reads at a TEB slot.
    ///
    /// Required for any binary built with `-fstack-protector`, which is nearly all
    /// of them. The execution engine owns the actual instruction translation.
    pub fn set_thread_pointer_slot(&mut self, slot: u32) {
        self.thread_pointer_slot = Some(slot);
    }

    /// Configures both durable per-thread values used by AOT TLS handling.
    pub fn set_thread_pointer_slots(
        &mut self,
        canary_slot: u32,
        thread_pointer_slot: u32,
        host_transition_slot: u32,
        scratch_slot: u32,
    ) {
        self.thread_pointer_slot = Some(canary_slot);
        self.thread_pointer_teb_slot = Some(thread_pointer_slot);
        self.host_transition_teb_slot = Some(host_transition_slot);
        self.thread_pointer_scratch_slot = Some(scratch_slot);
    }

    pub fn patched_syscall_sites(&self) -> usize {
        self.patched_syscall_sites
    }
    pub fn patched_canary_sites(&self) -> usize {
        self.patched_canary_sites
    }
    pub fn unpatched_thread_pointer_sites(&self) -> usize {
        self.unpatched_sites
    }

    /// Chooses what happens when nothing defines a referenced symbol.
    ///
    /// Defaults to failing the load. [`UnresolvedPolicy::Trap`] keeps the program
    /// loadable and defers the failure to the moment a missing symbol is used,
    /// which is what makes a large binary runnable before its whole libc surface
    /// exists.
    pub fn set_unresolved_policy(&mut self, policy: crate::trap::UnresolvedPolicy) {
        self.unresolved_policy = policy;
    }

    /// Registers a symbol the host implements directly.
    ///
    /// These win over every other provider, because nothing in the guest can
    /// supply them.
    pub fn add_builtin(&mut self, name: &str, address: usize) {
        self.builtins.insert(name.to_owned(), address);
    }

    pub fn objects(&self) -> &[MappedObject] {
        &self.objects
    }

    /// Loaded facade modules, including RTLD_LOCAL modules for dladdr and
    /// dl_iterate_phdr. The iterator borrows the linker's lifetime-pinned images.
    pub fn provider_images(&self) -> impl Iterator<Item = &dyn crate::ProviderImage> {
        self.scope
            .dlls
            .iter()
            .filter(|provider| provider.is_loaded())
            .map(|provider| provider.image())
    }

    /// Marks the object that owns the process entry point.
    ///
    /// PIE executables are `ET_DYN`, so file type alone cannot distinguish them
    /// from shared libraries. The kernel-entry path supplies this role explicitly.
    pub fn mark_executable(&mut self, id: ObjectId) -> Result<(), LinkError> {
        if id.0 >= self.objects.len() {
            return Err(LinkError::NoExecutableObject);
        }
        if self
            .objects
            .iter()
            .enumerate()
            .any(|(index, object)| index != id.0 && object.is_executable)
        {
            return Err(LinkError::MultipleExecutableObjects);
        }
        self.objects[id.0].is_executable = true;
        Ok(())
    }

    /// Loads an object and its whole dependency graph, then links everything.
    ///
    /// Returns the root object.
    pub fn load(&mut self, path: &Path) -> Result<ObjectId, LinkError> {
        self.load_root(path, None)
    }

    /// Loads a root object from the stable bytes captured before an exec child
    /// was created. Dependencies continue to use their resolved host paths.
    pub fn load_image(
        &mut self,
        path: &Path,
        image: crate::ImmutableBytes,
    ) -> Result<ObjectId, LinkError> {
        self.load_root(path, Some(image))
    }

    fn load_root(
        &mut self,
        path: &Path,
        image: Option<crate::ImmutableBytes>,
    ) -> Result<ObjectId, LinkError> {
        let root = self.load_graph_image(path, None, "<command line>", image)?;
        self.link_all()?;
        Ok(root)
    }

    /// Process startup places argv/auxv and publishes initial libc data before
    /// relocations copy those values into guest RELRO storage.
    pub fn load_process_graph(
        &mut self,
        path: &Path,
        image: Option<crate::ImmutableBytes>,
    ) -> Result<ObjectId, LinkError> {
        self.load_graph_image(path, None, "<command line>", image)
    }

    /// Loads an object graph without linking, so several roots can be combined.
    pub fn load_graph(
        &mut self,
        path: &Path,
        requested_name: Option<&str>,
        requested_by: &str,
    ) -> Result<ObjectId, LinkError> {
        self.load_graph_image(path, requested_name, requested_by, None)
    }

    fn load_graph_image(
        &mut self,
        path: &Path,
        requested_name: Option<&str>,
        requested_by: &str,
        image: Option<crate::ImmutableBytes>,
    ) -> Result<ObjectId, LinkError> {
        let _graph = profile::begin("graph", path.display());
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(existing) = self
            .objects
            .iter()
            .position(|object| object.path == canonical)
        {
            // Already loaded. A second request is a new reference, which matters
            // for dlclose.
            self.objects[existing].references += 1;
            return Ok(ObjectId(existing));
        }
        let _ = requested_by;

        let bytes = match image {
            Some(bytes) => bytes,
            None => read_elf_candidate(&canonical)?,
        };
        reject_unbound_facade(&bytes, &canonical)?;
        let mapping = profile::begin("map", canonical.display());
        let object = MappedObject::map_image(&canonical, requested_name, bytes)?;
        drop(mapping);
        let id = ObjectId(self.objects.len());
        let object_directory = canonical
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        // The needed list and search path are read before the object is moved into
        // the vector, because both borrow its retained bytes.
        let elf = ElfFile::parse(&object.bytes)?;
        let needed: Vec<String> = elf
            .dynamic_needed(&object.dynamic)?
            .into_iter()
            .map(str::to_owned)
            .collect();
        let own_search_path = elf.dynamic_search_path(&object.dynamic)?.map(str::to_owned);
        let name = object.name.clone();

        self.objects.push(object);
        self.provider_dependencies.push(Vec::new());
        // Load order is the breadth-first order of the DT_NEEDED graph, and it is
        // the search order for symbols, so the object is appended before its
        // dependencies.
        self.scope.order.push(id);
        self.scope.push_provider(Provider::Elf(id));

        let guest_root = &self.paths.host_directory;
        let runpath = own_search_path
            .as_deref()
            .map(|raw| expand_search_path(raw, &object_directory, guest_root))
            .unwrap_or_default();

        let mut dependencies = Vec::new();
        let mut provider_dependencies = Vec::new();
        for dependency in needed {
            let provider = self.satisfy(&dependency, &object_directory, &runpath, &name)?;
            if let Provider::Elf(child) = provider {
                dependencies.push(child);
            }
            provider_dependencies.push(provider);
        }
        self.objects[id.0].dependencies = dependencies;
        self.provider_dependencies[id.0] = provider_dependencies;
        Ok(id)
    }

    /// Finds and loads whatever satisfies one `DT_NEEDED` entry.
    ///
    /// Exact registered SONAMEs bind to their already-loaded facade; all other
    /// dependencies follow ordinary native ELF search paths.
    fn satisfy(
        &mut self,
        needed: &str,
        object_directory: &Path,
        runpath: &[PathBuf],
        requested_by: &str,
    ) -> Result<Provider, LinkError> {
        if let Some(image) = self.registry.get(needed) {
            return Ok(Provider::Dll(self.load_registered(image)?));
        }
        if let Some(existing) = self.objects.iter().position(|obj| {
            obj.name == needed || obj.path.file_name().and_then(|n| n.to_str()) == Some(needed)
        }) {
            self.objects[existing].references += 1;
            return Ok(Provider::Elf(ObjectId(existing)));
        }
        let mut searched = Vec::new();
        let object_system_dirs = [
            object_directory.join("../lib"),
            object_directory.join("../usr/lib"),
        ];

        let directories = self
            .library_paths
            .iter()
            .map(PathBuf::as_path)
            .chain(runpath.iter().map(PathBuf::as_path))
            .chain(self.paths.extra.iter().map(PathBuf::as_path))
            // Linux resolves the default system directories before any
            // compatibility fallback beside the executable.  Keeping that
            // order also prevents a stale build-tree PE alias from shadowing
            // the current provider installed in `/usr/lib`.
            .chain(object_system_dirs.iter().map(PathBuf::as_path))
            .chain(self.system_paths.iter().map(PathBuf::as_path))
            .chain(std::iter::once(object_directory));

        for directory in directories {
            let candidate = directory.join(needed);
            if let Some(resolved) = self.resolve_candidate(&candidate) {
                return self.load_candidate(&resolved, needed, requested_by);
            }
            searched.push(candidate);
        }

        Err(LinkError::MissingDependency {
            name: needed.to_owned(),
            requested_by: requested_by.to_owned(),
            searched,
        })
    }

    fn load_candidate(
        &mut self,
        path: &Path,
        name: &str,
        requested_by: &str,
    ) -> Result<Provider, LinkError> {
        if let Some(image) = self
            .registry
            .by_path(path)
            .or_else(|| self.registry.get(name))
        {
            return Ok(Provider::Dll(self.load_registered(image)?));
        }
        let image = read_elf_candidate(path)?;
        Ok(Provider::Elf(self.load_graph_image(
            path,
            Some(name),
            requested_by,
            Some(image),
        )?))
    }

    fn load_registered(
        &mut self,
        image: std::sync::Arc<dyn crate::ProviderImage>,
    ) -> Result<usize, LinkError> {
        if let Some((index, dll)) = self
            .scope
            .dlls
            .iter_mut()
            .enumerate()
            .find(|(_, dll)| dll.name == image.soname())
        {
            dll.add_reference()?;
            return Ok(index);
        }
        let index = self.scope.dlls.len();
        self.scope.dlls.push(DllProvider::from_registered(image));
        self.scope.push_provider(Provider::Dll(index));
        Ok(index)
    }

    /// Relocates every loaded object, then finalizes protections.
    pub fn link_all(&mut self) -> Result<(), LinkError> {
        self.link_from(0)
    }

    /// Relocates objects appended by a runtime `dlopen` without touching pages
    /// whose RELRO/protections were already finalized during process startup.
    fn link_from(&mut self, first: usize) -> Result<(), LinkError> {
        let _link = profile::begin("link", first);
        for object in &self.objects[first..] {
            if let Some(module_id) = object.tls_module {
                let _ = kinakaze_tls::reserve_static_elf_module(module_id);
            }
        }
        let mut pending = std::mem::take(&mut self.pending_ifuncs);
        let copies = relocate::CopiedVariables::default();
        for index in (first..self.objects.len()).rev() {
            if self.objects[index].self_relocating {
                continue;
            }
            if std::env::var_os("KINAKAZE_TRACE_COPIES").is_some() {
                eprintln!(
                    "kinakaze: relocating object[{index}] {} bias={:#x}",
                    self.objects[index].name, self.objects[index].load_bias
                );
            }
            let builtins = &self.builtins;
            let resolver = move |name: &str| builtins.get(name).copied();
            let context = relocate::Context {
                objects: &self.objects,
                scope: &self.scope,
                owner: ObjectId(index),
                builtins: &resolver,
                unresolved_policy: self.unresolved_policy,
                copies: &copies,
                thread_pointer_teb_slot: self.thread_pointer_teb_slot,
            };
            let _relocation = profile::begin("relocate", &self.objects[index].name);
            relocate::relocate_object(&context, &mut pending)?;
        }

        // Tell each provider where the guest's copy of its variables now lives. This
        // completes what a COPY relocation means: the value has been copied, and now
        // the provider's own accesses have to reach the same place. See
        // `install_copy_redirects`.
        self.install_copy_redirects(&copies.entries())?;
        for object in &self.objects[first..] {
            object.publish_relocated_tls()?;
        }
        // Relocations precede code preparation; final protections follow it.
        // ld.so supplies only a borrowed mapping view through runtime. Decoder,
        // cache, trampoline allocation and VEH state belong to the engine.
        if let Some(slot) = self.thread_pointer_slot {
            let dispatcher = self
                .builtins
                .get("kinakaze_abi_syscall_raw")
                .copied()
                .ok_or_else(|| LinkError::Execution("raw syscall dispatcher is missing".into()))?;
            let config = kinakaze_runtime::execution::CodeConfig {
                canary_slot: slot,
                thread_pointer_slot: self.thread_pointer_teb_slot.unwrap_or(u32::MAX),
                transition_slot: self.host_transition_teb_slot.unwrap_or(u32::MAX),
                scratch_slot: self.thread_pointer_scratch_slot.unwrap_or(u32::MAX),
                syscall_dispatcher: dispatcher,
            };
            for object in &self.objects[first..] {
                let _preparation = profile::begin("prepare", &object.name);
                let view = kinakaze_runtime::execution::CodeImage {
                    bytes: object.bytes.as_ptr(),
                    length: object.bytes.len(),
                    base: object.allocation as usize,
                    mapped_length: object.allocation_len,
                    load_bias: object.load_bias,
                };
                let mut result = kinakaze_runtime::execution::CodeResult::default();
                if unsafe { kinakaze_runtime::execution::prepare(&view, &config, &mut result) } != 0
                {
                    let length = result
                        .error
                        .iter()
                        .position(|&byte| byte == 0)
                        .unwrap_or(result.error.len());
                    return Err(LinkError::Execution(
                        String::from_utf8_lossy(&result.error[..length]).into_owned(),
                    ));
                }
                self.patched_canary_sites += result.canary_sites;
                self.patched_syscall_sites += result.syscall_sites;
                self.unpatched_sites += result.thread_pointer_sites;
            }
        }
        for object in &self.objects[first..] {
            let _protection = profile::begin("protect", &object.name);
            object.protect()?;
        }

        // TPOFF relocations above have now fixed the complete variant-II layout.
        // IFUNC resolvers are guest code and may themselves touch static TLS, so
        // the current thread needs its real Linux TP before the first resolver.
        if let Some(slot) = self.thread_pointer_teb_slot {
            kinakaze_tls::configure_static_tls_teb_slot(slot).map_err(|_| {
                LinkError::InvalidTls {
                    object: self.objects[0].name.clone(),
                }
            })?;
        }
        if let Some(slot) = self.host_transition_teb_slot {
            kinakaze_tls::configure_host_transition_teb_slot(slot).map_err(|_| {
                LinkError::InvalidTls {
                    object: self.objects[0].name.clone(),
                }
            })?;
        }
        // A block is installed even with no TPOFF modules: fs:[0] remains Linux's
        // self pointer, and import trampolines always require a published TP.
        kinakaze_tls::install_current_thread_static_tls().map_err(|_| LinkError::InvalidTls {
            object: self.objects[0].name.clone(),
        })?;

        // Static mapping, ordinary relocations, protection and TLS have passed
        // without executing guest code. An exec candidate can now acknowledge
        // readiness. Its manager waits for native death of the old worker (all
        // threads), so even IFUNC side effects occur after the exec boundary.
        // Resolver/constructor failures beyond this point are fatal to the new
        // image; the old image cannot be resumed after its successful commit.
        kinakaze_runtime::authority::activate_image().map_err(|errno| {
            LinkError::InvalidProvider(format!("guest image activation failed: errno {errno}"))
        })?;

        // Only now may resolvers run: they are ordinary guest code.
        // SAFETY: every object above is relocated and protected.
        unsafe { relocate::run_ifuncs(&pending)? };
        self.pending_ifuncs.clear();

        // IRELATIVE and STT_GNU_IFUNC slots may reside in PT_GNU_RELRO. Seal
        // those pages only after writing the resolved addresses; executable
        // segment permissions above already allow the resolvers to run.
        for object in &self.objects[first..] {
            object.apply_relro()?;
        }

        self.initialization_order = self.compute_initialization_order();
        Ok(())
    }

    /// Tells each provider DLL where the guest's copy of its variables lives.
    ///
    /// A `R_X86_64_COPY` gives the executable its own storage for a library's
    /// variable. Copying the initial value across is the visible half of the ABI's
    /// requirement; the other half is that the *library's* own accesses must reach
    /// that same storage afterwards, or the first write on either side makes the two
    /// disagree forever.
    ///
    /// A Linux dynamic linker satisfies that by repointing the library's GOT entry,
    /// because a PIC shared object reaches its own globals indirectly. A Windows DLL
    /// does not: its accesses are direct RIP-relative loads and stores that no
    /// relocation can reach. So the DLL has to cooperate, and each one that cares
    /// exports `kinakaze_copied_redirect(name, address)`.
    ///
    /// The registered adapter must explicitly handle each copied definition it
    /// supplied; a rejected redirect aborts linking instead of leaving two
    /// divergent writable locations.
    ///
    /// Failing to do this is a quiet bug with a confusing symptom. `busybox ls`
    /// parsed `-l` correctly — option handling is entirely inside libc — and then
    /// tried to list a file named `ls`, because libc had advanced its own `optind`
    /// while BusyBox collected operands from the copy in its own `.bss`, still 0.
    fn install_copy_redirects(&self, copies: &[relocate::CopiedVariable]) -> Result<(), LinkError> {
        for dll in &self.scope.dlls {
            for copy in copies {
                dll.redirect_copy(
                    &copy.name,
                    copy.guest_address,
                    copy.size,
                    copy.source_address,
                )?;
            }
        }
        Ok(())
    }

    /// Orders objects so that a dependency is initialized before its dependent.
    ///
    /// A post-order depth-first walk of the dependency graph gives exactly that.
    /// Cycles are possible in real libraries, so the visit set breaks them rather
    /// than recursing forever; the object that closes a cycle simply initializes
    /// after whichever member was reached first.
    fn compute_initialization_order(&self) -> Vec<ObjectId> {
        let mut order = Vec::new();
        let mut visited = vec![false; self.objects.len()];
        for id in &self.scope.order {
            self.visit_for_initialization(*id, &mut visited, &mut order);
        }
        order
    }

    fn visit_for_initialization(
        &self,
        id: ObjectId,
        visited: &mut [bool],
        order: &mut Vec<ObjectId>,
    ) {
        if visited[id.0] {
            return;
        }
        visited[id.0] = true;
        for dependency in &self.objects[id.0].dependencies {
            self.visit_for_initialization(*dependency, visited, order);
        }
        order.push(id);
    }

    /// Runs dependency `DT_INIT` and `DT_INIT_ARRAY` entries in dependency order.
    ///
    /// The main executable is intentionally excluded. Its `DT_PREINIT_ARRAY`
    /// belongs to [`Self::run_executable_preinitializers`], while modern libc
    /// invokes its `DT_INIT` and `DT_INIT_ARRAY` later through
    /// [`Self::run_executable_initializers`].
    ///
    /// # Safety
    ///
    /// Linking must be complete: initializers are guest code and will call into
    /// the rest of the image.
    pub unsafe fn run_initializers(&mut self) -> Result<(), LinkError> {
        // Runtime dlopen constructors have the C `void(void)` contract. Startup
        // uses `run_initializers_with_arguments` so initial dependency
        // constructors receive the process vectors Linux supplies there.
        unsafe { self.run_initializers_with_arguments(0, std::ptr::null(), std::ptr::null()) }
    }

    /// Runs dependency constructors with the process startup arguments.
    ///
    /// # Safety
    ///
    /// Linking must be complete and `argv`/`envp` must remain valid throughout
    /// every guest call.
    pub unsafe fn run_initializers_with_arguments(
        &mut self,
        argc: i32,
        argv: *const *const u8,
        envp: *const *const u8,
    ) -> Result<(), LinkError> {
        for id in self.initialization_order.clone() {
            if self.objects[id.0].is_executable || self.objects[id.0].initialized {
                continue;
            }
            self.objects[id.0].initialized = true;
            // SAFETY: forwarded from this function's contract.
            unsafe { self.run_object_initializers(id, argc, argv, envp)? };
        }
        Ok(())
    }

    /// Runs the main executable's `DT_PREINIT_ARRAY` before dependency
    /// constructors, matching the dynamic-loader phase of Linux startup.
    ///
    /// # Safety
    ///
    /// Linking must be complete and `argv`/`envp` must describe the live initial
    /// process stack.
    pub unsafe fn run_executable_preinitializers(
        &mut self,
        argc: i32,
        argv: *const *const u8,
        envp: *const *const u8,
    ) -> Result<(), LinkError> {
        let id = self.executable_object()?;
        if self.objects[id.0].preinitialized {
            return Ok(());
        }
        // Mark before entering guest code so a recursive loader callback cannot
        // execute the same preinit array twice.
        self.objects[id.0].preinitialized = true;
        let object = &self.objects[id.0];
        if let Some(table) = object.dynamic.preinit_array {
            // SAFETY: forwarded from this function's contract.
            unsafe { self.call_function_array_with(object, table, argc, argv, envp)? };
        }
        Ok(())
    }

    /// Runs the main executable's `DT_INIT` and `DT_INIT_ARRAY` from libc startup.
    ///
    /// # Safety
    ///
    /// The executable preinit phase and dependency initialization must already be
    /// complete, and the supplied process vectors must remain live.
    pub unsafe fn run_executable_initializers(
        &mut self,
        argc: i32,
        argv: *const *const u8,
        envp: *const *const u8,
    ) -> Result<(), LinkError> {
        let id = self.executable_object()?;
        if self.objects[id.0].initialized {
            return Ok(());
        }
        if !self.objects[id.0].preinitialized {
            return Err(LinkError::ExecutablePreinitNotRun);
        }
        // Mark before entering guest code to break constructor/dlopen cycles.
        self.objects[id.0].initialized = true;
        let object = &self.objects[id.0];

        if let Some(address) = object.dynamic.init {
            let address = object.resolve_address(address)?;
            // SAFETY: DT_INIT is a relocated initializer in the executable.
            unsafe { call_initializer_with(address, argc, argv, envp) };
        }
        if let Some(table) = object.dynamic.init_array {
            // SAFETY: forwarded from this function's contract.
            unsafe { self.call_function_array_with(object, table, argc, argv, envp)? };
        }
        Ok(())
    }

    fn executable_object(&self) -> Result<ObjectId, LinkError> {
        let mut executable = self
            .objects
            .iter()
            .enumerate()
            .filter_map(|(index, object)| object.is_executable.then_some(ObjectId(index)));
        let Some(id) = executable.next() else {
            return Err(LinkError::NoExecutableObject);
        };
        if executable.next().is_some() {
            return Err(LinkError::MultipleExecutableObjects);
        }
        Ok(id)
    }

    /// # Safety
    ///
    /// As [`Self::run_initializers`].
    unsafe fn run_object_initializers(
        &self,
        id: ObjectId,
        argc: i32,
        argv: *const *const u8,
        envp: *const *const u8,
    ) -> Result<(), LinkError> {
        let object = &self.objects[id.0];

        // DT_INIT precedes DT_INIT_ARRAY, as the ABI specifies.
        if let Some(address) = object.dynamic.init {
            let address = object.resolve_address(address)?;
            // SAFETY: DT_INIT names an initializer in this image.
            unsafe { call_initializer_with(address, argc, argv, envp) };
        }

        if let Some(table) = object.dynamic.init_array {
            // SAFETY: forwarded from this function's contract.
            unsafe { self.call_function_array_with(object, table, argc, argv, envp)? };
        }
        Ok(())
    }

    /// # Safety
    ///
    /// The table must name function pointers inside a linked image, and the
    /// process-vector pointers must be valid for every call.
    unsafe fn call_function_array_with(
        &self,
        object: &MappedObject,
        table: kinakaze_elf::TableLocation,
        argc: i32,
        argv: *const *const u8,
        envp: *const *const u8,
    ) -> Result<(), LinkError> {
        for index in 0..function_array_count(table) {
            let address = self.function_array_entry(object, table, index)?;
            if address == 0 || address == usize::MAX {
                // Linkers emit 0 and -1 as ignorable padding in these arrays.
                continue;
            }
            // SAFETY: forwarded from this function's contract.
            unsafe { call_initializer_with(address, argc, argv, envp) };
        }
        Ok(())
    }

    /// Reads one entry from an init/fini array.
    ///
    /// The array is read from the *mapped image*, not the file: its entries are
    /// relocated by `R_X86_64_RELATIVE`, so the file still holds link-time
    /// addresses while memory holds the real ones.
    fn function_array_entry(
        &self,
        object: &MappedObject,
        table: kinakaze_elf::TableLocation,
        index: u64,
    ) -> Result<usize, LinkError> {
        let address = table
            .address
            .checked_add(index * 8)
            .ok_or(LinkError::AddressOverflow)?;
        let slot = object.resolve_address(address)? as *const u64;
        // SAFETY: the slot lies inside this object's mapping and the array was
        // sized from DT_*_ARRAYSZ.
        let value = unsafe { slot.read_unaligned() };
        Ok(value as usize)
    }

    /// Resolves a symbol as `dlsym` would.
    pub fn lookup(&self, name: &str) -> Result<Option<Resolution>, LinkError> {
        if let Some(address) = self.builtins.get(name) {
            return Ok(Some(Resolution::from_address(*address)));
        }
        self.scope.resolve(&self.objects, name, None)
    }

    /// Bind libc's public buffers to the same allocator used by ELF callers.
    /// Called after relocation/TLS activation and before guest constructors.
    pub unsafe fn install_c_allocator(&mut self) -> Result<(), LinkError> {
        let mut entries = [0; 3];
        for (slot, name) in entries.iter_mut().zip(["malloc", "realloc", "free"]) {
            let Some(resolution) = self.lookup(name)? else {
                continue;
            };
            let (Some(owner), Some(address)) = (resolution.owner, resolution.address) else {
                continue; // Native libc retains its direct allocator path.
            };
            self.mark_runtime_nodelete(RuntimeHandle::Elf(owner))?;
            *slot = if resolution.is_ifunc {
                let resolver: unsafe extern "sysv64" fn() -> usize =
                    unsafe { core::mem::transmute(address) };
                unsafe { resolver() }
            } else {
                address
            };
        }
        self.c_allocator = entries;
        unsafe { self.restore_c_allocator() };
        Ok(())
    }

    pub(crate) unsafe fn restore_c_allocator(&self) {
        let [malloc, realloc, free] = self.c_allocator;
        unsafe { kinakaze_alloc::c::install(malloc, realloc, free) };
    }

    /// Opens either an ELF image or a bound facade at runtime and returns an
    /// opaque handle. The handle table is ordinary managed process state, so a
    /// fork child receives the same tokens and logical reference counts.
    ///
    /// # Safety
    ///
    /// Initializers in a newly loaded ELF graph execute guest code. The caller
    /// must invoke this only at a valid host-to-guest dynamic-loader boundary.
    pub unsafe fn open_runtime(
        &mut self,
        path: Option<&Path>,
        flags: i32,
    ) -> Result<usize, LinkError> {
        let binding = flags & (RTLD_LAZY | RTLD_NOW);
        if !matches!(binding, RTLD_LAZY | RTLD_NOW) || flags & !RTLD_KNOWN_FLAGS != 0 {
            return Err(LinkError::InvalidOpenFlags(flags));
        }

        let target = match path {
            None => RuntimeHandle::Main,
            Some(path) => {
                let candidate = self.find_runtime_candidate(path)?;
                if flags & RTLD_NOLOAD != 0 {
                    let target = self
                        .loaded_runtime_target(&candidate)
                        .ok_or_else(|| LinkError::ObjectNotLoaded(candidate.clone()))?;
                    self.retain_runtime_target(target)?;
                    target
                } else {
                    let requested = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    let first_object = self.objects.len();
                    let first_provider = self.scope.providers.len();
                    let checkpoint = loading::Checkpoint::capture(self);
                    let loaded = match self.load_candidate(&candidate, &requested, "dlopen") {
                        Ok(provider) => provider,
                        Err(error) => {
                            checkpoint.restore(self);
                            return Err(error);
                        }
                    };
                    let target = match loaded {
                        Provider::Dll(index) => RuntimeHandle::Dll(index),
                        Provider::Elf(id) => RuntimeHandle::Elf(id),
                    };

                    if self.objects.len() != first_object {
                        // Either kind of new group may reuse an RTLD_LOCAL
                        // dependency. It must be visible during relocation;
                        // GLOBAL promotion after linking would be too late.
                        let temporary = self.publish_existing_dependencies(target, first_provider);
                        let linked = self.link_from(first_object);
                        if flags & RTLD_GLOBAL == 0 {
                            for index in temporary {
                                self.scope.globals[index] = false;
                            }
                        }
                        if let Err(error) = linked {
                            checkpoint.restore(self);
                            return Err(error);
                        }
                        // Relocation sees the just-created dependency group, as it
                        // must, but RTLD_LOCAL is applied before constructors can
                        // observe the process-wide scope through RTLD_DEFAULT.
                        if flags & RTLD_GLOBAL == 0 {
                            for global in &mut self.scope.globals[first_provider..] {
                                *global = false;
                            }
                        }
                        // SAFETY: the newly appended graph was linked above.
                        unsafe { self.run_initializers()? };
                    } else if flags & RTLD_GLOBAL == 0 {
                        // A newly loaded PE provider has no ELF relocation phase.
                        for global in &mut self.scope.globals[first_provider..] {
                            *global = false;
                        }
                    }
                    target
                }
            }
        };
        if flags & RTLD_NODELETE != 0 {
            self.mark_runtime_nodelete(target)?;
        }
        if flags & RTLD_GLOBAL != 0 {
            self.promote_runtime_target(target);
        }
        Ok(self.allocate_runtime_handle(target))
    }

    fn loaded_runtime_target(&self, path: &Path) -> Option<RuntimeHandle> {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some((index, _)) = self
            .objects
            .iter()
            .enumerate()
            .find(|(_, object)| object.path == canonical)
        {
            return Some(RuntimeHandle::Elf(ObjectId(index)));
        }
        self.scope
            .dlls
            .iter()
            .enumerate()
            .find(|(_, dll)| dll.path == canonical && dll.is_loaded())
            .map(|(index, _)| RuntimeHandle::Dll(index))
    }

    fn retain_runtime_target(&mut self, target: RuntimeHandle) -> Result<(), LinkError> {
        match target {
            RuntimeHandle::Main => Ok(()),
            RuntimeHandle::Elf(id) => {
                let object = self.objects.get_mut(id.0).ok_or(LinkError::InvalidHandle)?;
                object.references = object.references.saturating_add(1);
                Ok(())
            }
            RuntimeHandle::Dll(index) => self
                .scope
                .dlls
                .get_mut(index)
                .ok_or(LinkError::InvalidHandle)?
                .add_reference(),
        }
    }

    fn mark_runtime_nodelete(&mut self, target: RuntimeHandle) -> Result<(), LinkError> {
        match target {
            RuntimeHandle::Main => Ok(()),
            RuntimeHandle::Elf(id) => {
                self.objects
                    .get_mut(id.0)
                    .ok_or(LinkError::InvalidHandle)?
                    .nodelete = true;
                Ok(())
            }
            RuntimeHandle::Dll(index) => {
                self.scope
                    .dlls
                    .get_mut(index)
                    .ok_or(LinkError::InvalidHandle)?
                    .mark_nodelete();
                Ok(())
            }
        }
    }

    fn promote_runtime_target(&mut self, target: RuntimeHandle) {
        for provider in self.runtime_provider_closure(target) {
            self.scope.set_global(provider, true);
        }
    }

    fn publish_existing_dependencies(
        &mut self,
        target: RuntimeHandle,
        first_new_provider: usize,
    ) -> Vec<usize> {
        let closure = self.runtime_provider_closure(target);
        let mut changed = Vec::new();
        for provider in closure {
            if let Some(index) = self
                .scope
                .providers
                .iter()
                .position(|candidate| *candidate == provider)
                && index < first_new_provider
                && !self.scope.globals[index]
            {
                self.scope.globals[index] = true;
                changed.push(index);
            }
        }
        changed
    }

    fn runtime_provider_closure(&self, target: RuntimeHandle) -> Vec<Provider> {
        let mut providers = Vec::new();
        let mut pending = match target {
            RuntimeHandle::Main => Vec::new(),
            RuntimeHandle::Elf(id) => vec![Provider::Elf(id)],
            RuntimeHandle::Dll(index) => vec![Provider::Dll(index)],
        };
        let mut visited_elf = vec![false; self.objects.len()];
        let mut visited_dll = vec![false; self.scope.dlls.len()];
        while let Some(provider) = pending.pop() {
            match provider {
                Provider::Elf(id) => {
                    if std::mem::replace(&mut visited_elf[id.0], true) {
                        continue;
                    }
                    if let Some(dependencies) = self.provider_dependencies.get(id.0) {
                        pending.extend(dependencies.iter().copied());
                    }
                }
                Provider::Dll(index) => {
                    if std::mem::replace(&mut visited_dll[index], true) {
                        continue;
                    }
                }
            }
            providers.push(provider);
        }
        providers
    }

    fn find_runtime_candidate(&self, path: &Path) -> Result<PathBuf, LinkError> {
        let mut searched = Vec::new();
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        if let Some(image) = self
            .registry
            .by_path(path)
            .or_else(|| self.registry.get(&file_name))
        {
            return Ok(image.path().to_path_buf());
        }

        // A bare SONAME may already be resident through another object's
        // RUNPATH. Reuse it before searching the caller's directories, just as
        // DT_NEEDED does; an explicit pathname still names that exact object.
        if path
            .parent()
            .is_none_or(|parent| parent.as_os_str().is_empty())
        {
            if let Some(object) = self.objects.iter().find(|object| {
                object.name == file_name
                    || object
                        .path
                        .file_name()
                        .is_some_and(|name| name == path.as_os_str())
            }) {
                return Ok(object.path.clone());
            }
        }

        if let Some(resolved) = self.resolve_candidate(path) {
            return Ok(resolved);
        }
        searched.push(path.to_path_buf());

        // A path containing a directory component is an explicit request. Bare
        // SONAMEs use the process search path, matching Linux `dlopen`.
        if path
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
        {
            return Err(LinkError::MissingDependency {
                name: path.display().to_string(),
                requested_by: "dlopen".to_owned(),
                searched,
            });
        }

        let name = path.to_string_lossy();
        let directories = self
            .library_paths
            .iter()
            .chain(self.paths.extra.iter())
            .chain(self.system_paths.iter());
        for directory in directories {
            let candidate = directory.join(path);
            if let Some(resolved) = self.resolve_candidate(&candidate) {
                return Ok(resolved);
            }
            searched.push(candidate);
        }

        Err(LinkError::MissingDependency {
            name: name.into_owned(),
            requested_by: "dlopen".to_owned(),
            searched,
        })
    }

    fn resolve_candidate(&self, candidate: &Path) -> Option<PathBuf> {
        let name = candidate.to_string_lossy();
        let resolved = if name.starts_with('/') {
            // Resolve the complete filename: resolving only the directory can
            // choose an upper overlay layer while its DSO lives in a lower one.
            let guest = name.replace('\\', "/");
            if self.paths.process_namespace {
                kinakaze_vfs::resolve_linux_path(&guest).ok()?
            } else {
                kinakaze_vfs::path::resolve_linux_path_from(&self.paths.host_directory, &guest)
                    .ok()?
            }
        } else {
            candidate.to_path_buf()
        };
        resolved.is_file().then_some(resolved)
    }

    fn allocate_runtime_handle(&mut self, target: RuntimeHandle) -> usize {
        loop {
            let handle = self.next_runtime_handle;
            self.next_runtime_handle = self.next_runtime_handle.wrapping_add(0x10);
            if handle != 0 && self.runtime_handles.insert(handle, target).is_none() {
                return handle;
            }
        }
    }

    /// Resolves one name through a runtime handle. `None` selects the global
    /// scope used by RTLD_DEFAULT/RTLD_NEXT.
    pub fn lookup_runtime(
        &self,
        handle: Option<usize>,
        name: &str,
    ) -> Result<Option<Resolution>, LinkError> {
        let Some(handle) = handle else {
            return self.lookup(name);
        };
        match self
            .runtime_handles
            .get(&handle)
            .copied()
            .ok_or(LinkError::InvalidHandle)?
        {
            RuntimeHandle::Main => self.lookup(name),
            RuntimeHandle::Elf(id) => {
                if let Some(address) = self.builtins.get(name) {
                    Ok(Some(Resolution::from_address(*address)))
                } else {
                    self.lookup_in(id, name)
                }
            }
            RuntimeHandle::Dll(index) => self
                .scope
                .dlls
                .get(index)
                .ok_or(LinkError::InvalidHandle)?
                .resolve(name, None),
        }
    }

    /// Looks up an explicit ABI version for dlvsym; host builtins carry no
    /// declared versions and therefore cannot satisfy this request implicitly.
    pub fn lookup_runtime_version(
        &self,
        handle: Option<usize>,
        name: &str,
        version: &str,
    ) -> Result<Option<Resolution>, LinkError> {
        let Some(handle) = handle else {
            return self.scope.resolve(&self.objects, name, Some(version));
        };
        match self
            .runtime_handles
            .get(&handle)
            .copied()
            .ok_or(LinkError::InvalidHandle)?
        {
            RuntimeHandle::Main => self.scope.resolve(&self.objects, name, Some(version)),
            RuntimeHandle::Elf(id) => self.lookup_in_version(id, name, Some(version)),
            RuntimeHandle::Dll(index) => self
                .scope
                .dlls
                .get(index)
                .ok_or(LinkError::InvalidHandle)?
                .resolve(name, Some(version)),
        }
    }

    pub fn lookup_next_version(
        &self,
        caller_address: usize,
        name: &str,
        version: &str,
    ) -> Result<Option<Resolution>, LinkError> {
        let caller = self
            .owner_of_address(caller_address)
            .ok_or(LinkError::InvalidCallerAddress(caller_address))?;
        self.scope
            .resolve_after(&self.objects, caller, name, Some(version))
    }

    /// Resolves RTLD_NEXT relative to the ELF object containing the call site.
    pub fn lookup_next(
        &self,
        caller_address: usize,
        name: &str,
    ) -> Result<Option<Resolution>, LinkError> {
        let caller = self
            .owner_of_address(caller_address)
            .ok_or(LinkError::InvalidCallerAddress(caller_address))?;
        self.scope.resolve_after(&self.objects, caller, name, None)
    }

    /// Releases one runtime handle. PE images are actually unloaded when their
    /// logical reference count reaches zero. ELF mappings remain resident for
    /// now, as permitted for NODELETE/reference-retained objects, but their
    /// explicit dlopen count is decremented.
    pub fn close_runtime(&mut self, handle: usize) -> Result<(), LinkError> {
        match self
            .runtime_handles
            .remove(&handle)
            .ok_or(LinkError::InvalidHandle)?
        {
            RuntimeHandle::Main => {}
            RuntimeHandle::Elf(id) => {
                let object = self.objects.get_mut(id.0).ok_or(LinkError::InvalidHandle)?;
                object.references = object.references.saturating_sub(1);
            }
            RuntimeHandle::Dll(index) => {
                let dll = self
                    .scope
                    .dlls
                    .get_mut(index)
                    .ok_or(LinkError::InvalidHandle)?;
                dll.release_reference();
                if !dll.is_loaded() {
                    self.scope.set_global(Provider::Dll(index), false);
                }
            }
        }
        Ok(())
    }

    /// Resolves a symbol starting from one object, as `dlsym(handle, ...)` does.
    pub fn lookup_in(&self, id: ObjectId, name: &str) -> Result<Option<Resolution>, LinkError> {
        self.lookup_in_version(id, name, None)
    }

    fn lookup_in_version(
        &self,
        id: ObjectId,
        name: &str,
        version: Option<&str>,
    ) -> Result<Option<Resolution>, LinkError> {
        let mut visited_elf = vec![false; self.objects.len()];
        let mut visited_dll = vec![false; self.scope.dlls.len()];
        self.lookup_in_provider(
            Provider::Elf(id),
            name,
            version,
            &mut visited_elf,
            &mut visited_dll,
        )
    }

    fn lookup_in_provider(
        &self,
        provider: Provider,
        name: &str,
        version: Option<&str>,
        visited_elf: &mut [bool],
        visited_dll: &mut [bool],
    ) -> Result<Option<Resolution>, LinkError> {
        match provider {
            Provider::Dll(index) => {
                let dll = self.scope.dlls.get(index).ok_or(LinkError::InvalidHandle)?;
                if std::mem::replace(
                    visited_dll.get_mut(index).ok_or(LinkError::InvalidHandle)?,
                    true,
                ) {
                    return Ok(None);
                }
                dll.resolve(name, version)
            }
            Provider::Elf(id) => {
                let object = self.objects.get(id.0).ok_or(LinkError::InvalidHandle)?;
                if std::mem::replace(
                    visited_elf.get_mut(id.0).ok_or(LinkError::InvalidHandle)?,
                    true,
                ) {
                    return Ok(None);
                }
                if let Some(symbol) = object.lookup_versioned(name, version)? {
                    if object.is_tls(symbol) {
                        return Ok(Some(Resolution {
                            owner: Some(id),
                            address: None,
                            size: symbol.size,
                            tls_module: object.tls_module,
                            tls_offset: Some(
                                usize::try_from(symbol.value)
                                    .map_err(|_| LinkError::AddressOverflow)?,
                            ),
                            is_ifunc: false,
                            is_trap: false,
                        }));
                    }
                    return Ok(Some(Resolution {
                        owner: Some(id),
                        address: Some(object.definition_address(symbol)?),
                        size: symbol.size,
                        tls_module: None,
                        tls_offset: None,
                        is_ifunc: object.is_ifunc(symbol),
                        is_trap: false,
                    }));
                }
                for dependency in self
                    .provider_dependencies
                    .get(id.0)
                    .ok_or(LinkError::InvalidHandle)?
                    .iter()
                    .copied()
                {
                    if let Some(found) = self.lookup_in_provider(
                        dependency,
                        name,
                        version,
                        visited_elf,
                        visited_dll,
                    )? {
                        return Ok(Some(found));
                    }
                }
                Ok(None)
            }
        }
    }

    /// Finds which object contains an address, for `dladdr`.
    pub fn owner_of_address(&self, address: usize) -> Option<ObjectId> {
        self.objects.iter().enumerate().find_map(|(index, object)| {
            let start = object.allocation as usize;
            let end = start + object.allocation_len;
            (start..end).contains(&address).then_some(ObjectId(index))
        })
    }

    pub fn symbol_for_address(&self, address: usize) -> Result<Option<(String, usize)>, LinkError> {
        let Some(owner) = self.owner_of_address(address) else {
            return Ok(None);
        };
        self.objects[owner.0].symbol_for_address(address)
    }

    /// The entry point of the root object.
    pub fn entry(&self, id: ObjectId) -> Result<usize, LinkError> {
        self.objects
            .get(id.0)
            .ok_or(LinkError::InvalidHandle)?
            .entry()
    }

    /// Collects the auxiliary-vector facts about an object.
    ///
    /// `AT_PHDR` has to be the *mapped* address of the program headers, not their
    /// file offset: glibc walks them to find `PT_DYNAMIC` and `PT_TLS`, so a file
    /// offset would send it into unmapped memory. The headers are reachable in
    /// memory only when a `PT_PHDR` segment declares where they were loaded, which
    /// is why its absence is reported rather than guessed at.
    pub fn auxiliary_values(
        &self,
        id: ObjectId,
    ) -> Result<crate::stack::AuxiliaryValues, LinkError> {
        let object = self.objects.get(id.0).ok_or(LinkError::InvalidHandle)?;
        let elf = object.elf()?;
        let header = elf.header();
        let headers = elf.program_headers()?;

        let program_headers = headers
            .iter()
            .find(|entry| entry.kind == kinakaze_elf::PT_PHDR)
            .and_then(|entry| object.resolve_address(entry.virtual_address).ok());

        Ok(crate::stack::AuxiliaryValues {
            program_headers,
            program_header_size: u64::from(header.program_entry_size),
            program_header_count: u64::from(header.program_count),
            entry: object.entry().ok(),
            // No separate interpreter is mapped: this linker *is* the interpreter,
            // so there is no second base address to report.
            interpreter_base: None,
            page_size: crate::object::PAGE_SIZE,
            executable_path: Some(object.path.display().to_string()),
        })
    }
}

fn function_array_count(table: kinakaze_elf::TableLocation) -> u64 {
    table.size.unwrap_or(0) / 8
}

/// # Safety
///
/// `address` must name an initializer in a linked image and the process vectors
/// must remain valid for the call.
unsafe fn call_initializer_with(
    address: usize,
    argc: i32,
    argv: *const *const u8,
    envp: *const *const u8,
) {
    let function: unsafe extern "sysv64" fn(i32, *const *const u8, *const *const u8) =
        // SAFETY: forwarded from this function's contract.
        unsafe { std::mem::transmute(address) };
    // SAFETY: as above.
    unsafe { function(argc, argv, envp) };
}

/// Reads only native ELF. PE files and unbound generated facades must never
/// enter the generic ELF mapping path: registry binding is mandatory.
fn read_elf_candidate(path: &Path) -> Result<crate::ImmutableBytes, LinkError> {
    let snapshot = profile::begin("image-snapshot", path.display());
    let image = crate::snapshot_guest_image(path)?;
    drop(snapshot);
    if image.starts_with(b"MZ") {
        return Err(LinkError::InvalidProvider(format!(
            "{} is PE; register a paired ELF facade instead",
            path.display()
        )));
    }
    reject_unbound_facade(&image, path)?;
    Ok(image)
}

fn reject_unbound_facade(image: &[u8], path: &Path) -> Result<(), LinkError> {
    // The bridge format has a private ELF note. Look only in note segments,
    // never arbitrary .rodata, so an unrelated ELF string cannot trigger this.
    let elf = ElfFile::parse(image)?;
    for header in elf.program_headers()? {
        if header.kind != 4 {
            continue;
        } // PT_NOTE
        let start = usize::try_from(header.offset).map_err(|_| LinkError::AddressOverflow)?;
        let len = usize::try_from(header.file_size).map_err(|_| LinkError::AddressOverflow)?;
        let end = start.checked_add(len).ok_or(LinkError::AddressOverflow)?;
        let mut notes = &image[start..end];
        while notes.len() >= 12 {
            let namesz = u32::from_le_bytes(notes[0..4].try_into().unwrap()) as usize;
            let descsz = u32::from_le_bytes(notes[4..8].try_into().unwrap()) as usize;
            let kind = u32::from_le_bytes(notes[8..12].try_into().unwrap());
            if kind == 0x4352_5932
                && notes.get(12..12usize.saturating_add(namesz)) == Some(b"KINAKAZE\0")
            {
                return Err(LinkError::InvalidProvider(format!(
                    "{} is an unbound facade; register its paired image first",
                    path.display()
                )));
            }
            let padded_name = namesz.checked_add(3).ok_or(LinkError::AddressOverflow)? & !3;
            let padded_desc = descsz.checked_add(3).ok_or(LinkError::AddressOverflow)? & !3;
            let next = 12usize
                .checked_add(padded_name)
                .and_then(|at| at.checked_add(padded_desc))
                .ok_or(LinkError::AddressOverflow)?;
            let Some(rest) = notes.get(next..) else {
                break;
            };
            notes = rest;
        }
    }
    Ok(())
}

/// Expands a `DT_RUNPATH` string into host directories.
///
/// `$ORIGIN` is the directory of the object that declared the path, which is how
/// relocatable installs find their own libraries.
///
/// Every entry is a *guest* path, and the two kinds have to be treated
/// differently. `$ORIGIN` is already a native object directory. Plain guest
/// paths stay unresolved until the SONAME is appended, so merged directories
/// do not accidentally pin all subsequent lookups to one overlay layer.
pub fn expand_search_path(raw: &str, object_directory: &Path, _guest_root: &Path) -> Vec<PathBuf> {
    let origin = object_directory.to_string_lossy();
    raw.split([':', ';'])
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let from_origin = entry.contains("$ORIGIN") || entry.contains("${ORIGIN}");
            let expanded = entry
                .replace("$ORIGIN", &origin)
                .replace("${ORIGIN}", &origin);
            if from_origin {
                // Already a host directory, because `origin` is where the
                // object was actually loaded from. Translating it a second time
                // would push the search under the guest root twice and send
                // every `$ORIGIN`-relative install looking in the wrong place.
                PathBuf::from(expanded)
            } else {
                // Keep guest paths until the SONAME is appended and looked up.
                PathBuf::from(expanded)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unbound_facade_note_is_rejected_without_rejecting_unrelated_elf_notes() {
        let mut elf = vec![0u8; 160];
        elf[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        elf[16..18].copy_from_slice(&3u16.to_le_bytes());
        elf[18..20].copy_from_slice(&62u16.to_le_bytes());
        elf[20..24].copy_from_slice(&1u32.to_le_bytes());
        elf[32..40].copy_from_slice(&64u64.to_le_bytes());
        elf[52..54].copy_from_slice(&64u16.to_le_bytes());
        elf[54..56].copy_from_slice(&56u16.to_le_bytes());
        elf[56..58].copy_from_slice(&1u16.to_le_bytes());
        elf[64..68].copy_from_slice(&4u32.to_le_bytes());
        elf[72..80].copy_from_slice(&120u64.to_le_bytes());
        elf[96..104].copy_from_slice(&40u64.to_le_bytes());
        elf[104..112].copy_from_slice(&40u64.to_le_bytes());
        elf[120..124].copy_from_slice(&9u32.to_le_bytes());
        elf[124..128].copy_from_slice(&16u32.to_le_bytes());
        elf[128..132].copy_from_slice(&0x4352_5932u32.to_le_bytes());
        elf[132..141].copy_from_slice(b"KINAKAZE\0");
        assert!(matches!(
            reject_unbound_facade(&elf, Path::new("libc.so.6")),
            Err(LinkError::InvalidProvider(_))
        ));
        // Same text in another vendor's note payload has no facade semantics.
        elf[128..132].copy_from_slice(&7u32.to_le_bytes());
        assert!(reject_unbound_facade(&elf, Path::new("native.so")).is_ok());
        elf[64..68].copy_from_slice(&1u32.to_le_bytes()); // ordinary PT_LOAD data
        elf[128..132].copy_from_slice(&0x4352_5932u32.to_le_bytes());
        assert!(reject_unbound_facade(&elf, Path::new("native.so")).is_ok());
    }

    #[test]
    fn runpath_expansion_resolves_origin() {
        let directory = Path::new("/opt/app/bin");
        let root = Path::new("/guest/root");
        let expanded = expand_search_path("$ORIGIN/../lib:/usr/lib", directory, root);
        assert_eq!(expanded.len(), 2);
        // `$ORIGIN` is a host directory already, so it is used verbatim.
        assert_eq!(expanded[0], PathBuf::from("/opt/app/bin/../lib"));
        // Guest paths stay unresolved until a complete filename is available.
        assert_eq!(expanded[1], PathBuf::from("/usr/lib"));

        // The braced form means the same thing.
        let braced = expand_search_path("${ORIGIN}/lib", directory, root);
        assert_eq!(braced[0], PathBuf::from("/opt/app/bin/lib"));

        // Empty entries are dropped rather than becoming the current directory,
        // which would silently widen the search.
        assert!(expand_search_path("::", directory, root).is_empty());
    }

    /// `libpulse.so.0` declares `/usr/lib/x86_64-linux-gnu/pulseaudio`, and its
    /// `libpulsecommon-16.1.so` lives only there. Without translation the
    /// directory is joined into `/usr/lib/...`, which no host call resolves, so
    /// the library is reported missing while sitting on disk.
    #[test]
    fn runpath_absolute_entry_keeps_the_guest_namespace_until_lookup() {
        let root = Path::new(r"E:\app");
        let expanded = expand_search_path(
            "/usr/lib/x86_64-linux-gnu/pulseaudio",
            Path::new(r"E:\app\usr\lib\x86_64-linux-gnu"),
            root,
        );
        assert_eq!(
            expanded[0],
            PathBuf::from("/usr/lib/x86_64-linux-gnu/pulseaudio")
        );
    }

    #[test]
    fn test_load_libcrypto() {
        let p = Path::new("../../target/debug/usr/lib/libcrypto.so.3");
        if !p.exists() {
            println!("Path {:?} does not exist!", p);
            return;
        }
        let obj = MappedObject::map(p, None).unwrap();
        let sym = obj.lookup("OPENSSL_init_crypto").unwrap();
        println!(
            "test_load_libcrypto: sym = {:?}",
            sym.as_ref()
                .map(|s| (s.name, s.index, s.value, s.is_defined(), s.is_local_only()))
        );
        if let Some(s) = sym {
            let ver = obj.definition_version(s.index);
            let hidden = obj.definition_is_hidden(s.index);
            println!(
                "test_load_libcrypto: ver = {:?}, hidden = {:?}",
                ver, hidden
            );
        }
    }

    #[test]
    fn test_load_node() {
        let p = Path::new("../../target/debug/usr/bin/node");
        if !p.exists() {
            println!("Path {:?} does not exist!", p);
            return;
        }
        let t0 = std::time::Instant::now();
        let host_dir = Path::new("../../target/debug").canonicalize().unwrap();
        let mut linker = Linker::new(SearchPaths::with_host_directory(host_dir));
        linker.set_unresolved_policy(crate::trap::UnresolvedPolicy::Trap);
        let root = linker.load(p).unwrap();
        println!(
            "Loaded root: {:?} in {}ms, total objects: {}",
            root,
            t0.elapsed().as_millis(),
            linker.objects().len()
        );
        for (i, obj) in linker.objects().iter().enumerate() {
            println!("  [{i}] name: {}, path: {:?}", obj.name, obj.path);
        }
        let sym = linker.lookup("__once_proxy");
        println!("Lookup __once_proxy: {:?}", sym);
    }
}
