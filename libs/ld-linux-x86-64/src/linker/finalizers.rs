//! Resolve teardown once; keep pending calls in the loader's forkable state.
use super::{Linker, function_array_count};
use crate::LinkError;

impl Linker {
    pub(crate) fn next_finalizer(&mut self) -> Result<Option<usize>, LinkError> {
        if self.finalizers.is_none() {
            let mut pending = Vec::new();
            // Popping reverses both dependency order and each DT_FINI_ARRAY.
            // ELF mappings remain resident across dlclose, including during
            // teardown. Resolve addresses before any guest callback can dlopen
            // and relocate the metadata Vec. No metadata borrow spans a call.
            for id in &self.initialization_order {
                let object = &self.objects[id.0];
                if !object.initialized {
                    continue;
                }
                if let Some(address) = object.dynamic.fini {
                    pending.push(object.resolve_address(address)?);
                }
                if let Some(table) = object.dynamic.fini_array {
                    for index in 0..function_array_count(table) {
                        let address = self.function_array_entry(object, table, index)?;
                        if address != 0 && address != usize::MAX {
                            pending.push(address);
                        }
                    }
                }
            }
            self.finalizers = Some(pending);
        }
        // Remove before invoking: recursion cannot repeat a callback, and a
        // fork inside it inherits only work that has not started.
        Ok(self.finalizers.as_mut().unwrap().pop())
    }
}
