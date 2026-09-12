//! Pipelines, shaders, descriptors, render passes and samplers. 34 entry points.
//!
//! The objects a guest builds once and then binds repeatedly. Almost every one is a
//! create/destroy pair, which makes the allocator argument the thing to get right:
//! 30 of the 34 take one, and declaring it as a plain pointer rather than
//! [`crate::forward::Allocator`] compiles cleanly and then has the driver calling the
//! guest's allocator with the wrong convention. The test at the bottom parses this
//! file's own text to check every declaration against a written-out list, because
//! nothing else can catch it.
//!
//! Two pairs in here are *not* create/destroy despite looking like it.
//! `vkAllocateDescriptorSets` and `vkFreeDescriptorSets` take no allocator: descriptor
//! sets come out of a pool, which owns their memory. And `vkGetRenderAreaGranularity`
//! and `vkGetRenderingAreaGranularity` are different functions with different
//! signatures rather than a spelling variation — the first takes a `VkRenderPass`, the
//! second a `VkRenderingAreaInfo *` for dynamic rendering.

use crate::forward::Allocator;
use crate::passthrough;
use crate::types::*;

passthrough! {
    // -----------------------------------------------------------------------
    // Pipelines
    //
    // Both creation entry points take the cache *before* the count, which is easy to
    // transpose: a swapped pair passes a count where a handle belongs and the driver
    // reads an array pointer out of an integer.
    // -----------------------------------------------------------------------

    fn vkCreateGraphicsPipelines(
        device: VkDevice,
        pipelineCache: VkHandle,
        createInfoCount: u32,
        pCreateInfos: VkStruct,
        pAllocator: Allocator,
        pPipelines: *mut VkHandle,
    ) -> VkResult;
    fn vkCreateComputePipelines(
        device: VkDevice,
        pipelineCache: VkHandle,
        createInfoCount: u32,
        pCreateInfos: VkStruct,
        pAllocator: Allocator,
        pPipelines: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroyPipeline(device: VkDevice, pipeline: VkHandle, pAllocator: Allocator);

    // -----------------------------------------------------------------------
    // Pipeline caches
    // -----------------------------------------------------------------------

    fn vkCreatePipelineCache(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pPipelineCache: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroyPipelineCache(device: VkDevice, pipelineCache: VkHandle, pAllocator: Allocator);
    // `pDataSize` is a `size_t *`, not the `uint32_t *` the enumeration idiom uses
    // elsewhere — a cache blob can exceed four gigabytes in principle, and the header
    // says `size_t`.
    fn vkGetPipelineCacheData(
        device: VkDevice,
        pipelineCache: VkHandle,
        pDataSize: *mut usize,
        pData: VkStructMut,
    ) -> VkResult;
    fn vkMergePipelineCaches(
        device: VkDevice,
        dstCache: VkHandle,
        srcCacheCount: u32,
        pSrcCaches: VkStruct,
    ) -> VkResult;

    // -----------------------------------------------------------------------
    // Pipeline layouts and shader modules
    // -----------------------------------------------------------------------

    fn vkCreatePipelineLayout(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pPipelineLayout: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroyPipelineLayout(
        device: VkDevice,
        pipelineLayout: VkHandle,
        pAllocator: Allocator,
    );
    fn vkCreateShaderModule(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pShaderModule: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroyShaderModule(device: VkDevice, shaderModule: VkHandle, pAllocator: Allocator);

    // -----------------------------------------------------------------------
    // Descriptor pools and sets
    //
    // The two allocation entry points take no allocator: the pool owns the memory.
    // -----------------------------------------------------------------------

    fn vkCreateDescriptorPool(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pDescriptorPool: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroyDescriptorPool(
        device: VkDevice,
        descriptorPool: VkHandle,
        pAllocator: Allocator,
    );
    fn vkResetDescriptorPool(
        device: VkDevice,
        descriptorPool: VkHandle,
        flags: VkFlags,
    ) -> VkResult;
    fn vkAllocateDescriptorSets(
        device: VkDevice,
        pAllocateInfo: VkStruct,
        pDescriptorSets: *mut VkHandle,
    ) -> VkResult;
    fn vkFreeDescriptorSets(
        device: VkDevice,
        descriptorPool: VkHandle,
        descriptorSetCount: u32,
        pDescriptorSets: VkStruct,
    ) -> VkResult;
    // Two count/array pairs — writes and copies — and returns nothing.
    fn vkUpdateDescriptorSets(
        device: VkDevice,
        descriptorWriteCount: u32,
        pDescriptorWrites: VkStruct,
        descriptorCopyCount: u32,
        pDescriptorCopies: VkStruct,
    );

    // -----------------------------------------------------------------------
    // Descriptor set layouts and update templates
    // -----------------------------------------------------------------------

    fn vkCreateDescriptorSetLayout(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pSetLayout: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroyDescriptorSetLayout(
        device: VkDevice,
        descriptorSetLayout: VkHandle,
        pAllocator: Allocator,
    );
    fn vkGetDescriptorSetLayoutSupport(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pSupport: VkStructMut,
    );
    fn vkCreateDescriptorUpdateTemplate(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pDescriptorUpdateTemplate: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroyDescriptorUpdateTemplate(
        device: VkDevice,
        descriptorUpdateTemplate: VkHandle,
        pAllocator: Allocator,
    );
    fn vkUpdateDescriptorSetWithTemplate(
        device: VkDevice,
        descriptorSet: VkHandle,
        descriptorUpdateTemplate: VkHandle,
        pData: VkStruct,
    );

    // -----------------------------------------------------------------------
    // Render passes and framebuffers
    // -----------------------------------------------------------------------

    fn vkCreateRenderPass(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pRenderPass: *mut VkHandle,
    ) -> VkResult;
    // Identical in shape to the base version; only the info structure differs, and
    // structures are opaque here.
    fn vkCreateRenderPass2(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pRenderPass: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroyRenderPass(device: VkDevice, renderPass: VkHandle, pAllocator: Allocator);
    /// The granularity of a render pass's render area.
    fn vkGetRenderAreaGranularity(
        device: VkDevice,
        renderPass: VkHandle,
        pGranularity: VkStructMut,
    );
    /// The dynamic-rendering counterpart, which describes the area with a structure
    /// rather than naming a render pass. Not a spelling variant of the above.
    fn vkGetRenderingAreaGranularity(
        device: VkDevice,
        pRenderingAreaInfo: VkStruct,
        pGranularity: VkStructMut,
    );
    fn vkCreateFramebuffer(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pFramebuffer: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroyFramebuffer(device: VkDevice, framebuffer: VkHandle, pAllocator: Allocator);

    // -----------------------------------------------------------------------
    // Samplers
    // -----------------------------------------------------------------------

    fn vkCreateSampler(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pSampler: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroySampler(device: VkDevice, sampler: VkHandle, pAllocator: Allocator);
    fn vkCreateSamplerYcbcrConversion(
        device: VkDevice,
        pCreateInfo: VkStruct,
        pAllocator: Allocator,
        pYcbcrConversion: *mut VkHandle,
    ) -> VkResult;
    fn vkDestroySamplerYcbcrConversion(
        device: VkDevice,
        ycbcrConversion: VkHandle,
        pAllocator: Allocator,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The declarations, with this test module's own text removed.
    ///
    /// The split matters: without it the names written out below would be found in the
    /// test source itself and every check would pass by reading its own expectations.
    fn declarations() -> &'static str {
        let source = include_str!("pipeline.rs");
        source
            .split_once("#[cfg(test)]")
            .map_or(source, |(before, _)| before)
    }

    /// The parameter list of one declared entry point.
    fn parameters(name: &str) -> String {
        let declarations = declarations();
        let start = declarations
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("{name} is not declared"));
        let open = declarations[start..].find('(').expect("a parameter list") + start;
        let end = declarations[open..]
            .find(';')
            .expect("a terminated declaration")
            + open;
        declarations[open..end].to_owned()
    }

    /// Whether a declaration passes its allocator through the translating wrapper.
    ///
    /// Checks the parameter's *type*, not the substring `Allocator` — the parameter is
    /// named `pAllocator`, so a substring search matches even when the type is wrong,
    /// which is exactly the mistake being looked for.
    ///
    /// The type is trimmed of the parameter list's own punctuation. A single-line
    /// declaration ends `pAllocator: Allocator)`, so comparing the untrimmed text
    /// against `"Allocator"` never matches and every entry point looks unwrapped —
    /// which is how this function failed when first written.
    fn takes_allocator(name: &str) -> bool {
        parameters(name)
            .split(',')
            .filter_map(|parameter| parameter.split_once(':'))
            .any(|(_, kind)| {
                kind.trim().trim_matches(|c| c == ')' || c == '(').trim() == "Allocator"
            })
    }

    /// The entry points the specification gives a `pAllocator`.
    ///
    /// Written out rather than derived, because deriving it from the source would make
    /// the test agree with whatever the source says — including a mistake.
    const WITH_ALLOCATOR: &[&str] = &[
        "vkCreateGraphicsPipelines",
        "vkCreateComputePipelines",
        "vkDestroyPipeline",
        "vkCreatePipelineCache",
        "vkDestroyPipelineCache",
        "vkCreatePipelineLayout",
        "vkDestroyPipelineLayout",
        "vkCreateShaderModule",
        "vkDestroyShaderModule",
        "vkCreateDescriptorPool",
        "vkDestroyDescriptorPool",
        "vkCreateDescriptorSetLayout",
        "vkDestroyDescriptorSetLayout",
        "vkCreateDescriptorUpdateTemplate",
        "vkDestroyDescriptorUpdateTemplate",
        "vkCreateRenderPass",
        "vkCreateRenderPass2",
        "vkDestroyRenderPass",
        "vkCreateFramebuffer",
        "vkDestroyFramebuffer",
        "vkCreateSampler",
        "vkDestroySampler",
        "vkCreateSamplerYcbcrConversion",
        "vkDestroySamplerYcbcrConversion",
    ];

    /// The ones that take none, listed explicitly so the two sets together cover the
    /// group and neither can be quietly incomplete.
    const WITHOUT_ALLOCATOR: &[&str] = &[
        // The pool owns descriptor-set memory, so neither of these allocates.
        "vkAllocateDescriptorSets",
        "vkFreeDescriptorSets",
        "vkResetDescriptorPool",
        "vkUpdateDescriptorSets",
        "vkUpdateDescriptorSetWithTemplate",
        "vkGetDescriptorSetLayoutSupport",
        "vkGetPipelineCacheData",
        "vkMergePipelineCaches",
        "vkGetRenderAreaGranularity",
        "vkGetRenderingAreaGranularity",
    ];

    /// A wrapped name must resolve to our thunk, never the host's address.
    #[test]
    fn lookup_returns_our_thunk() {
        if !crate::host::loader_present() {
            eprintln!("skipped: no host Vulkan loader on this machine");
            return;
        }
        let ours = lookup("vkCreateShaderModule").expect("declared and present");
        let host = crate::host::symbol("vkCreateShaderModule").expect("host exports it");
        assert_ne!(
            ours, host,
            "returned the host address rather than our thunk"
        );
    }

    /// Every allocator is declared as one, and nothing else is.
    ///
    /// This is the check that matters most in this group. A `pAllocator` typed as a
    /// bare pointer links, runs, and passes every other test — and then the driver
    /// calls the guest's allocation callbacks with the Windows convention against
    /// System V code, producing a corrupt pointer inside an allocation path.
    #[test]
    fn every_allocator_is_declared_as_an_allocator() {
        for name in WITH_ALLOCATOR {
            assert!(
                takes_allocator(name),
                "{name} takes a pAllocator and must declare it as `Allocator`"
            );
        }
        for name in WITHOUT_ALLOCATOR {
            assert!(
                !takes_allocator(name),
                "{name} takes no pAllocator; declaring one would translate an argument \
                 that is not there"
            );
        }
        // Together the two lists must account for the whole group, or a name could be
        // missing from both and checked by neither.
        assert_eq!(
            WITH_ALLOCATOR.len() + WITHOUT_ALLOCATOR.len(),
            NAMES.len(),
            "every declared entry point must appear in exactly one of the two lists"
        );
    }

    /// The pipeline-creation pair takes its cache before its count.
    ///
    /// Transposing them compiles — both are the same width once the handle is a `u64`
    /// and the count a `u32` promoted into a register — and then the driver reads a
    /// pointer out of a count.
    #[test]
    fn pipeline_creation_takes_the_cache_before_the_count() {
        for name in ["vkCreateGraphicsPipelines", "vkCreateComputePipelines"] {
            let parameters = parameters(name);
            let cache = parameters.find("pipelineCache").expect("a cache parameter");
            let count = parameters
                .find("createInfoCount")
                .expect("a count parameter");
            assert!(
                cache < count,
                "{name}: pipelineCache must precede createInfoCount"
            );
            assert_eq!(
                parameters.matches(':').count(),
                6,
                "{name} takes six arguments"
            );
        }
    }

    /// The two granularity queries are distinct functions, not one name spelled twice.
    #[test]
    fn the_two_granularity_queries_differ() {
        let by_render_pass = parameters("vkGetRenderAreaGranularity");
        assert!(by_render_pass.contains("renderPass"));
        let by_info = parameters("vkGetRenderingAreaGranularity");
        assert!(by_info.contains("pRenderingAreaInfo"));
        assert_ne!(by_render_pass, by_info);
    }

    /// The group declares its full share and nothing outside it.
    #[test]
    fn the_group_declares_its_full_share() {
        assert_eq!(NAMES.len(), 34, "the pipeline group owns 34 entry points");
    }
}
