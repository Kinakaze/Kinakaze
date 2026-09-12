//! Command-buffer recording. 89 entry points.
//!
//! The largest group, and the most uniform: almost every one takes a
//! `VkCommandBuffer` and returns nothing, because recording defers all of the work
//! and so has nothing to report. That uniformity is why the whole group can be a
//! single `passthrough!` block.
//!
//! Two details in here are worth knowing about before reading, because both are
//! mistakes that compile cleanly:
//!
//! **Floats must be `f32`.** Four entry points take floating-point arguments by
//! value. Both calling conventions pass floats in SSE registers, but they map
//! argument positions to registers differently, so declaring a float as `u32` sends
//! it in an integer register and the driver reads whatever happened to be in
//! `xmm0`. The affected entry points are `vkCmdSetLineWidth`, `vkCmdSetDepthBias`
//! and `vkCmdSetDepthBounds`. `vkCmdSetBlendConstants` looks like a fifth and is
//! not: its `const float[4]` decays to a pointer.
//!
//! **The `2` variants are not the base versions plus a suffix.** `vkCmdBlitImage2`
//! collapses eight arguments into one info structure; `vkCmdPipelineBarrier2`
//! collapses ten into two. They are separate entry points with separate shapes, and
//! assuming otherwise produces an argument count that is merely plausible.

use crate::passthrough;
use crate::types::*;

passthrough! {
    // -----------------------------------------------------------------------
    // Drawing
    // -----------------------------------------------------------------------

    fn vkCmdDraw(
        commandBuffer: VkCommandBuffer,
        vertexCount: u32,
        instanceCount: u32,
        firstVertex: u32,
        firstInstance: u32,
    );
    fn vkCmdDrawIndexed(
        commandBuffer: VkCommandBuffer,
        indexCount: u32,
        instanceCount: u32,
        firstIndex: u32,
        vertexOffset: i32,
        firstInstance: u32,
    );
    fn vkCmdDrawIndirect(
        commandBuffer: VkCommandBuffer,
        buffer: VkHandle,
        offset: VkDeviceSize,
        drawCount: u32,
        stride: u32,
    );
    fn vkCmdDrawIndexedIndirect(
        commandBuffer: VkCommandBuffer,
        buffer: VkHandle,
        offset: VkDeviceSize,
        drawCount: u32,
        stride: u32,
    );
    // The count comes from a second buffer, so these take two buffer/offset pairs
    // plus a maximum and a stride: seven arguments, not five.
    fn vkCmdDrawIndirectCount(
        commandBuffer: VkCommandBuffer,
        buffer: VkHandle,
        offset: VkDeviceSize,
        countBuffer: VkHandle,
        countBufferOffset: VkDeviceSize,
        maxDrawCount: u32,
        stride: u32,
    );
    fn vkCmdDrawIndexedIndirectCount(
        commandBuffer: VkCommandBuffer,
        buffer: VkHandle,
        offset: VkDeviceSize,
        countBuffer: VkHandle,
        countBufferOffset: VkDeviceSize,
        maxDrawCount: u32,
        stride: u32,
    );

    // -----------------------------------------------------------------------
    // Dispatch
    // -----------------------------------------------------------------------

    fn vkCmdDispatch(
        commandBuffer: VkCommandBuffer,
        groupCountX: u32,
        groupCountY: u32,
        groupCountZ: u32,
    );
    fn vkCmdDispatchIndirect(
        commandBuffer: VkCommandBuffer,
        buffer: VkHandle,
        offset: VkDeviceSize,
    );
    fn vkCmdDispatchBase(
        commandBuffer: VkCommandBuffer,
        baseGroupX: u32,
        baseGroupY: u32,
        baseGroupZ: u32,
        groupCountX: u32,
        groupCountY: u32,
        groupCountZ: u32,
    );

    // -----------------------------------------------------------------------
    // Binding
    // -----------------------------------------------------------------------

    fn vkCmdBindPipeline(
        commandBuffer: VkCommandBuffer,
        pipelineBindPoint: VkFlags,
        pipeline: VkHandle,
    );
    fn vkCmdBindIndexBuffer(
        commandBuffer: VkCommandBuffer,
        buffer: VkHandle,
        offset: VkDeviceSize,
        indexType: VkFlags,
    );
    // The 1.4 addition takes an explicit size, so the range need not run to the end
    // of the buffer.
    fn vkCmdBindIndexBuffer2(
        commandBuffer: VkCommandBuffer,
        buffer: VkHandle,
        offset: VkDeviceSize,
        size: VkDeviceSize,
        indexType: VkFlags,
    );
    fn vkCmdBindVertexBuffers(
        commandBuffer: VkCommandBuffer,
        firstBinding: u32,
        bindingCount: u32,
        pBuffers: VkStruct,
        pOffsets: VkStruct,
    );
    // Two more optional arrays than the base version: sizes and strides.
    fn vkCmdBindVertexBuffers2(
        commandBuffer: VkCommandBuffer,
        firstBinding: u32,
        bindingCount: u32,
        pBuffers: VkStruct,
        pOffsets: VkStruct,
        pSizes: VkStruct,
        pStrides: VkStruct,
    );
    // Eight arguments, with two count/array pairs: the sets, and the dynamic
    // offsets that apply to them.
    fn vkCmdBindDescriptorSets(
        commandBuffer: VkCommandBuffer,
        pipelineBindPoint: VkFlags,
        layout: VkHandle,
        firstSet: u32,
        descriptorSetCount: u32,
        pDescriptorSets: VkStruct,
        dynamicOffsetCount: u32,
        pDynamicOffsets: VkStruct,
    );
    fn vkCmdBindDescriptorSets2(commandBuffer: VkCommandBuffer, pBindDescriptorSetsInfo: VkStruct);

    // -----------------------------------------------------------------------
    // Copying and blitting
    //
    // The `2` variants each take a single info structure in place of the base
    // version's argument list, which is why the pairs look so different.
    // -----------------------------------------------------------------------

    fn vkCmdCopyBuffer(
        commandBuffer: VkCommandBuffer,
        srcBuffer: VkHandle,
        dstBuffer: VkHandle,
        regionCount: u32,
        pRegions: VkStruct,
    );
    fn vkCmdCopyBuffer2(commandBuffer: VkCommandBuffer, pCopyBufferInfo: VkStruct);
    fn vkCmdCopyImage(
        commandBuffer: VkCommandBuffer,
        srcImage: VkHandle,
        srcImageLayout: VkFlags,
        dstImage: VkHandle,
        dstImageLayout: VkFlags,
        regionCount: u32,
        pRegions: VkStruct,
    );
    fn vkCmdCopyImage2(commandBuffer: VkCommandBuffer, pCopyImageInfo: VkStruct);
    // Note the asymmetry with `vkCmdCopyImageToBuffer`: the image layout belongs to
    // whichever side is the image, so the argument order differs between the two.
    fn vkCmdCopyBufferToImage(
        commandBuffer: VkCommandBuffer,
        srcBuffer: VkHandle,
        dstImage: VkHandle,
        dstImageLayout: VkFlags,
        regionCount: u32,
        pRegions: VkStruct,
    );
    fn vkCmdCopyBufferToImage2(commandBuffer: VkCommandBuffer, pCopyBufferToImageInfo: VkStruct);
    fn vkCmdCopyImageToBuffer(
        commandBuffer: VkCommandBuffer,
        srcImage: VkHandle,
        srcImageLayout: VkFlags,
        dstBuffer: VkHandle,
        regionCount: u32,
        pRegions: VkStruct,
    );
    fn vkCmdCopyImageToBuffer2(commandBuffer: VkCommandBuffer, pCopyImageToBufferInfo: VkStruct);
    fn vkCmdBlitImage(
        commandBuffer: VkCommandBuffer,
        srcImage: VkHandle,
        srcImageLayout: VkFlags,
        dstImage: VkHandle,
        dstImageLayout: VkFlags,
        regionCount: u32,
        pRegions: VkStruct,
        filter: VkFlags,
    );
    fn vkCmdBlitImage2(commandBuffer: VkCommandBuffer, pBlitImageInfo: VkStruct);
    fn vkCmdResolveImage(
        commandBuffer: VkCommandBuffer,
        srcImage: VkHandle,
        srcImageLayout: VkFlags,
        dstImage: VkHandle,
        dstImageLayout: VkFlags,
        regionCount: u32,
        pRegions: VkStruct,
    );
    fn vkCmdResolveImage2(commandBuffer: VkCommandBuffer, pResolveImageInfo: VkStruct);

    // -----------------------------------------------------------------------
    // Clearing and filling
    // -----------------------------------------------------------------------

    // `pColor` is a `VkClearColorValue *` — a union of four floats, four ints or
    // four uints, passed by pointer. The floats inside it never touch a register
    // here, which is why this is not one of the float-carrying signatures.
    fn vkCmdClearColorImage(
        commandBuffer: VkCommandBuffer,
        image: VkHandle,
        imageLayout: VkFlags,
        pColor: VkStruct,
        rangeCount: u32,
        pRanges: VkStruct,
    );
    fn vkCmdClearDepthStencilImage(
        commandBuffer: VkCommandBuffer,
        image: VkHandle,
        imageLayout: VkFlags,
        pDepthStencil: VkStruct,
        rangeCount: u32,
        pRanges: VkStruct,
    );
    fn vkCmdClearAttachments(
        commandBuffer: VkCommandBuffer,
        attachmentCount: u32,
        pAttachments: VkStruct,
        rectCount: u32,
        pRects: VkStruct,
    );
    fn vkCmdFillBuffer(
        commandBuffer: VkCommandBuffer,
        dstBuffer: VkHandle,
        dstOffset: VkDeviceSize,
        size: VkDeviceSize,
        data: u32,
    );
    fn vkCmdUpdateBuffer(
        commandBuffer: VkCommandBuffer,
        dstBuffer: VkHandle,
        dstOffset: VkDeviceSize,
        dataSize: VkDeviceSize,
        pData: VkStruct,
    );

    // -----------------------------------------------------------------------
    // Barriers and events
    //
    // The base `vkCmdPipelineBarrier` has ten arguments and three count/array
    // pairs. The `2` form replaces all of it with one `VkDependencyInfo`, which is
    // also where the 64-bit `*Flags2` masks live — so they do not appear in these
    // signatures at all.
    // -----------------------------------------------------------------------

    fn vkCmdPipelineBarrier(
        commandBuffer: VkCommandBuffer,
        srcStageMask: VkFlags,
        dstStageMask: VkFlags,
        dependencyFlags: VkFlags,
        memoryBarrierCount: u32,
        pMemoryBarriers: VkStruct,
        bufferMemoryBarrierCount: u32,
        pBufferMemoryBarriers: VkStruct,
        imageMemoryBarrierCount: u32,
        pImageMemoryBarriers: VkStruct,
    );
    fn vkCmdPipelineBarrier2(commandBuffer: VkCommandBuffer, pDependencyInfo: VkStruct);
    fn vkCmdSetEvent(commandBuffer: VkCommandBuffer, event: VkHandle, stageMask: VkFlags);
    fn vkCmdSetEvent2(
        commandBuffer: VkCommandBuffer,
        event: VkHandle,
        pDependencyInfo: VkStruct,
    );
    fn vkCmdResetEvent(commandBuffer: VkCommandBuffer, event: VkHandle, stageMask: VkFlags);
    // `VkPipelineStageFlags2` is 64-bit, and here it is a by-value argument rather
    // than a field inside a structure — so this one really does need `VkFlags64`.
    fn vkCmdResetEvent2(commandBuffer: VkCommandBuffer, event: VkHandle, stageMask: VkFlags64);
    fn vkCmdWaitEvents(
        commandBuffer: VkCommandBuffer,
        eventCount: u32,
        pEvents: VkStruct,
        srcStageMask: VkFlags,
        dstStageMask: VkFlags,
        memoryBarrierCount: u32,
        pMemoryBarriers: VkStruct,
        bufferMemoryBarrierCount: u32,
        pBufferMemoryBarriers: VkStruct,
        imageMemoryBarrierCount: u32,
        pImageMemoryBarriers: VkStruct,
    );
    fn vkCmdWaitEvents2(
        commandBuffer: VkCommandBuffer,
        eventCount: u32,
        pEvents: VkStruct,
        pDependencyInfos: VkStruct,
    );

    // -----------------------------------------------------------------------
    // Queries
    // -----------------------------------------------------------------------

    fn vkCmdBeginQuery(
        commandBuffer: VkCommandBuffer,
        queryPool: VkHandle,
        query: u32,
        flags: VkFlags,
    );
    fn vkCmdEndQuery(commandBuffer: VkCommandBuffer, queryPool: VkHandle, query: u32);
    fn vkCmdResetQueryPool(
        commandBuffer: VkCommandBuffer,
        queryPool: VkHandle,
        firstQuery: u32,
        queryCount: u32,
    );
    fn vkCmdWriteTimestamp(
        commandBuffer: VkCommandBuffer,
        pipelineStage: VkFlags,
        queryPool: VkHandle,
        query: u32,
    );
    // As with `vkCmdResetEvent2`, the stage mask here is the 64-bit form.
    fn vkCmdWriteTimestamp2(
        commandBuffer: VkCommandBuffer,
        stage: VkFlags64,
        queryPool: VkHandle,
        query: u32,
    );
    fn vkCmdCopyQueryPoolResults(
        commandBuffer: VkCommandBuffer,
        queryPool: VkHandle,
        firstQuery: u32,
        queryCount: u32,
        dstBuffer: VkHandle,
        dstOffset: VkDeviceSize,
        stride: VkDeviceSize,
        flags: VkFlags,
    );

    // -----------------------------------------------------------------------
    // Render passes and dynamic rendering
    // -----------------------------------------------------------------------

    fn vkCmdBeginRenderPass(
        commandBuffer: VkCommandBuffer,
        pRenderPassBegin: VkStruct,
        contents: VkFlags,
    );
    fn vkCmdBeginRenderPass2(
        commandBuffer: VkCommandBuffer,
        pRenderPassBegin: VkStruct,
        pSubpassBeginInfo: VkStruct,
    );
    fn vkCmdNextSubpass(commandBuffer: VkCommandBuffer, contents: VkFlags);
    fn vkCmdNextSubpass2(
        commandBuffer: VkCommandBuffer,
        pSubpassBeginInfo: VkStruct,
        pSubpassEndInfo: VkStruct,
    );
    fn vkCmdEndRenderPass(commandBuffer: VkCommandBuffer);
    fn vkCmdEndRenderPass2(commandBuffer: VkCommandBuffer, pSubpassEndInfo: VkStruct);
    fn vkCmdBeginRendering(commandBuffer: VkCommandBuffer, pRenderingInfo: VkStruct);
    fn vkCmdEndRendering(commandBuffer: VkCommandBuffer);
    fn vkCmdSetRenderingAttachmentLocations(
        commandBuffer: VkCommandBuffer,
        pLocationInfo: VkStruct,
    );
    fn vkCmdSetRenderingInputAttachmentIndices(
        commandBuffer: VkCommandBuffer,
        pInputAttachmentIndexInfo: VkStruct,
    );
    fn vkCmdExecuteCommands(
        commandBuffer: VkCommandBuffer,
        commandBufferCount: u32,
        pCommandBuffers: VkStruct,
    );

    // -----------------------------------------------------------------------
    // Push constants and push descriptors
    // -----------------------------------------------------------------------

    fn vkCmdPushConstants(
        commandBuffer: VkCommandBuffer,
        layout: VkHandle,
        stageFlags: VkFlags,
        offset: u32,
        size: u32,
        pValues: VkStruct,
    );
    fn vkCmdPushConstants2(commandBuffer: VkCommandBuffer, pPushConstantsInfo: VkStruct);
    fn vkCmdPushDescriptorSet(
        commandBuffer: VkCommandBuffer,
        pipelineBindPoint: VkFlags,
        layout: VkHandle,
        set: u32,
        descriptorWriteCount: u32,
        pDescriptorWrites: VkStruct,
    );
    fn vkCmdPushDescriptorSet2(commandBuffer: VkCommandBuffer, pPushDescriptorSetInfo: VkStruct);
    fn vkCmdPushDescriptorSetWithTemplate(
        commandBuffer: VkCommandBuffer,
        descriptorUpdateTemplate: VkHandle,
        layout: VkHandle,
        set: u32,
        pData: VkStruct,
    );
    fn vkCmdPushDescriptorSetWithTemplate2(
        commandBuffer: VkCommandBuffer,
        pPushDescriptorSetWithTemplateInfo: VkStruct,
    );

    // -----------------------------------------------------------------------
    // Dynamic state
    //
    // The four float-carrying entry points are here. See the module docs.
    // -----------------------------------------------------------------------

    fn vkCmdSetViewport(
        commandBuffer: VkCommandBuffer,
        firstViewport: u32,
        viewportCount: u32,
        pViewports: VkStruct,
    );
    fn vkCmdSetViewportWithCount(
        commandBuffer: VkCommandBuffer,
        viewportCount: u32,
        pViewports: VkStruct,
    );
    fn vkCmdSetScissor(
        commandBuffer: VkCommandBuffer,
        firstScissor: u32,
        scissorCount: u32,
        pScissors: VkStruct,
    );
    fn vkCmdSetScissorWithCount(
        commandBuffer: VkCommandBuffer,
        scissorCount: u32,
        pScissors: VkStruct,
    );
    /// One `float`, by value.
    fn vkCmdSetLineWidth(commandBuffer: VkCommandBuffer, lineWidth: f32);
    /// Three `float`s, by value.
    fn vkCmdSetDepthBias(
        commandBuffer: VkCommandBuffer,
        depthBiasConstantFactor: f32,
        depthBiasClamp: f32,
        depthBiasSlopeFactor: f32,
    );
    /// Two `float`s, by value.
    fn vkCmdSetDepthBounds(
        commandBuffer: VkCommandBuffer,
        minDepthBounds: f32,
        maxDepthBounds: f32,
    );
    /// A `const float[4]`, which decays to a pointer — so no float register is
    /// involved, despite the name.
    fn vkCmdSetBlendConstants(commandBuffer: VkCommandBuffer, blendConstants: VkStruct);
    fn vkCmdSetStencilCompareMask(
        commandBuffer: VkCommandBuffer,
        faceMask: VkFlags,
        compareMask: u32,
    );
    fn vkCmdSetStencilWriteMask(
        commandBuffer: VkCommandBuffer,
        faceMask: VkFlags,
        writeMask: u32,
    );
    fn vkCmdSetStencilReference(
        commandBuffer: VkCommandBuffer,
        faceMask: VkFlags,
        reference: u32,
    );
    fn vkCmdSetStencilOp(
        commandBuffer: VkCommandBuffer,
        faceMask: VkFlags,
        failOp: VkFlags,
        passOp: VkFlags,
        depthFailOp: VkFlags,
        compareOp: VkFlags,
    );
    fn vkCmdSetStencilTestEnable(commandBuffer: VkCommandBuffer, stencilTestEnable: VkBool32);
    fn vkCmdSetCullMode(commandBuffer: VkCommandBuffer, cullMode: VkFlags);
    fn vkCmdSetFrontFace(commandBuffer: VkCommandBuffer, frontFace: VkFlags);
    fn vkCmdSetPrimitiveTopology(commandBuffer: VkCommandBuffer, primitiveTopology: VkFlags);
    fn vkCmdSetPrimitiveRestartEnable(
        commandBuffer: VkCommandBuffer,
        primitiveRestartEnable: VkBool32,
    );
    fn vkCmdSetDepthTestEnable(commandBuffer: VkCommandBuffer, depthTestEnable: VkBool32);
    fn vkCmdSetDepthWriteEnable(commandBuffer: VkCommandBuffer, depthWriteEnable: VkBool32);
    fn vkCmdSetDepthCompareOp(commandBuffer: VkCommandBuffer, depthCompareOp: VkFlags);
    fn vkCmdSetDepthBoundsTestEnable(
        commandBuffer: VkCommandBuffer,
        depthBoundsTestEnable: VkBool32,
    );
    fn vkCmdSetDepthBiasEnable(commandBuffer: VkCommandBuffer, depthBiasEnable: VkBool32);
    fn vkCmdSetRasterizerDiscardEnable(
        commandBuffer: VkCommandBuffer,
        rasterizerDiscardEnable: VkBool32,
    );
    fn vkCmdSetLineStipple(
        commandBuffer: VkCommandBuffer,
        lineStippleFactor: u32,
        lineStipplePattern: u16,
    );
    fn vkCmdSetDeviceMask(commandBuffer: VkCommandBuffer, deviceMask: u32);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The declarations, with this test module's own text removed.
    ///
    /// Splitting at the `#[cfg(test)]` attribute matters: without it the expected
    /// names written out below would be found in the test source and every check
    /// would pass by reading itself.
    fn declarations() -> &'static str {
        let source = include_str!("command.rs");
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

    /// A wrapped name must resolve to our thunk, never the host's address — the
    /// guest would call a host address with the wrong convention and crash inside a
    /// driver with nothing in the backtrace to explain it.
    #[test]
    fn lookup_returns_our_thunk() {
        if !crate::host::loader_present() {
            eprintln!("skipped: no host Vulkan loader on this machine");
            return;
        }
        let ours = lookup("vkCmdClearColorImage").expect("declared and present");
        let host = crate::host::symbol("vkCmdClearColorImage").expect("host exports it");
        assert_ne!(
            ours, host,
            "returned the host address rather than our thunk"
        );
    }

    /// The float signatures, checked by reading the source.
    ///
    /// This is the one mistake in the group that nothing else would catch. A float
    /// declared as `u32` compiles, links, resolves, and passes every test that only
    /// checks addresses — and then arrives in an integer register, so the driver
    /// reads an unrelated value out of `xmm0`. There is no runtime check available
    /// without a device and a frame, so the declaration itself is what gets tested.
    #[test]
    fn the_float_signatures_carry_floats() {
        let cases = [
            ("vkCmdSetLineWidth", 1),
            ("vkCmdSetDepthBias", 3),
            ("vkCmdSetDepthBounds", 2),
            // Its four floats arrive as a pointer, so none is passed by value.
            ("vkCmdSetBlendConstants", 0),
        ];
        for (name, expected) in cases {
            let found = parameters(name).matches("f32").count();
            assert_eq!(
                found, expected,
                "{name} should declare {expected} f32 parameters"
            );
        }
    }

    /// The two entry points that take a 64-bit stage mask by value. Declaring
    /// either as `VkFlags` would truncate it, dropping every stage bit above 32.
    #[test]
    fn the_sixty_four_bit_masks_are_wide() {
        for name in ["vkCmdResetEvent2", "vkCmdWriteTimestamp2"] {
            assert!(
                parameters(name).contains("VkFlags64"),
                "{name} should declare a VkFlags64 stage mask"
            );
        }
    }

    /// The barrier the guest demo depends on, pinned by argument count.
    ///
    /// Ten parameters with three count/array pairs, which is easy to get wrong by
    /// dropping a pair — and a dropped pair reads the following argument as a count.
    #[test]
    fn the_pipeline_barrier_has_its_full_argument_list() {
        let base = parameters("vkCmdPipelineBarrier");
        assert_eq!(base.matches(':').count(), 10);
        for expected in [
            "memoryBarrierCount",
            "bufferMemoryBarrierCount",
            "imageMemoryBarrierCount",
        ] {
            assert!(base.contains(expected), "missing {expected}");
        }

        // The `2` form is a different shape, not the same list with a suffix.
        let second = parameters("vkCmdPipelineBarrier2");
        assert_eq!(second.matches(':').count(), 2);
        assert!(second.contains("pDependencyInfo"));
    }

    /// Every name the partition assigns to this group is declared, and no others.
    ///
    /// `NAMES` comes from the macro, so this compares the group against its own
    /// expected size rather than against the host loader — which
    /// `groups::tests::every_host_export_is_wrapped` already does.
    #[test]
    fn the_group_declares_its_full_share() {
        assert_eq!(NAMES.len(), 89, "the command group owns 89 entry points");
        for name in NAMES {
            assert!(
                name.starts_with("vkCmd"),
                "{name} is not a recording command"
            );
        }
    }
}
