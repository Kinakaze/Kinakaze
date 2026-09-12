//! A failed dlopen must not expose a partially relocated dependency graph to
//! later symbol lookups or constructor/finalizer traversal.
use super::*;

pub(super) struct Checkpoint {
    object_references: Vec<usize>,
    dll_references: Vec<usize>,
    order: Vec<ObjectId>,
    providers: Vec<Provider>,
    globals: Vec<bool>,
    pending_ifuncs: Vec<PendingIfunc>,
    initialization_order: Vec<ObjectId>,
    patch_counts: [usize; 3],
}

impl Checkpoint {
    pub(super) fn capture(linker: &Linker) -> Self {
        Self {
            object_references: linker.objects.iter().map(|o| o.references).collect(),
            dll_references: linker.scope.dlls.iter().map(|o| o.references).collect(),
            order: linker.scope.order.clone(),
            providers: linker.scope.providers.clone(),
            globals: linker.scope.globals.clone(),
            pending_ifuncs: linker.pending_ifuncs.clone(),
            initialization_order: linker.initialization_order.clone(),
            patch_counts: [
                linker.patched_canary_sites,
                linker.patched_syscall_sites,
                linker.unpatched_sites,
            ],
        }
    }

    pub(super) fn restore(self, linker: &mut Linker) {
        linker.scope.order = self.order;
        linker.scope.providers = self.providers;
        linker.scope.globals = self.globals;
        linker.pending_ifuncs = self.pending_ifuncs;
        linker.initialization_order = self.initialization_order;
        linker
            .provider_dependencies
            .truncate(self.object_references.len());
        linker.objects.truncate(self.object_references.len());
        linker.scope.dlls.truncate(self.dll_references.len());
        for (object, references) in linker.objects.iter_mut().zip(self.object_references) {
            object.references = references;
        }
        for (provider, references) in linker.scope.dlls.iter_mut().zip(self.dll_references) {
            provider.references = references;
        }
        [
            linker.patched_canary_sites,
            linker.patched_syscall_sites,
            linker.unpatched_sites,
        ] = self.patch_counts;
        // TLS IDs, like those of dlclosed objects, remain monotonic. Templates
        // own their bytes; reusing an ID could alias another thread's old DTV
        // slot or static TLS block. No failed object remains in lookup scope.
    }
}
