//! Generated from Khronos vk.xml by scripts/generate-vulkan-extensions.py.
//! Do not edit by hand.

use crate::dynamic_passthrough;
use crate::forward::Allocator;
use crate::types::{VkDevice, VkInstance};
use core::ffi::c_void;

dynamic_passthrough! {
    fn vkAcquireDrmDisplayEXT(arg0: *mut c_void, arg1: i32, arg2: u64) -> i32;
    fn vkAcquireFullScreenExclusiveModeEXT(arg0: *mut c_void, arg1: u64) -> i32;
    fn vkAcquireImageANDROID(arg0: *mut c_void, arg1: u64, arg2: i32, arg3: u64, arg4: u64) -> i32;
    fn vkAcquireImageOHOS(arg0: *mut c_void, arg1: u64, arg2: i32, arg3: u64, arg4: u64) -> i32;
    fn vkAcquirePerformanceConfigurationINTEL(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkAcquireProfilingLockKHR(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkAcquireWinrtDisplayNV(arg0: *mut c_void, arg1: u64) -> i32;
    fn vkAcquireXlibDisplayEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: u64) -> i32;
    fn vkAntiLagUpdateAMD(arg0: *mut c_void, arg1: *mut c_void);
    fn vkBindAccelerationStructureMemoryNV(arg0: *mut c_void, arg1: u32, arg2: *mut c_void) -> i32;
    fn vkBindBufferMemory2KHR(arg0: *mut c_void, arg1: u32, arg2: *mut c_void) -> i32;
    fn vkBindDataGraphPipelineSessionMemoryARM(arg0: *mut c_void, arg1: u32, arg2: *mut c_void) -> i32;
    fn vkBindImageMemory2KHR(arg0: *mut c_void, arg1: u32, arg2: *mut c_void) -> i32;
    fn vkBindOpticalFlowSessionImageNV(arg0: *mut c_void, arg1: u64, arg2: i32, arg3: u64, arg4: i32) -> i32;
    fn vkBindTensorMemoryARM(arg0: *mut c_void, arg1: u32, arg2: *mut c_void) -> i32;
    fn vkBindVideoSessionMemoryKHR(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: *mut c_void) -> i32;
    fn vkBuildAccelerationStructuresKHR(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: *mut c_void, arg4: *mut c_void) -> i32;
    fn vkBuildMicromapsEXT(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: *mut c_void) -> i32;
    fn vkClearShaderInstrumentationMetricsARM(arg0: *mut c_void, arg1: u64);
    fn vkCmdBeginConditionalRendering2EXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdBeginConditionalRenderingEXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdBeginCustomResolveEXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdBeginDebugUtilsLabelEXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdBeginGpaSampleAMD(arg0: *mut c_void, arg1: u64, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkCmdBeginGpaSessionAMD(arg0: *mut c_void, arg1: u64) -> i32;
    fn vkCmdBeginPerTileExecutionQCOM(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdBeginQueryIndexedEXT(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: u32, arg4: u32);
    fn vkCmdBeginRenderPass2KHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkCmdBeginRenderingKHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdBeginShaderInstrumentationARM(arg0: *mut c_void, arg1: u64);
    fn vkCmdBeginTransformFeedback2EXT(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void);
    fn vkCmdBeginTransformFeedbackEXT(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void, arg4: *mut c_void);
    fn vkCmdBeginVideoCodingKHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdBindDescriptorBufferEmbeddedSamplers2EXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdBindDescriptorBufferEmbeddedSamplersEXT(arg0: *mut c_void, arg1: i32, arg2: u64, arg3: u32);
    fn vkCmdBindDescriptorBuffersEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void);
    fn vkCmdBindDescriptorSets2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdBindIndexBuffer2KHR(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u64, arg4: i32);
    fn vkCmdBindIndexBuffer3KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdBindInvocationMaskHUAWEI(arg0: *mut c_void, arg1: u64, arg2: i32);
    fn vkCmdBindPipelineShaderGroupNV(arg0: *mut c_void, arg1: i32, arg2: u64, arg3: u32);
    fn vkCmdBindResourceHeapEXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdBindSamplerHeapEXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdBindShadersEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: *mut c_void);
    fn vkCmdBindShadingRateImageNV(arg0: *mut c_void, arg1: u64, arg2: i32);
    fn vkCmdBindTileMemoryQCOM(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdBindTransformFeedbackBuffers2EXT(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void);
    fn vkCmdBindTransformFeedbackBuffersEXT(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void, arg4: *mut c_void, arg5: *mut c_void);
    fn vkCmdBindVertexBuffers2EXT(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void, arg4: *mut c_void, arg5: *mut c_void, arg6: *mut c_void);
    fn vkCmdBindVertexBuffers3KHR(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void);
    fn vkCmdBlitImage2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdBuildAccelerationStructureNV(arg0: *mut c_void, arg1: *mut c_void, arg2: u64, arg3: u64, arg4: u32, arg5: u64, arg6: u64, arg7: u64, arg8: u64);
    fn vkCmdBuildAccelerationStructuresIndirectKHR(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: *mut c_void, arg4: *mut c_void, arg5: *mut c_void);
    fn vkCmdBuildAccelerationStructuresKHR(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: *mut c_void);
    fn vkCmdBuildClusterAccelerationStructureIndirectNV(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdBuildMicromapsEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void);
    fn vkCmdBuildPartitionedAccelerationStructuresNV(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdControlVideoCodingKHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdConvertCooperativeVectorMatrixNV(arg0: *mut c_void, arg1: u32, arg2: *mut c_void);
    fn vkCmdCopyAccelerationStructureKHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdCopyAccelerationStructureNV(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: i32);
    fn vkCmdCopyAccelerationStructureToMemoryKHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdCopyBuffer2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdCopyBufferToImage2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdCopyGpaSessionResultsAMD(arg0: *mut c_void, arg1: u64);
    fn vkCmdCopyImage2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdCopyImageToBuffer2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdCopyImageToMemoryKHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdCopyMemoryIndirectKHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdCopyMemoryIndirectNV(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: u32);
    fn vkCmdCopyMemoryKHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdCopyMemoryToAccelerationStructureKHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdCopyMemoryToImageIndirectKHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdCopyMemoryToImageIndirectNV(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: u32, arg4: u64, arg5: i32, arg6: *mut c_void);
    fn vkCmdCopyMemoryToImageKHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdCopyMemoryToMicromapEXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdCopyMicromapEXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdCopyMicromapToMemoryEXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdCopyQueryPoolResultsToMemoryKHR(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: u32, arg4: *mut c_void, arg5: u32, arg6: u32);
    fn vkCmdCopyTensorARM(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdCuLaunchKernelNVX(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdCudaLaunchKernelNV(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdDebugMarkerBeginEXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdDebugMarkerEndEXT(arg0: *mut c_void);
    fn vkCmdDebugMarkerInsertEXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdDecodeVideoKHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdDecompressMemoryEXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdDecompressMemoryIndirectCountEXT(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u64, arg4: u32, arg5: u32);
    fn vkCmdDecompressMemoryIndirectCountNV(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u32);
    fn vkCmdDecompressMemoryNV(arg0: *mut c_void, arg1: u32, arg2: *mut c_void);
    fn vkCmdDispatchBaseKHR(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: u32, arg4: u32, arg5: u32, arg6: u32);
    fn vkCmdDispatchDataGraphARM(arg0: *mut c_void, arg1: u64, arg2: *mut c_void);
    fn vkCmdDispatchGraphAMDX(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: *mut c_void);
    fn vkCmdDispatchGraphIndirectAMDX(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: *mut c_void);
    fn vkCmdDispatchGraphIndirectCountAMDX(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u64);
    fn vkCmdDispatchIndirect2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdDispatchTileQCOM(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdDrawClusterHUAWEI(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: u32);
    fn vkCmdDrawClusterIndirectHUAWEI(arg0: *mut c_void, arg1: u64, arg2: u64);
    fn vkCmdDrawIndexedIndirect2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdDrawIndexedIndirectCount2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdDrawIndexedIndirectCountAMD(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u32, arg6: u32);
    fn vkCmdDrawIndexedIndirectCountKHR(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u32, arg6: u32);
    fn vkCmdDrawIndirect2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdDrawIndirectByteCount2EXT(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void, arg4: u32, arg5: u32);
    fn vkCmdDrawIndirectByteCountEXT(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: u64, arg4: u64, arg5: u32, arg6: u32);
    fn vkCmdDrawIndirectCount2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdDrawIndirectCountAMD(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u32, arg6: u32);
    fn vkCmdDrawIndirectCountKHR(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u32, arg6: u32);
    fn vkCmdDrawMeshTasksEXT(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: u32);
    fn vkCmdDrawMeshTasksIndirect2EXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdDrawMeshTasksIndirectCount2EXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdDrawMeshTasksIndirectCountEXT(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u32, arg6: u32);
    fn vkCmdDrawMeshTasksIndirectCountNV(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u32, arg6: u32);
    fn vkCmdDrawMeshTasksIndirectEXT(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u32, arg4: u32);
    fn vkCmdDrawMeshTasksIndirectNV(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u32, arg4: u32);
    fn vkCmdDrawMeshTasksNV(arg0: *mut c_void, arg1: u32, arg2: u32);
    fn vkCmdDrawMultiEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: u32, arg4: u32, arg5: u32);
    fn vkCmdDrawMultiIndexedEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: u32, arg4: u32, arg5: u32, arg6: *mut c_void);
    fn vkCmdEncodeVideoKHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdEndConditionalRenderingEXT(arg0: *mut c_void);
    fn vkCmdEndDebugUtilsLabelEXT(arg0: *mut c_void);
    fn vkCmdEndGpaSampleAMD(arg0: *mut c_void, arg1: u64, arg2: u32);
    fn vkCmdEndGpaSessionAMD(arg0: *mut c_void, arg1: u64) -> i32;
    fn vkCmdEndPerTileExecutionQCOM(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdEndQueryIndexedEXT(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: u32);
    fn vkCmdEndRenderPass2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdEndRendering2EXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdEndRendering2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdEndRenderingKHR(arg0: *mut c_void);
    fn vkCmdEndShaderInstrumentationARM(arg0: *mut c_void);
    fn vkCmdEndTransformFeedback2EXT(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void);
    fn vkCmdEndTransformFeedbackEXT(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void, arg4: *mut c_void);
    fn vkCmdEndVideoCodingKHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdExecuteGeneratedCommandsEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void);
    fn vkCmdExecuteGeneratedCommandsNV(arg0: *mut c_void, arg1: u32, arg2: *mut c_void);
    fn vkCmdFillMemoryKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: u32, arg3: u32);
    fn vkCmdInitializeGraphScratchMemoryAMDX(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u64);
    fn vkCmdInsertDebugUtilsLabelEXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdNextSubpass2KHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkCmdOpticalFlowExecuteNV(arg0: *mut c_void, arg1: u64, arg2: *mut c_void);
    fn vkCmdPipelineBarrier2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdPreprocessGeneratedCommandsEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkCmdPreprocessGeneratedCommandsNV(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdPushConstants2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdPushDataEXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdPushDescriptorSet2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdPushDescriptorSetKHR(arg0: *mut c_void, arg1: i32, arg2: u64, arg3: u32, arg4: u32, arg5: *mut c_void);
    fn vkCmdPushDescriptorSetWithTemplate2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdPushDescriptorSetWithTemplateKHR(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u32, arg4: *mut c_void);
    fn vkCmdRefreshObjectsKHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdResetEvent2KHR(arg0: *mut c_void, arg1: u64, arg2: u64);
    fn vkCmdResolveImage2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdSetAlphaToCoverageEnableEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetAlphaToOneEnableEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetAttachmentFeedbackLoopEnableEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetCheckpointNV(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdSetCoarseSampleOrderNV(arg0: *mut c_void, arg1: i32, arg2: u32, arg3: *mut c_void);
    fn vkCmdSetColorBlendAdvancedEXT(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void);
    fn vkCmdSetColorBlendEnableEXT(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void);
    fn vkCmdSetColorBlendEquationEXT(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void);
    fn vkCmdSetColorWriteEnableEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void);
    fn vkCmdSetColorWriteMaskEXT(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void);
    fn vkCmdSetComputeOccupancyPriorityNV(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdSetConservativeRasterizationModeEXT(arg0: *mut c_void, arg1: i32);
    fn vkCmdSetCoverageModulationModeNV(arg0: *mut c_void, arg1: i32);
    fn vkCmdSetCoverageModulationTableEnableNV(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetCoverageModulationTableNV(arg0: *mut c_void, arg1: u32, arg2: *mut c_void);
    fn vkCmdSetCoverageReductionModeNV(arg0: *mut c_void, arg1: i32);
    fn vkCmdSetCoverageToColorEnableNV(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetCoverageToColorLocationNV(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetCullModeEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetDepthBias2EXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdSetDepthBiasEnableEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetDepthBoundsTestEnableEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetDepthClampEnableEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetDepthClampRangeEXT(arg0: *mut c_void, arg1: i32, arg2: *mut c_void);
    fn vkCmdSetDepthClipEnableEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetDepthClipNegativeOneToOneEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetDepthCompareOpEXT(arg0: *mut c_void, arg1: i32);
    fn vkCmdSetDepthTestEnableEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetDepthWriteEnableEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetDescriptorBufferOffsets2EXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdSetDescriptorBufferOffsetsEXT(arg0: *mut c_void, arg1: i32, arg2: u64, arg3: u32, arg4: u32, arg5: *mut c_void, arg6: *mut c_void);
    fn vkCmdSetDeviceMaskKHR(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetDiscardRectangleEXT(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void);
    fn vkCmdSetDiscardRectangleEnableEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetDiscardRectangleModeEXT(arg0: *mut c_void, arg1: i32);
    fn vkCmdSetDispatchParametersARM(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdSetEvent2KHR(arg0: *mut c_void, arg1: u64, arg2: *mut c_void);
    fn vkCmdSetExclusiveScissorEnableNV(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void);
    fn vkCmdSetExclusiveScissorNV(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void);
    fn vkCmdSetExtraPrimitiveOverestimationSizeEXT(arg0: *mut c_void, arg1: f32);
    fn vkCmdSetFragmentShadingRateEnumNV(arg0: *mut c_void, arg1: i32, arg2: *mut c_void);
    fn vkCmdSetFragmentShadingRateKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkCmdSetFrontFaceEXT(arg0: *mut c_void, arg1: i32);
    fn vkCmdSetLineRasterizationModeEXT(arg0: *mut c_void, arg1: i32);
    fn vkCmdSetLineStippleEXT(arg0: *mut c_void, arg1: u32, arg2: u16);
    fn vkCmdSetLineStippleEnableEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetLineStippleKHR(arg0: *mut c_void, arg1: u32, arg2: u16);
    fn vkCmdSetLogicOpEXT(arg0: *mut c_void, arg1: i32);
    fn vkCmdSetLogicOpEnableEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetPatchControlPointsEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetPerformanceMarkerINTEL(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkCmdSetPerformanceOverrideINTEL(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkCmdSetPerformanceStreamMarkerINTEL(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkCmdSetPolygonModeEXT(arg0: *mut c_void, arg1: i32);
    fn vkCmdSetPrimitiveRestartEnableEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetPrimitiveRestartIndexEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetPrimitiveTopologyEXT(arg0: *mut c_void, arg1: i32);
    fn vkCmdSetProvokingVertexModeEXT(arg0: *mut c_void, arg1: i32);
    fn vkCmdSetRasterizationSamplesEXT(arg0: *mut c_void, arg1: i32);
    fn vkCmdSetRasterizationStreamEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetRasterizerDiscardEnableEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetRayTracingPipelineStackSizeKHR(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetRenderingAttachmentLocationsKHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdSetRenderingInputAttachmentIndicesKHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdSetRepresentativeFragmentTestEnableNV(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetSampleLocationsEXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdSetSampleLocationsEnableEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetSampleMaskEXT(arg0: *mut c_void, arg1: i32, arg2: *mut c_void);
    fn vkCmdSetScissorWithCountEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void);
    fn vkCmdSetShadingRateImageEnableNV(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetStencilOpEXT(arg0: *mut c_void, arg1: u32, arg2: i32, arg3: i32, arg4: i32, arg5: i32);
    fn vkCmdSetStencilTestEnableEXT(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetTessellationDomainOriginEXT(arg0: *mut c_void, arg1: i32);
    fn vkCmdSetVertexInputEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: u32, arg4: *mut c_void);
    fn vkCmdSetViewportShadingRatePaletteNV(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void);
    fn vkCmdSetViewportSwizzleNV(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void);
    fn vkCmdSetViewportWScalingEnableNV(arg0: *mut c_void, arg1: u32);
    fn vkCmdSetViewportWScalingNV(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: *mut c_void);
    fn vkCmdSetViewportWithCountEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void);
    fn vkCmdSubpassShadingHUAWEI(arg0: *mut c_void);
    fn vkCmdTraceRaysIndirect2KHR(arg0: *mut c_void, arg1: u64);
    fn vkCmdTraceRaysIndirectKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void, arg3: *mut c_void, arg4: *mut c_void, arg5: u64);
    fn vkCmdTraceRaysKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void, arg3: *mut c_void, arg4: *mut c_void, arg5: u32, arg6: u32, arg7: u32);
    fn vkCmdTraceRaysNV(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u64, arg6: u64, arg7: u64, arg8: u64, arg9: u64, arg10: u64, arg11: u64, arg12: u32, arg13: u32, arg14: u32);
    fn vkCmdUpdateMemoryKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: u32, arg3: u64, arg4: *mut c_void);
    fn vkCmdUpdatePipelineIndirectBufferNV(arg0: *mut c_void, arg1: i32, arg2: u64);
    fn vkCmdWaitEvents2KHR(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: *mut c_void);
    fn vkCmdWriteAccelerationStructuresPropertiesKHR(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: i32, arg4: u64, arg5: u32);
    fn vkCmdWriteAccelerationStructuresPropertiesNV(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: i32, arg4: u64, arg5: u32);
    fn vkCmdWriteBufferMarker2AMD(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u64, arg4: u32);
    fn vkCmdWriteBufferMarkerAMD(arg0: *mut c_void, arg1: i32, arg2: u64, arg3: u64, arg4: u32);
    fn vkCmdWriteMarkerToMemoryAMD(arg0: *mut c_void, arg1: *mut c_void);
    fn vkCmdWriteMicromapsPropertiesEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: i32, arg4: u64, arg5: u32);
    fn vkCmdWriteTimestamp2KHR(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u32);
    fn vkCompileDeferredNV(arg0: *mut c_void, arg1: u64, arg2: u32) -> i32;
    fn vkConvertCooperativeVectorMatrixNV(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkCopyAccelerationStructureKHR(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkCopyAccelerationStructureToMemoryKHR(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkCopyImageToImageEXT(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkCopyImageToMemoryEXT(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkCopyMemoryToAccelerationStructureKHR(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkCopyMemoryToImageEXT(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkCopyMemoryToMicromapEXT(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkCopyMicromapEXT(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkCopyMicromapToMemoryEXT(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkCreateAccelerationStructure2KHR(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateAccelerationStructureKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateAccelerationStructureNV(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateAndroidSurfaceKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateBufferCollectionFUCHSIA(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateCuFunctionNVX(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateCuModuleNVX(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateCudaFunctionNV(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateCudaModuleNV(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateDataGraphPipelineSessionARM(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateDataGraphPipelinesARM(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u32, arg4: *mut c_void, arg5: Allocator, arg6: *mut c_void) -> i32;
    fn vkCreateDeferredOperationKHR(arg0: *mut c_void, arg1: Allocator, arg2: *mut c_void) -> i32;
    fn vkCreateDescriptorUpdateTemplateKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateDirectFBSurfaceEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateExecutionGraphPipelinesAMDX(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: *mut c_void, arg4: Allocator, arg5: *mut c_void) -> i32;
    fn vkCreateExternalComputeQueueNV(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateGpaSessionAMD(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateIOSSurfaceMVK(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateImagePipeSurfaceFUCHSIA(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateIndirectCommandsLayoutEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateIndirectCommandsLayoutNV(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateIndirectExecutionSetEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateMacOSSurfaceMVK(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateMetalSurfaceEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateMicromapEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateOpticalFlowSessionNV(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreatePipelineBinariesKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreatePrivateDataSlotEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateRayTracingPipelinesKHR(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u32, arg4: *mut c_void, arg5: Allocator, arg6: *mut c_void) -> i32;
    fn vkCreateRayTracingPipelinesNV(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: *mut c_void, arg4: Allocator, arg5: *mut c_void) -> i32;
    fn vkCreateRenderPass2KHR(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateSamplerYcbcrConversionKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateScreenSurfaceQNX(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateSemaphoreSciSyncPoolNV(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateShaderInstrumentationARM(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateShadersEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: Allocator, arg4: *mut c_void) -> i32;
    fn vkCreateStreamDescriptorSurfaceGGP(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateSurfaceOHOS(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateTensorARM(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateTensorViewARM(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateUbmSurfaceSEC(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateValidationCacheEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateViSurfaceNN(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateVideoSessionKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkCreateVideoSessionParametersKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkDebugMarkerSetObjectNameEXT(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkDebugMarkerSetObjectTagEXT(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkDebugReportMessageEXT(arg0: *mut c_void, arg1: u32, arg2: i32, arg3: u64, arg4: usize, arg5: i32, arg6: *mut c_void, arg7: *mut c_void);
    fn vkDeferredOperationJoinKHR(arg0: *mut c_void, arg1: u64) -> i32;
    fn vkDestroyAccelerationStructureKHR(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyAccelerationStructureNV(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyBufferCollectionFUCHSIA(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyCuFunctionNVX(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyCuModuleNVX(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyCudaFunctionNV(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyCudaModuleNV(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyDataGraphPipelineSessionARM(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyDebugReportCallbackEXT(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyDebugUtilsMessengerEXT(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyDeferredOperationKHR(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyDescriptorUpdateTemplateKHR(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyExternalComputeQueueNV(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator);
    fn vkDestroyGpaSessionAMD(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyIndirectCommandsLayoutEXT(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyIndirectCommandsLayoutNV(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyIndirectExecutionSetEXT(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyMicromapEXT(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyOpticalFlowSessionNV(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyPipelineBinaryKHR(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyPrivateDataSlotEXT(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroySamplerYcbcrConversionKHR(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroySemaphoreSciSyncPoolNV(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyShaderEXT(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyShaderInstrumentationARM(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyTensorARM(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyTensorViewARM(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyValidationCacheEXT(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyVideoSessionKHR(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDestroyVideoSessionParametersKHR(arg0: *mut c_void, arg1: u64, arg2: Allocator);
    fn vkDisplayPowerControlEXT(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkEnumeratePhysicalDeviceGroupsKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkEnumeratePhysicalDeviceQueueFamilyPerformanceCountersByRegionARM(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: *mut c_void, arg4: *mut c_void) -> i32;
    fn vkEnumeratePhysicalDeviceQueueFamilyPerformanceQueryCountersKHR(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: *mut c_void, arg4: *mut c_void) -> i32;
    fn vkEnumeratePhysicalDeviceShaderInstrumentationMetricsARM(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkExportMetalObjectsEXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkGetAccelerationStructureBuildSizesKHR(arg0: *mut c_void, arg1: i32, arg2: *mut c_void, arg3: *mut c_void, arg4: *mut c_void);
    fn vkGetAccelerationStructureDeviceAddressKHR(arg0: *mut c_void, arg1: *mut c_void) -> u64;
    fn vkGetAccelerationStructureHandleNV(arg0: *mut c_void, arg1: u64, arg2: usize, arg3: *mut c_void) -> i32;
    fn vkGetAccelerationStructureMemoryRequirementsNV(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetAccelerationStructureOpaqueCaptureDescriptorDataEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetAndroidHardwareBufferPropertiesANDROID(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetBufferCollectionPropertiesFUCHSIA(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkGetBufferDeviceAddressEXT(arg0: *mut c_void, arg1: *mut c_void) -> u64;
    fn vkGetBufferDeviceAddressKHR(arg0: *mut c_void, arg1: *mut c_void) -> u64;
    fn vkGetBufferMemoryRequirements2KHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetBufferOpaqueCaptureAddressKHR(arg0: *mut c_void, arg1: *mut c_void) -> u64;
    fn vkGetBufferOpaqueCaptureDescriptorDataEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetCalibratedTimestampsEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: *mut c_void, arg4: *mut c_void) -> i32;
    fn vkGetCalibratedTimestampsKHR(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: *mut c_void, arg4: *mut c_void) -> i32;
    fn vkGetClusterAccelerationStructureBuildSizesNV(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetCommandPoolMemoryConsumption(arg0: *mut c_void, arg1: u64, arg2: *mut c_void, arg3: *mut c_void);
    fn vkGetCudaModuleCacheNV(arg0: *mut c_void, arg1: u64, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetDataGraphPipelineAvailablePropertiesARM(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetDataGraphPipelinePropertiesARM(arg0: *mut c_void, arg1: *mut c_void, arg2: u32, arg3: *mut c_void) -> i32;
    fn vkGetDataGraphPipelineSessionBindPointRequirementsARM(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetDataGraphPipelineSessionMemoryRequirementsARM(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetDeferredOperationMaxConcurrencyKHR(arg0: *mut c_void, arg1: u64) -> u32;
    fn vkGetDeferredOperationResultKHR(arg0: *mut c_void, arg1: u64) -> i32;
    fn vkGetDescriptorEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: usize, arg3: *mut c_void);
    fn vkGetDescriptorSetHostMappingVALVE(arg0: *mut c_void, arg1: u64, arg2: *mut c_void);
    fn vkGetDescriptorSetLayoutBindingOffsetEXT(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: *mut c_void);
    fn vkGetDescriptorSetLayoutHostMappingInfoVALVE(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetDescriptorSetLayoutSizeEXT(arg0: *mut c_void, arg1: u64, arg2: *mut c_void);
    fn vkGetDescriptorSetLayoutSupportKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetDeviceAccelerationStructureCompatibilityKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetDeviceBufferMemoryRequirementsKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetDeviceCombinedImageSamplerIndexNVX(arg0: *mut c_void, arg1: u64, arg2: u64) -> u64;
    fn vkGetDeviceFaultDebugInfoKHR(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkGetDeviceFaultInfoEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetDeviceFaultReportsKHR(arg0: *mut c_void, arg1: u64, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetDeviceGroupPeerMemoryFeaturesKHR(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: u32, arg4: *mut c_void);
    fn vkGetDeviceGroupSurfacePresentModes2EXT(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetDeviceImageMemoryRequirementsKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetDeviceImageSparseMemoryRequirementsKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void, arg3: *mut c_void);
    fn vkGetDeviceImageSubresourceLayoutKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetDeviceMemoryOpaqueCaptureAddressKHR(arg0: *mut c_void, arg1: *mut c_void) -> u64;
    fn vkGetDeviceMicromapCompatibilityEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetDeviceSubpassShadingMaxWorkgroupSizeHUAWEI(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkGetDeviceTensorMemoryRequirementsARM(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetDrmDisplayEXT(arg0: *mut c_void, arg1: i32, arg2: u32, arg3: *mut c_void) -> i32;
    fn vkGetDynamicRenderingTilePropertiesQCOM(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetEncodedVideoSessionParametersKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void, arg3: *mut c_void, arg4: *mut c_void) -> i32;
    fn vkGetExecutionGraphPipelineNodeIndexAMDX(arg0: *mut c_void, arg1: u64, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetExecutionGraphPipelineScratchSizeAMDX(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkGetExternalComputeQueueDataNV(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetFaultData(arg0: *mut c_void, arg1: i32, arg2: *mut c_void, arg3: *mut c_void, arg4: *mut c_void) -> i32;
    fn vkGetFenceFdKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetFenceSciSyncFenceNV(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetFenceSciSyncObjNV(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetFenceWin32HandleKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetFramebufferTilePropertiesQCOM(arg0: *mut c_void, arg1: u64, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetGeneratedCommandsMemoryRequirementsEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetGeneratedCommandsMemoryRequirementsNV(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetGpaDeviceClockInfoAMD(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkGetGpaSessionResultsAMD(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: *mut c_void, arg4: *mut c_void) -> i32;
    fn vkGetGpaSessionStatusAMD(arg0: *mut c_void, arg1: u64) -> i32;
    fn vkGetImageDrmFormatModifierPropertiesEXT(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkGetImageMemoryRequirements2KHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetImageOpaqueCaptureDataEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetImageOpaqueCaptureDescriptorDataEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetImageSparseMemoryRequirements2KHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void, arg3: *mut c_void);
    fn vkGetImageSubresourceLayout2EXT(arg0: *mut c_void, arg1: u64, arg2: *mut c_void, arg3: *mut c_void);
    fn vkGetImageSubresourceLayout2KHR(arg0: *mut c_void, arg1: u64, arg2: *mut c_void, arg3: *mut c_void);
    fn vkGetImageViewAddressNVX(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkGetImageViewHandle64NVX(arg0: *mut c_void, arg1: *mut c_void) -> u64;
    fn vkGetImageViewHandleNVX(arg0: *mut c_void, arg1: *mut c_void) -> u32;
    fn vkGetImageViewOpaqueCaptureDescriptorDataEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetLatencyTimingsLegacyNV(arg0: *mut c_void, arg1: *mut c_void);
    fn vkGetLatencyTimingsNV(arg0: *mut c_void, arg1: u64, arg2: *mut c_void);
    fn vkGetMemoryAndroidHardwareBufferANDROID(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetMemoryFdKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetMemoryFdPropertiesKHR(arg0: *mut c_void, arg1: i32, arg2: i32, arg3: *mut c_void) -> i32;
    fn vkGetMemoryHostPointerPropertiesEXT(arg0: *mut c_void, arg1: i32, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetMemoryMetalHandleEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetMemoryMetalHandlePropertiesEXT(arg0: *mut c_void, arg1: i32, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetMemoryNativeBufferOHOS(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetMemoryRemoteAddressNV(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetMemorySciBufNV(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetMemoryWin32HandleKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetMemoryWin32HandleNV(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: *mut c_void) -> i32;
    fn vkGetMemoryWin32HandlePropertiesKHR(arg0: *mut c_void, arg1: i32, arg2: usize, arg3: *mut c_void) -> i32;
    fn vkGetMemoryZirconHandleFUCHSIA(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetMemoryZirconHandlePropertiesFUCHSIA(arg0: *mut c_void, arg1: i32, arg2: u32, arg3: *mut c_void) -> i32;
    fn vkGetMicromapBuildSizesEXT(arg0: *mut c_void, arg1: i32, arg2: *mut c_void, arg3: *mut c_void);
    fn vkGetNativeBufferPropertiesOHOS(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetPartitionedAccelerationStructuresBuildSizesNV(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetPastPresentationTimingEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetPastPresentationTimingGOOGLE(arg0: *mut c_void, arg1: u64, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetPerformanceParameterINTEL(arg0: *mut c_void, arg1: i32, arg2: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceCalibrateableTimeDomainsEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceCalibrateableTimeDomainsKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceCooperativeMatrixFlexibleDimensionsPropertiesNV(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceCooperativeMatrixProperties2EXT(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceCooperativeMatrixPropertiesKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceCooperativeMatrixPropertiesNV(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceCooperativeVectorPropertiesNV(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceDescriptorSizeEXT(arg0: *mut c_void, arg1: i32) -> u64;
    fn vkGetPhysicalDeviceDirectFBPresentationSupportEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void) -> u32;
    fn vkGetPhysicalDeviceExternalBufferPropertiesKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetPhysicalDeviceExternalFencePropertiesKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetPhysicalDeviceExternalImageFormatPropertiesNV(arg0: *mut c_void, arg1: i32, arg2: i32, arg3: i32, arg4: u32, arg5: u32, arg6: u32, arg7: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceExternalMemorySciBufPropertiesNV(arg0: *mut c_void, arg1: i32, arg2: usize, arg3: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceExternalSemaphorePropertiesKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetPhysicalDeviceExternalTensorPropertiesARM(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetPhysicalDeviceFeatures2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkGetPhysicalDeviceFormatProperties2KHR(arg0: *mut c_void, arg1: i32, arg2: *mut c_void);
    fn vkGetPhysicalDeviceFragmentShadingRatesKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceImageFormatProperties2KHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceMemoryProperties2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkGetPhysicalDeviceMultisamplePropertiesEXT(arg0: *mut c_void, arg1: i32, arg2: *mut c_void);
    fn vkGetPhysicalDeviceOpticalFlowImageFormatsNV(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceProperties2KHR(arg0: *mut c_void, arg1: *mut c_void);
    fn vkGetPhysicalDeviceQueueFamilyDataGraphEngineOperationPropertiesARM(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceQueueFamilyDataGraphOpticalFlowImageFormatsARM(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: *mut c_void, arg4: *mut c_void, arg5: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceQueueFamilyDataGraphProcessingEnginePropertiesARM(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetPhysicalDeviceQueueFamilyDataGraphPropertiesARM(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceQueueFamilyPerformanceQueryPassesKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetPhysicalDeviceQueueFamilyProperties2KHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetPhysicalDeviceRefreshableObjectTypesKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceSciBufAttributesNV(arg0: *mut c_void, arg1: usize) -> i32;
    fn vkGetPhysicalDeviceSciSyncAttributesNV(arg0: *mut c_void, arg1: *mut c_void, arg2: usize) -> i32;
    fn vkGetPhysicalDeviceScreenPresentationSupportQNX(arg0: *mut c_void, arg1: u32, arg2: *mut c_void) -> u32;
    fn vkGetPhysicalDeviceSparseImageFormatProperties2KHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void, arg3: *mut c_void);
    fn vkGetPhysicalDeviceSupportedFramebufferMixedSamplesCombinationsNV(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceSurfaceCapabilities2EXT(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceSurfacePresentModes2EXT(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceToolPropertiesEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceUbmPresentationSupportSEC(arg0: *mut c_void, arg1: u32, arg2: *mut c_void) -> u32;
    fn vkGetPhysicalDeviceVideoCapabilitiesKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceVideoEncodeQualityLevelPropertiesKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetPhysicalDeviceVideoFormatPropertiesKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetPipelineBinaryDataKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void, arg3: *mut c_void, arg4: *mut c_void) -> i32;
    fn vkGetPipelineExecutableInternalRepresentationsKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetPipelineExecutablePropertiesKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetPipelineExecutableStatisticsKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetPipelineIndirectDeviceAddressNV(arg0: *mut c_void, arg1: *mut c_void) -> u64;
    fn vkGetPipelineIndirectMemoryRequirementsNV(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetPipelineKeyKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetPipelinePropertiesEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetPrivateDataEXT(arg0: *mut c_void, arg1: i32, arg2: u64, arg3: u64, arg4: *mut c_void);
    fn vkGetQueueCheckpointData2NV(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetQueueCheckpointDataNV(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetRandROutputDisplayEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: usize, arg3: *mut c_void) -> i32;
    fn vkGetRayTracingCaptureReplayShaderGroupHandlesKHR(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: u32, arg4: usize, arg5: *mut c_void) -> i32;
    fn vkGetRayTracingShaderGroupHandlesKHR(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: u32, arg4: usize, arg5: *mut c_void) -> i32;
    fn vkGetRayTracingShaderGroupHandlesNV(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: u32, arg4: usize, arg5: *mut c_void) -> i32;
    fn vkGetRayTracingShaderGroupStackSizeKHR(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: i32) -> u64;
    fn vkGetRefreshCycleDurationGOOGLE(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkGetRenderingAreaGranularityKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetSamplerOpaqueCaptureDescriptorDataEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetScreenBufferPropertiesQNX(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetSemaphoreCounterValueKHR(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkGetSemaphoreFdKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetSemaphoreSciSyncObjNV(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetSemaphoreWin32HandleKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetSemaphoreZirconHandleFUCHSIA(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetShaderBinaryDataEXT(arg0: *mut c_void, arg1: u64, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetShaderInfoAMD(arg0: *mut c_void, arg1: u64, arg2: i32, arg3: i32, arg4: *mut c_void, arg5: *mut c_void) -> i32;
    fn vkGetShaderInstrumentationValuesARM(arg0: *mut c_void, arg1: u64, arg2: *mut c_void, arg3: *mut c_void, arg4: u32) -> i32;
    fn vkGetShaderModuleCreateInfoIdentifierEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetShaderModuleIdentifierEXT(arg0: *mut c_void, arg1: u64, arg2: *mut c_void);
    fn vkGetSleepStatusLegacyNV(arg0: *mut c_void, arg1: *mut c_void);
    fn vkGetSwapchainCounterEXT(arg0: *mut c_void, arg1: u64, arg2: i32, arg3: *mut c_void) -> i32;
    fn vkGetSwapchainGrallocUsage2ANDROID(arg0: *mut c_void, arg1: i32, arg2: u32, arg3: u32, arg4: *mut c_void, arg5: *mut c_void) -> i32;
    fn vkGetSwapchainGrallocUsageANDROID(arg0: *mut c_void, arg1: i32, arg2: u32, arg3: *mut c_void) -> i32;
    fn vkGetSwapchainGrallocUsageOHOS(arg0: *mut c_void, arg1: i32, arg2: u32, arg3: *mut c_void) -> i32;
    fn vkGetSwapchainStatusKHR(arg0: *mut c_void, arg1: u64) -> i32;
    fn vkGetSwapchainTimeDomainPropertiesEXT(arg0: *mut c_void, arg1: u64, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetSwapchainTimingPropertiesEXT(arg0: *mut c_void, arg1: u64, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetTensorMemoryRequirementsARM(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void);
    fn vkGetTensorOpaqueCaptureDataARM(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetTensorOpaqueCaptureDescriptorDataARM(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetTensorViewOpaqueCaptureDescriptorDataARM(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkGetValidationCacheDataEXT(arg0: *mut c_void, arg1: u64, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetVideoSessionMemoryRequirementsKHR(arg0: *mut c_void, arg1: u64, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkGetWinrtDisplayNV(arg0: *mut c_void, arg1: u32, arg2: *mut c_void) -> i32;
    fn vkImportFenceFdKHR(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkImportFenceSciSyncFenceNV(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkImportFenceSciSyncObjNV(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkImportFenceWin32HandleKHR(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkImportSemaphoreFdKHR(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkImportSemaphoreSciSyncObjNV(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkImportSemaphoreWin32HandleKHR(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkImportSemaphoreZirconHandleFUCHSIA(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkInitializePerformanceApiINTEL(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkLatencySleepLegacyNV(arg0: *mut c_void, arg1: u64, arg2: u64);
    fn vkLatencySleepNV(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkMapMemory2KHR(arg0: *mut c_void, arg1: *mut c_void, arg2: *mut c_void) -> i32;
    fn vkMergeValidationCachesEXT(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: *mut c_void) -> i32;
    fn vkQueueBeginDebugUtilsLabelEXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkQueueEndDebugUtilsLabelEXT(arg0: *mut c_void);
    fn vkQueueInsertDebugUtilsLabelEXT(arg0: *mut c_void, arg1: *mut c_void);
    fn vkQueueNotifyOutOfBandLegacyNV(arg0: *mut c_void, arg1: u32);
    fn vkQueueNotifyOutOfBandNV(arg0: *mut c_void, arg1: *mut c_void);
    fn vkQueueSetPerfHintQCOM(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkQueueSetPerformanceConfigurationINTEL(arg0: *mut c_void, arg1: u64) -> i32;
    fn vkQueueSignalReleaseImageANDROID(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: u64, arg4: *mut c_void) -> i32;
    fn vkQueueSignalReleaseImageOHOS(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: u64, arg4: *mut c_void) -> i32;
    fn vkQueueSubmit2KHR(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: u64) -> i32;
    fn vkRegisterCustomBorderColorEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: u32, arg3: *mut c_void) -> i32;
    fn vkRegisterDeviceEventEXT(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator, arg3: *mut c_void) -> i32;
    fn vkRegisterDisplayEventEXT(arg0: *mut c_void, arg1: u64, arg2: *mut c_void, arg3: Allocator, arg4: *mut c_void) -> i32;
    fn vkReleaseCapturedPipelineDataKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: Allocator) -> i32;
    fn vkReleaseDisplayEXT(arg0: *mut c_void, arg1: u64) -> i32;
    fn vkReleaseFullScreenExclusiveModeEXT(arg0: *mut c_void, arg1: u64) -> i32;
    fn vkReleasePerformanceConfigurationINTEL(arg0: *mut c_void, arg1: u64) -> i32;
    fn vkReleaseProfilingLockKHR(arg0: *mut c_void);
    fn vkReleaseSwapchainImagesEXT(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkReleaseSwapchainImagesKHR(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkResetGpaSessionAMD(arg0: *mut c_void, arg1: u64) -> i32;
    fn vkResetQueryPoolEXT(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: u32);
    fn vkSetBufferCollectionBufferConstraintsFUCHSIA(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkSetBufferCollectionImageConstraintsFUCHSIA(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkSetDebugUtilsObjectNameEXT(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkSetDebugUtilsObjectTagEXT(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkSetDeviceMemoryPriorityEXT(arg0: *mut c_void, arg1: u64, arg2: f32);
    fn vkSetGpaDeviceClockModeAMD(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkSetHdrMetadataEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: *mut c_void);
    fn vkSetLatencyMarkerLegacyNV(arg0: *mut c_void, arg1: u64, arg2: u32);
    fn vkSetLatencyMarkerNV(arg0: *mut c_void, arg1: u64, arg2: *mut c_void);
    fn vkSetLatencySleepModeLegacyNV(arg0: *mut c_void, arg1: u32, arg2: u32, arg3: u32);
    fn vkSetLatencySleepModeNV(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkSetLocalDimmingAMD(arg0: *mut c_void, arg1: u64, arg2: u32);
    fn vkSetPrivateDataEXT(arg0: *mut c_void, arg1: i32, arg2: u64, arg3: u64, arg4: u64) -> i32;
    fn vkSetSwapchainPresentTimingQueueSizeEXT(arg0: *mut c_void, arg1: u64, arg2: u32) -> i32;
    fn vkShutdownLatencyDeviceLegacyNV(arg0: *mut c_void);
    fn vkSignalSemaphoreKHR(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkSubmitDebugUtilsMessageEXT(arg0: *mut c_void, arg1: i32, arg2: u32, arg3: *mut c_void);
    fn vkTransitionImageLayoutEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void) -> i32;
    fn vkTrimCommandPoolKHR(arg0: *mut c_void, arg1: u64, arg2: u32);
    fn vkUninitializePerformanceApiINTEL(arg0: *mut c_void);
    fn vkUnmapMemory2KHR(arg0: *mut c_void, arg1: *mut c_void) -> i32;
    fn vkUnregisterCustomBorderColorEXT(arg0: *mut c_void, arg1: u32);
    fn vkUpdateDescriptorSetWithTemplateKHR(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: *mut c_void);
    fn vkUpdateIndirectExecutionSetPipelineEXT(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: *mut c_void);
    fn vkUpdateIndirectExecutionSetShaderEXT(arg0: *mut c_void, arg1: u64, arg2: u32, arg3: *mut c_void);
    fn vkUpdateVideoSessionParametersKHR(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkWaitForPresent2KHR(arg0: *mut c_void, arg1: u64, arg2: *mut c_void) -> i32;
    fn vkWaitForPresentKHR(arg0: *mut c_void, arg1: u64, arg2: u64, arg3: u64) -> i32;
    fn vkWaitSemaphoresKHR(arg0: *mut c_void, arg1: *mut c_void, arg2: u64) -> i32;
    fn vkWriteAccelerationStructuresPropertiesKHR(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: i32, arg4: usize, arg5: *mut c_void, arg6: usize) -> i32;
    fn vkWriteMicromapsPropertiesEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: i32, arg4: usize, arg5: *mut c_void, arg6: usize) -> i32;
    fn vkWriteResourceDescriptorsEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: *mut c_void) -> i32;
    fn vkWriteSamplerDescriptorsEXT(arg0: *mut c_void, arg1: u32, arg2: *mut c_void, arg3: *mut c_void) -> i32;
}

fn bind(name: &str, address: usize) -> Option<usize> {
    match name {
        "vkAcquireDrmDisplayEXT" => {
            vkAcquireDrmDisplayEXT::HOST.bind(address);
            Some(vkAcquireDrmDisplayEXT as *const () as usize)
        }
        "vkAcquireFullScreenExclusiveModeEXT" => {
            vkAcquireFullScreenExclusiveModeEXT::HOST.bind(address);
            Some(vkAcquireFullScreenExclusiveModeEXT as *const () as usize)
        }
        "vkAcquireImageANDROID" => {
            vkAcquireImageANDROID::HOST.bind(address);
            Some(vkAcquireImageANDROID as *const () as usize)
        }
        "vkAcquireImageOHOS" => {
            vkAcquireImageOHOS::HOST.bind(address);
            Some(vkAcquireImageOHOS as *const () as usize)
        }
        "vkAcquirePerformanceConfigurationINTEL" => {
            vkAcquirePerformanceConfigurationINTEL::HOST.bind(address);
            Some(vkAcquirePerformanceConfigurationINTEL as *const () as usize)
        }
        "vkAcquireProfilingLockKHR" => {
            vkAcquireProfilingLockKHR::HOST.bind(address);
            Some(vkAcquireProfilingLockKHR as *const () as usize)
        }
        "vkAcquireWinrtDisplayNV" => {
            vkAcquireWinrtDisplayNV::HOST.bind(address);
            Some(vkAcquireWinrtDisplayNV as *const () as usize)
        }
        "vkAcquireXlibDisplayEXT" => {
            vkAcquireXlibDisplayEXT::HOST.bind(address);
            Some(vkAcquireXlibDisplayEXT as *const () as usize)
        }
        "vkAntiLagUpdateAMD" => {
            vkAntiLagUpdateAMD::HOST.bind(address);
            Some(vkAntiLagUpdateAMD as *const () as usize)
        }
        "vkBindAccelerationStructureMemoryNV" => {
            vkBindAccelerationStructureMemoryNV::HOST.bind(address);
            Some(vkBindAccelerationStructureMemoryNV as *const () as usize)
        }
        "vkBindBufferMemory2KHR" => {
            vkBindBufferMemory2KHR::HOST.bind(address);
            Some(vkBindBufferMemory2KHR as *const () as usize)
        }
        "vkBindDataGraphPipelineSessionMemoryARM" => {
            vkBindDataGraphPipelineSessionMemoryARM::HOST.bind(address);
            Some(vkBindDataGraphPipelineSessionMemoryARM as *const () as usize)
        }
        "vkBindImageMemory2KHR" => {
            vkBindImageMemory2KHR::HOST.bind(address);
            Some(vkBindImageMemory2KHR as *const () as usize)
        }
        "vkBindOpticalFlowSessionImageNV" => {
            vkBindOpticalFlowSessionImageNV::HOST.bind(address);
            Some(vkBindOpticalFlowSessionImageNV as *const () as usize)
        }
        "vkBindTensorMemoryARM" => {
            vkBindTensorMemoryARM::HOST.bind(address);
            Some(vkBindTensorMemoryARM as *const () as usize)
        }
        "vkBindVideoSessionMemoryKHR" => {
            vkBindVideoSessionMemoryKHR::HOST.bind(address);
            Some(vkBindVideoSessionMemoryKHR as *const () as usize)
        }
        "vkBuildAccelerationStructuresKHR" => {
            vkBuildAccelerationStructuresKHR::HOST.bind(address);
            Some(vkBuildAccelerationStructuresKHR as *const () as usize)
        }
        "vkBuildMicromapsEXT" => {
            vkBuildMicromapsEXT::HOST.bind(address);
            Some(vkBuildMicromapsEXT as *const () as usize)
        }
        "vkClearShaderInstrumentationMetricsARM" => {
            vkClearShaderInstrumentationMetricsARM::HOST.bind(address);
            Some(vkClearShaderInstrumentationMetricsARM as *const () as usize)
        }
        "vkCmdBeginConditionalRendering2EXT" => {
            vkCmdBeginConditionalRendering2EXT::HOST.bind(address);
            Some(vkCmdBeginConditionalRendering2EXT as *const () as usize)
        }
        "vkCmdBeginConditionalRenderingEXT" => {
            vkCmdBeginConditionalRenderingEXT::HOST.bind(address);
            Some(vkCmdBeginConditionalRenderingEXT as *const () as usize)
        }
        "vkCmdBeginCustomResolveEXT" => {
            vkCmdBeginCustomResolveEXT::HOST.bind(address);
            Some(vkCmdBeginCustomResolveEXT as *const () as usize)
        }
        "vkCmdBeginDebugUtilsLabelEXT" => {
            vkCmdBeginDebugUtilsLabelEXT::HOST.bind(address);
            Some(vkCmdBeginDebugUtilsLabelEXT as *const () as usize)
        }
        "vkCmdBeginGpaSampleAMD" => {
            vkCmdBeginGpaSampleAMD::HOST.bind(address);
            Some(vkCmdBeginGpaSampleAMD as *const () as usize)
        }
        "vkCmdBeginGpaSessionAMD" => {
            vkCmdBeginGpaSessionAMD::HOST.bind(address);
            Some(vkCmdBeginGpaSessionAMD as *const () as usize)
        }
        "vkCmdBeginPerTileExecutionQCOM" => {
            vkCmdBeginPerTileExecutionQCOM::HOST.bind(address);
            Some(vkCmdBeginPerTileExecutionQCOM as *const () as usize)
        }
        "vkCmdBeginQueryIndexedEXT" => {
            vkCmdBeginQueryIndexedEXT::HOST.bind(address);
            Some(vkCmdBeginQueryIndexedEXT as *const () as usize)
        }
        "vkCmdBeginRenderPass2KHR" => {
            vkCmdBeginRenderPass2KHR::HOST.bind(address);
            Some(vkCmdBeginRenderPass2KHR as *const () as usize)
        }
        "vkCmdBeginRenderingKHR" => {
            vkCmdBeginRenderingKHR::HOST.bind(address);
            Some(vkCmdBeginRenderingKHR as *const () as usize)
        }
        "vkCmdBeginShaderInstrumentationARM" => {
            vkCmdBeginShaderInstrumentationARM::HOST.bind(address);
            Some(vkCmdBeginShaderInstrumentationARM as *const () as usize)
        }
        "vkCmdBeginTransformFeedback2EXT" => {
            vkCmdBeginTransformFeedback2EXT::HOST.bind(address);
            Some(vkCmdBeginTransformFeedback2EXT as *const () as usize)
        }
        "vkCmdBeginTransformFeedbackEXT" => {
            vkCmdBeginTransformFeedbackEXT::HOST.bind(address);
            Some(vkCmdBeginTransformFeedbackEXT as *const () as usize)
        }
        "vkCmdBeginVideoCodingKHR" => {
            vkCmdBeginVideoCodingKHR::HOST.bind(address);
            Some(vkCmdBeginVideoCodingKHR as *const () as usize)
        }
        "vkCmdBindDescriptorBufferEmbeddedSamplers2EXT" => {
            vkCmdBindDescriptorBufferEmbeddedSamplers2EXT::HOST.bind(address);
            Some(vkCmdBindDescriptorBufferEmbeddedSamplers2EXT as *const () as usize)
        }
        "vkCmdBindDescriptorBufferEmbeddedSamplersEXT" => {
            vkCmdBindDescriptorBufferEmbeddedSamplersEXT::HOST.bind(address);
            Some(vkCmdBindDescriptorBufferEmbeddedSamplersEXT as *const () as usize)
        }
        "vkCmdBindDescriptorBuffersEXT" => {
            vkCmdBindDescriptorBuffersEXT::HOST.bind(address);
            Some(vkCmdBindDescriptorBuffersEXT as *const () as usize)
        }
        "vkCmdBindDescriptorSets2KHR" => {
            vkCmdBindDescriptorSets2KHR::HOST.bind(address);
            Some(vkCmdBindDescriptorSets2KHR as *const () as usize)
        }
        "vkCmdBindIndexBuffer2KHR" => {
            vkCmdBindIndexBuffer2KHR::HOST.bind(address);
            Some(vkCmdBindIndexBuffer2KHR as *const () as usize)
        }
        "vkCmdBindIndexBuffer3KHR" => {
            vkCmdBindIndexBuffer3KHR::HOST.bind(address);
            Some(vkCmdBindIndexBuffer3KHR as *const () as usize)
        }
        "vkCmdBindInvocationMaskHUAWEI" => {
            vkCmdBindInvocationMaskHUAWEI::HOST.bind(address);
            Some(vkCmdBindInvocationMaskHUAWEI as *const () as usize)
        }
        "vkCmdBindPipelineShaderGroupNV" => {
            vkCmdBindPipelineShaderGroupNV::HOST.bind(address);
            Some(vkCmdBindPipelineShaderGroupNV as *const () as usize)
        }
        "vkCmdBindResourceHeapEXT" => {
            vkCmdBindResourceHeapEXT::HOST.bind(address);
            Some(vkCmdBindResourceHeapEXT as *const () as usize)
        }
        "vkCmdBindSamplerHeapEXT" => {
            vkCmdBindSamplerHeapEXT::HOST.bind(address);
            Some(vkCmdBindSamplerHeapEXT as *const () as usize)
        }
        "vkCmdBindShadersEXT" => {
            vkCmdBindShadersEXT::HOST.bind(address);
            Some(vkCmdBindShadersEXT as *const () as usize)
        }
        "vkCmdBindShadingRateImageNV" => {
            vkCmdBindShadingRateImageNV::HOST.bind(address);
            Some(vkCmdBindShadingRateImageNV as *const () as usize)
        }
        "vkCmdBindTileMemoryQCOM" => {
            vkCmdBindTileMemoryQCOM::HOST.bind(address);
            Some(vkCmdBindTileMemoryQCOM as *const () as usize)
        }
        "vkCmdBindTransformFeedbackBuffers2EXT" => {
            vkCmdBindTransformFeedbackBuffers2EXT::HOST.bind(address);
            Some(vkCmdBindTransformFeedbackBuffers2EXT as *const () as usize)
        }
        "vkCmdBindTransformFeedbackBuffersEXT" => {
            vkCmdBindTransformFeedbackBuffersEXT::HOST.bind(address);
            Some(vkCmdBindTransformFeedbackBuffersEXT as *const () as usize)
        }
        "vkCmdBindVertexBuffers2EXT" => {
            vkCmdBindVertexBuffers2EXT::HOST.bind(address);
            Some(vkCmdBindVertexBuffers2EXT as *const () as usize)
        }
        "vkCmdBindVertexBuffers3KHR" => {
            vkCmdBindVertexBuffers3KHR::HOST.bind(address);
            Some(vkCmdBindVertexBuffers3KHR as *const () as usize)
        }
        "vkCmdBlitImage2KHR" => {
            vkCmdBlitImage2KHR::HOST.bind(address);
            Some(vkCmdBlitImage2KHR as *const () as usize)
        }
        "vkCmdBuildAccelerationStructureNV" => {
            vkCmdBuildAccelerationStructureNV::HOST.bind(address);
            Some(vkCmdBuildAccelerationStructureNV as *const () as usize)
        }
        "vkCmdBuildAccelerationStructuresIndirectKHR" => {
            vkCmdBuildAccelerationStructuresIndirectKHR::HOST.bind(address);
            Some(vkCmdBuildAccelerationStructuresIndirectKHR as *const () as usize)
        }
        "vkCmdBuildAccelerationStructuresKHR" => {
            vkCmdBuildAccelerationStructuresKHR::HOST.bind(address);
            Some(vkCmdBuildAccelerationStructuresKHR as *const () as usize)
        }
        "vkCmdBuildClusterAccelerationStructureIndirectNV" => {
            vkCmdBuildClusterAccelerationStructureIndirectNV::HOST.bind(address);
            Some(vkCmdBuildClusterAccelerationStructureIndirectNV as *const () as usize)
        }
        "vkCmdBuildMicromapsEXT" => {
            vkCmdBuildMicromapsEXT::HOST.bind(address);
            Some(vkCmdBuildMicromapsEXT as *const () as usize)
        }
        "vkCmdBuildPartitionedAccelerationStructuresNV" => {
            vkCmdBuildPartitionedAccelerationStructuresNV::HOST.bind(address);
            Some(vkCmdBuildPartitionedAccelerationStructuresNV as *const () as usize)
        }
        "vkCmdControlVideoCodingKHR" => {
            vkCmdControlVideoCodingKHR::HOST.bind(address);
            Some(vkCmdControlVideoCodingKHR as *const () as usize)
        }
        "vkCmdConvertCooperativeVectorMatrixNV" => {
            vkCmdConvertCooperativeVectorMatrixNV::HOST.bind(address);
            Some(vkCmdConvertCooperativeVectorMatrixNV as *const () as usize)
        }
        "vkCmdCopyAccelerationStructureKHR" => {
            vkCmdCopyAccelerationStructureKHR::HOST.bind(address);
            Some(vkCmdCopyAccelerationStructureKHR as *const () as usize)
        }
        "vkCmdCopyAccelerationStructureNV" => {
            vkCmdCopyAccelerationStructureNV::HOST.bind(address);
            Some(vkCmdCopyAccelerationStructureNV as *const () as usize)
        }
        "vkCmdCopyAccelerationStructureToMemoryKHR" => {
            vkCmdCopyAccelerationStructureToMemoryKHR::HOST.bind(address);
            Some(vkCmdCopyAccelerationStructureToMemoryKHR as *const () as usize)
        }
        "vkCmdCopyBuffer2KHR" => {
            vkCmdCopyBuffer2KHR::HOST.bind(address);
            Some(vkCmdCopyBuffer2KHR as *const () as usize)
        }
        "vkCmdCopyBufferToImage2KHR" => {
            vkCmdCopyBufferToImage2KHR::HOST.bind(address);
            Some(vkCmdCopyBufferToImage2KHR as *const () as usize)
        }
        "vkCmdCopyGpaSessionResultsAMD" => {
            vkCmdCopyGpaSessionResultsAMD::HOST.bind(address);
            Some(vkCmdCopyGpaSessionResultsAMD as *const () as usize)
        }
        "vkCmdCopyImage2KHR" => {
            vkCmdCopyImage2KHR::HOST.bind(address);
            Some(vkCmdCopyImage2KHR as *const () as usize)
        }
        "vkCmdCopyImageToBuffer2KHR" => {
            vkCmdCopyImageToBuffer2KHR::HOST.bind(address);
            Some(vkCmdCopyImageToBuffer2KHR as *const () as usize)
        }
        "vkCmdCopyImageToMemoryKHR" => {
            vkCmdCopyImageToMemoryKHR::HOST.bind(address);
            Some(vkCmdCopyImageToMemoryKHR as *const () as usize)
        }
        "vkCmdCopyMemoryIndirectKHR" => {
            vkCmdCopyMemoryIndirectKHR::HOST.bind(address);
            Some(vkCmdCopyMemoryIndirectKHR as *const () as usize)
        }
        "vkCmdCopyMemoryIndirectNV" => {
            vkCmdCopyMemoryIndirectNV::HOST.bind(address);
            Some(vkCmdCopyMemoryIndirectNV as *const () as usize)
        }
        "vkCmdCopyMemoryKHR" => {
            vkCmdCopyMemoryKHR::HOST.bind(address);
            Some(vkCmdCopyMemoryKHR as *const () as usize)
        }
        "vkCmdCopyMemoryToAccelerationStructureKHR" => {
            vkCmdCopyMemoryToAccelerationStructureKHR::HOST.bind(address);
            Some(vkCmdCopyMemoryToAccelerationStructureKHR as *const () as usize)
        }
        "vkCmdCopyMemoryToImageIndirectKHR" => {
            vkCmdCopyMemoryToImageIndirectKHR::HOST.bind(address);
            Some(vkCmdCopyMemoryToImageIndirectKHR as *const () as usize)
        }
        "vkCmdCopyMemoryToImageIndirectNV" => {
            vkCmdCopyMemoryToImageIndirectNV::HOST.bind(address);
            Some(vkCmdCopyMemoryToImageIndirectNV as *const () as usize)
        }
        "vkCmdCopyMemoryToImageKHR" => {
            vkCmdCopyMemoryToImageKHR::HOST.bind(address);
            Some(vkCmdCopyMemoryToImageKHR as *const () as usize)
        }
        "vkCmdCopyMemoryToMicromapEXT" => {
            vkCmdCopyMemoryToMicromapEXT::HOST.bind(address);
            Some(vkCmdCopyMemoryToMicromapEXT as *const () as usize)
        }
        "vkCmdCopyMicromapEXT" => {
            vkCmdCopyMicromapEXT::HOST.bind(address);
            Some(vkCmdCopyMicromapEXT as *const () as usize)
        }
        "vkCmdCopyMicromapToMemoryEXT" => {
            vkCmdCopyMicromapToMemoryEXT::HOST.bind(address);
            Some(vkCmdCopyMicromapToMemoryEXT as *const () as usize)
        }
        "vkCmdCopyQueryPoolResultsToMemoryKHR" => {
            vkCmdCopyQueryPoolResultsToMemoryKHR::HOST.bind(address);
            Some(vkCmdCopyQueryPoolResultsToMemoryKHR as *const () as usize)
        }
        "vkCmdCopyTensorARM" => {
            vkCmdCopyTensorARM::HOST.bind(address);
            Some(vkCmdCopyTensorARM as *const () as usize)
        }
        "vkCmdCuLaunchKernelNVX" => {
            vkCmdCuLaunchKernelNVX::HOST.bind(address);
            Some(vkCmdCuLaunchKernelNVX as *const () as usize)
        }
        "vkCmdCudaLaunchKernelNV" => {
            vkCmdCudaLaunchKernelNV::HOST.bind(address);
            Some(vkCmdCudaLaunchKernelNV as *const () as usize)
        }
        "vkCmdDebugMarkerBeginEXT" => {
            vkCmdDebugMarkerBeginEXT::HOST.bind(address);
            Some(vkCmdDebugMarkerBeginEXT as *const () as usize)
        }
        "vkCmdDebugMarkerEndEXT" => {
            vkCmdDebugMarkerEndEXT::HOST.bind(address);
            Some(vkCmdDebugMarkerEndEXT as *const () as usize)
        }
        "vkCmdDebugMarkerInsertEXT" => {
            vkCmdDebugMarkerInsertEXT::HOST.bind(address);
            Some(vkCmdDebugMarkerInsertEXT as *const () as usize)
        }
        "vkCmdDecodeVideoKHR" => {
            vkCmdDecodeVideoKHR::HOST.bind(address);
            Some(vkCmdDecodeVideoKHR as *const () as usize)
        }
        "vkCmdDecompressMemoryEXT" => {
            vkCmdDecompressMemoryEXT::HOST.bind(address);
            Some(vkCmdDecompressMemoryEXT as *const () as usize)
        }
        "vkCmdDecompressMemoryIndirectCountEXT" => {
            vkCmdDecompressMemoryIndirectCountEXT::HOST.bind(address);
            Some(vkCmdDecompressMemoryIndirectCountEXT as *const () as usize)
        }
        "vkCmdDecompressMemoryIndirectCountNV" => {
            vkCmdDecompressMemoryIndirectCountNV::HOST.bind(address);
            Some(vkCmdDecompressMemoryIndirectCountNV as *const () as usize)
        }
        "vkCmdDecompressMemoryNV" => {
            vkCmdDecompressMemoryNV::HOST.bind(address);
            Some(vkCmdDecompressMemoryNV as *const () as usize)
        }
        "vkCmdDispatchBaseKHR" => {
            vkCmdDispatchBaseKHR::HOST.bind(address);
            Some(vkCmdDispatchBaseKHR as *const () as usize)
        }
        "vkCmdDispatchDataGraphARM" => {
            vkCmdDispatchDataGraphARM::HOST.bind(address);
            Some(vkCmdDispatchDataGraphARM as *const () as usize)
        }
        "vkCmdDispatchGraphAMDX" => {
            vkCmdDispatchGraphAMDX::HOST.bind(address);
            Some(vkCmdDispatchGraphAMDX as *const () as usize)
        }
        "vkCmdDispatchGraphIndirectAMDX" => {
            vkCmdDispatchGraphIndirectAMDX::HOST.bind(address);
            Some(vkCmdDispatchGraphIndirectAMDX as *const () as usize)
        }
        "vkCmdDispatchGraphIndirectCountAMDX" => {
            vkCmdDispatchGraphIndirectCountAMDX::HOST.bind(address);
            Some(vkCmdDispatchGraphIndirectCountAMDX as *const () as usize)
        }
        "vkCmdDispatchIndirect2KHR" => {
            vkCmdDispatchIndirect2KHR::HOST.bind(address);
            Some(vkCmdDispatchIndirect2KHR as *const () as usize)
        }
        "vkCmdDispatchTileQCOM" => {
            vkCmdDispatchTileQCOM::HOST.bind(address);
            Some(vkCmdDispatchTileQCOM as *const () as usize)
        }
        "vkCmdDrawClusterHUAWEI" => {
            vkCmdDrawClusterHUAWEI::HOST.bind(address);
            Some(vkCmdDrawClusterHUAWEI as *const () as usize)
        }
        "vkCmdDrawClusterIndirectHUAWEI" => {
            vkCmdDrawClusterIndirectHUAWEI::HOST.bind(address);
            Some(vkCmdDrawClusterIndirectHUAWEI as *const () as usize)
        }
        "vkCmdDrawIndexedIndirect2KHR" => {
            vkCmdDrawIndexedIndirect2KHR::HOST.bind(address);
            Some(vkCmdDrawIndexedIndirect2KHR as *const () as usize)
        }
        "vkCmdDrawIndexedIndirectCount2KHR" => {
            vkCmdDrawIndexedIndirectCount2KHR::HOST.bind(address);
            Some(vkCmdDrawIndexedIndirectCount2KHR as *const () as usize)
        }
        "vkCmdDrawIndexedIndirectCountAMD" => {
            vkCmdDrawIndexedIndirectCountAMD::HOST.bind(address);
            Some(vkCmdDrawIndexedIndirectCountAMD as *const () as usize)
        }
        "vkCmdDrawIndexedIndirectCountKHR" => {
            vkCmdDrawIndexedIndirectCountKHR::HOST.bind(address);
            Some(vkCmdDrawIndexedIndirectCountKHR as *const () as usize)
        }
        "vkCmdDrawIndirect2KHR" => {
            vkCmdDrawIndirect2KHR::HOST.bind(address);
            Some(vkCmdDrawIndirect2KHR as *const () as usize)
        }
        "vkCmdDrawIndirectByteCount2EXT" => {
            vkCmdDrawIndirectByteCount2EXT::HOST.bind(address);
            Some(vkCmdDrawIndirectByteCount2EXT as *const () as usize)
        }
        "vkCmdDrawIndirectByteCountEXT" => {
            vkCmdDrawIndirectByteCountEXT::HOST.bind(address);
            Some(vkCmdDrawIndirectByteCountEXT as *const () as usize)
        }
        "vkCmdDrawIndirectCount2KHR" => {
            vkCmdDrawIndirectCount2KHR::HOST.bind(address);
            Some(vkCmdDrawIndirectCount2KHR as *const () as usize)
        }
        "vkCmdDrawIndirectCountAMD" => {
            vkCmdDrawIndirectCountAMD::HOST.bind(address);
            Some(vkCmdDrawIndirectCountAMD as *const () as usize)
        }
        "vkCmdDrawIndirectCountKHR" => {
            vkCmdDrawIndirectCountKHR::HOST.bind(address);
            Some(vkCmdDrawIndirectCountKHR as *const () as usize)
        }
        "vkCmdDrawMeshTasksEXT" => {
            vkCmdDrawMeshTasksEXT::HOST.bind(address);
            Some(vkCmdDrawMeshTasksEXT as *const () as usize)
        }
        "vkCmdDrawMeshTasksIndirect2EXT" => {
            vkCmdDrawMeshTasksIndirect2EXT::HOST.bind(address);
            Some(vkCmdDrawMeshTasksIndirect2EXT as *const () as usize)
        }
        "vkCmdDrawMeshTasksIndirectCount2EXT" => {
            vkCmdDrawMeshTasksIndirectCount2EXT::HOST.bind(address);
            Some(vkCmdDrawMeshTasksIndirectCount2EXT as *const () as usize)
        }
        "vkCmdDrawMeshTasksIndirectCountEXT" => {
            vkCmdDrawMeshTasksIndirectCountEXT::HOST.bind(address);
            Some(vkCmdDrawMeshTasksIndirectCountEXT as *const () as usize)
        }
        "vkCmdDrawMeshTasksIndirectCountNV" => {
            vkCmdDrawMeshTasksIndirectCountNV::HOST.bind(address);
            Some(vkCmdDrawMeshTasksIndirectCountNV as *const () as usize)
        }
        "vkCmdDrawMeshTasksIndirectEXT" => {
            vkCmdDrawMeshTasksIndirectEXT::HOST.bind(address);
            Some(vkCmdDrawMeshTasksIndirectEXT as *const () as usize)
        }
        "vkCmdDrawMeshTasksIndirectNV" => {
            vkCmdDrawMeshTasksIndirectNV::HOST.bind(address);
            Some(vkCmdDrawMeshTasksIndirectNV as *const () as usize)
        }
        "vkCmdDrawMeshTasksNV" => {
            vkCmdDrawMeshTasksNV::HOST.bind(address);
            Some(vkCmdDrawMeshTasksNV as *const () as usize)
        }
        "vkCmdDrawMultiEXT" => {
            vkCmdDrawMultiEXT::HOST.bind(address);
            Some(vkCmdDrawMultiEXT as *const () as usize)
        }
        "vkCmdDrawMultiIndexedEXT" => {
            vkCmdDrawMultiIndexedEXT::HOST.bind(address);
            Some(vkCmdDrawMultiIndexedEXT as *const () as usize)
        }
        "vkCmdEncodeVideoKHR" => {
            vkCmdEncodeVideoKHR::HOST.bind(address);
            Some(vkCmdEncodeVideoKHR as *const () as usize)
        }
        "vkCmdEndConditionalRenderingEXT" => {
            vkCmdEndConditionalRenderingEXT::HOST.bind(address);
            Some(vkCmdEndConditionalRenderingEXT as *const () as usize)
        }
        "vkCmdEndDebugUtilsLabelEXT" => {
            vkCmdEndDebugUtilsLabelEXT::HOST.bind(address);
            Some(vkCmdEndDebugUtilsLabelEXT as *const () as usize)
        }
        "vkCmdEndGpaSampleAMD" => {
            vkCmdEndGpaSampleAMD::HOST.bind(address);
            Some(vkCmdEndGpaSampleAMD as *const () as usize)
        }
        "vkCmdEndGpaSessionAMD" => {
            vkCmdEndGpaSessionAMD::HOST.bind(address);
            Some(vkCmdEndGpaSessionAMD as *const () as usize)
        }
        "vkCmdEndPerTileExecutionQCOM" => {
            vkCmdEndPerTileExecutionQCOM::HOST.bind(address);
            Some(vkCmdEndPerTileExecutionQCOM as *const () as usize)
        }
        "vkCmdEndQueryIndexedEXT" => {
            vkCmdEndQueryIndexedEXT::HOST.bind(address);
            Some(vkCmdEndQueryIndexedEXT as *const () as usize)
        }
        "vkCmdEndRenderPass2KHR" => {
            vkCmdEndRenderPass2KHR::HOST.bind(address);
            Some(vkCmdEndRenderPass2KHR as *const () as usize)
        }
        "vkCmdEndRendering2EXT" => {
            vkCmdEndRendering2EXT::HOST.bind(address);
            Some(vkCmdEndRendering2EXT as *const () as usize)
        }
        "vkCmdEndRendering2KHR" => {
            vkCmdEndRendering2KHR::HOST.bind(address);
            Some(vkCmdEndRendering2KHR as *const () as usize)
        }
        "vkCmdEndRenderingKHR" => {
            vkCmdEndRenderingKHR::HOST.bind(address);
            Some(vkCmdEndRenderingKHR as *const () as usize)
        }
        "vkCmdEndShaderInstrumentationARM" => {
            vkCmdEndShaderInstrumentationARM::HOST.bind(address);
            Some(vkCmdEndShaderInstrumentationARM as *const () as usize)
        }
        "vkCmdEndTransformFeedback2EXT" => {
            vkCmdEndTransformFeedback2EXT::HOST.bind(address);
            Some(vkCmdEndTransformFeedback2EXT as *const () as usize)
        }
        "vkCmdEndTransformFeedbackEXT" => {
            vkCmdEndTransformFeedbackEXT::HOST.bind(address);
            Some(vkCmdEndTransformFeedbackEXT as *const () as usize)
        }
        "vkCmdEndVideoCodingKHR" => {
            vkCmdEndVideoCodingKHR::HOST.bind(address);
            Some(vkCmdEndVideoCodingKHR as *const () as usize)
        }
        "vkCmdExecuteGeneratedCommandsEXT" => {
            vkCmdExecuteGeneratedCommandsEXT::HOST.bind(address);
            Some(vkCmdExecuteGeneratedCommandsEXT as *const () as usize)
        }
        "vkCmdExecuteGeneratedCommandsNV" => {
            vkCmdExecuteGeneratedCommandsNV::HOST.bind(address);
            Some(vkCmdExecuteGeneratedCommandsNV as *const () as usize)
        }
        "vkCmdFillMemoryKHR" => {
            vkCmdFillMemoryKHR::HOST.bind(address);
            Some(vkCmdFillMemoryKHR as *const () as usize)
        }
        "vkCmdInitializeGraphScratchMemoryAMDX" => {
            vkCmdInitializeGraphScratchMemoryAMDX::HOST.bind(address);
            Some(vkCmdInitializeGraphScratchMemoryAMDX as *const () as usize)
        }
        "vkCmdInsertDebugUtilsLabelEXT" => {
            vkCmdInsertDebugUtilsLabelEXT::HOST.bind(address);
            Some(vkCmdInsertDebugUtilsLabelEXT as *const () as usize)
        }
        "vkCmdNextSubpass2KHR" => {
            vkCmdNextSubpass2KHR::HOST.bind(address);
            Some(vkCmdNextSubpass2KHR as *const () as usize)
        }
        "vkCmdOpticalFlowExecuteNV" => {
            vkCmdOpticalFlowExecuteNV::HOST.bind(address);
            Some(vkCmdOpticalFlowExecuteNV as *const () as usize)
        }
        "vkCmdPipelineBarrier2KHR" => {
            vkCmdPipelineBarrier2KHR::HOST.bind(address);
            Some(vkCmdPipelineBarrier2KHR as *const () as usize)
        }
        "vkCmdPreprocessGeneratedCommandsEXT" => {
            vkCmdPreprocessGeneratedCommandsEXT::HOST.bind(address);
            Some(vkCmdPreprocessGeneratedCommandsEXT as *const () as usize)
        }
        "vkCmdPreprocessGeneratedCommandsNV" => {
            vkCmdPreprocessGeneratedCommandsNV::HOST.bind(address);
            Some(vkCmdPreprocessGeneratedCommandsNV as *const () as usize)
        }
        "vkCmdPushConstants2KHR" => {
            vkCmdPushConstants2KHR::HOST.bind(address);
            Some(vkCmdPushConstants2KHR as *const () as usize)
        }
        "vkCmdPushDataEXT" => {
            vkCmdPushDataEXT::HOST.bind(address);
            Some(vkCmdPushDataEXT as *const () as usize)
        }
        "vkCmdPushDescriptorSet2KHR" => {
            vkCmdPushDescriptorSet2KHR::HOST.bind(address);
            Some(vkCmdPushDescriptorSet2KHR as *const () as usize)
        }
        "vkCmdPushDescriptorSetKHR" => {
            vkCmdPushDescriptorSetKHR::HOST.bind(address);
            Some(vkCmdPushDescriptorSetKHR as *const () as usize)
        }
        "vkCmdPushDescriptorSetWithTemplate2KHR" => {
            vkCmdPushDescriptorSetWithTemplate2KHR::HOST.bind(address);
            Some(vkCmdPushDescriptorSetWithTemplate2KHR as *const () as usize)
        }
        "vkCmdPushDescriptorSetWithTemplateKHR" => {
            vkCmdPushDescriptorSetWithTemplateKHR::HOST.bind(address);
            Some(vkCmdPushDescriptorSetWithTemplateKHR as *const () as usize)
        }
        "vkCmdRefreshObjectsKHR" => {
            vkCmdRefreshObjectsKHR::HOST.bind(address);
            Some(vkCmdRefreshObjectsKHR as *const () as usize)
        }
        "vkCmdResetEvent2KHR" => {
            vkCmdResetEvent2KHR::HOST.bind(address);
            Some(vkCmdResetEvent2KHR as *const () as usize)
        }
        "vkCmdResolveImage2KHR" => {
            vkCmdResolveImage2KHR::HOST.bind(address);
            Some(vkCmdResolveImage2KHR as *const () as usize)
        }
        "vkCmdSetAlphaToCoverageEnableEXT" => {
            vkCmdSetAlphaToCoverageEnableEXT::HOST.bind(address);
            Some(vkCmdSetAlphaToCoverageEnableEXT as *const () as usize)
        }
        "vkCmdSetAlphaToOneEnableEXT" => {
            vkCmdSetAlphaToOneEnableEXT::HOST.bind(address);
            Some(vkCmdSetAlphaToOneEnableEXT as *const () as usize)
        }
        "vkCmdSetAttachmentFeedbackLoopEnableEXT" => {
            vkCmdSetAttachmentFeedbackLoopEnableEXT::HOST.bind(address);
            Some(vkCmdSetAttachmentFeedbackLoopEnableEXT as *const () as usize)
        }
        "vkCmdSetCheckpointNV" => {
            vkCmdSetCheckpointNV::HOST.bind(address);
            Some(vkCmdSetCheckpointNV as *const () as usize)
        }
        "vkCmdSetCoarseSampleOrderNV" => {
            vkCmdSetCoarseSampleOrderNV::HOST.bind(address);
            Some(vkCmdSetCoarseSampleOrderNV as *const () as usize)
        }
        "vkCmdSetColorBlendAdvancedEXT" => {
            vkCmdSetColorBlendAdvancedEXT::HOST.bind(address);
            Some(vkCmdSetColorBlendAdvancedEXT as *const () as usize)
        }
        "vkCmdSetColorBlendEnableEXT" => {
            vkCmdSetColorBlendEnableEXT::HOST.bind(address);
            Some(vkCmdSetColorBlendEnableEXT as *const () as usize)
        }
        "vkCmdSetColorBlendEquationEXT" => {
            vkCmdSetColorBlendEquationEXT::HOST.bind(address);
            Some(vkCmdSetColorBlendEquationEXT as *const () as usize)
        }
        "vkCmdSetColorWriteEnableEXT" => {
            vkCmdSetColorWriteEnableEXT::HOST.bind(address);
            Some(vkCmdSetColorWriteEnableEXT as *const () as usize)
        }
        "vkCmdSetColorWriteMaskEXT" => {
            vkCmdSetColorWriteMaskEXT::HOST.bind(address);
            Some(vkCmdSetColorWriteMaskEXT as *const () as usize)
        }
        "vkCmdSetComputeOccupancyPriorityNV" => {
            vkCmdSetComputeOccupancyPriorityNV::HOST.bind(address);
            Some(vkCmdSetComputeOccupancyPriorityNV as *const () as usize)
        }
        "vkCmdSetConservativeRasterizationModeEXT" => {
            vkCmdSetConservativeRasterizationModeEXT::HOST.bind(address);
            Some(vkCmdSetConservativeRasterizationModeEXT as *const () as usize)
        }
        "vkCmdSetCoverageModulationModeNV" => {
            vkCmdSetCoverageModulationModeNV::HOST.bind(address);
            Some(vkCmdSetCoverageModulationModeNV as *const () as usize)
        }
        "vkCmdSetCoverageModulationTableEnableNV" => {
            vkCmdSetCoverageModulationTableEnableNV::HOST.bind(address);
            Some(vkCmdSetCoverageModulationTableEnableNV as *const () as usize)
        }
        "vkCmdSetCoverageModulationTableNV" => {
            vkCmdSetCoverageModulationTableNV::HOST.bind(address);
            Some(vkCmdSetCoverageModulationTableNV as *const () as usize)
        }
        "vkCmdSetCoverageReductionModeNV" => {
            vkCmdSetCoverageReductionModeNV::HOST.bind(address);
            Some(vkCmdSetCoverageReductionModeNV as *const () as usize)
        }
        "vkCmdSetCoverageToColorEnableNV" => {
            vkCmdSetCoverageToColorEnableNV::HOST.bind(address);
            Some(vkCmdSetCoverageToColorEnableNV as *const () as usize)
        }
        "vkCmdSetCoverageToColorLocationNV" => {
            vkCmdSetCoverageToColorLocationNV::HOST.bind(address);
            Some(vkCmdSetCoverageToColorLocationNV as *const () as usize)
        }
        "vkCmdSetCullModeEXT" => {
            vkCmdSetCullModeEXT::HOST.bind(address);
            Some(vkCmdSetCullModeEXT as *const () as usize)
        }
        "vkCmdSetDepthBias2EXT" => {
            vkCmdSetDepthBias2EXT::HOST.bind(address);
            Some(vkCmdSetDepthBias2EXT as *const () as usize)
        }
        "vkCmdSetDepthBiasEnableEXT" => {
            vkCmdSetDepthBiasEnableEXT::HOST.bind(address);
            Some(vkCmdSetDepthBiasEnableEXT as *const () as usize)
        }
        "vkCmdSetDepthBoundsTestEnableEXT" => {
            vkCmdSetDepthBoundsTestEnableEXT::HOST.bind(address);
            Some(vkCmdSetDepthBoundsTestEnableEXT as *const () as usize)
        }
        "vkCmdSetDepthClampEnableEXT" => {
            vkCmdSetDepthClampEnableEXT::HOST.bind(address);
            Some(vkCmdSetDepthClampEnableEXT as *const () as usize)
        }
        "vkCmdSetDepthClampRangeEXT" => {
            vkCmdSetDepthClampRangeEXT::HOST.bind(address);
            Some(vkCmdSetDepthClampRangeEXT as *const () as usize)
        }
        "vkCmdSetDepthClipEnableEXT" => {
            vkCmdSetDepthClipEnableEXT::HOST.bind(address);
            Some(vkCmdSetDepthClipEnableEXT as *const () as usize)
        }
        "vkCmdSetDepthClipNegativeOneToOneEXT" => {
            vkCmdSetDepthClipNegativeOneToOneEXT::HOST.bind(address);
            Some(vkCmdSetDepthClipNegativeOneToOneEXT as *const () as usize)
        }
        "vkCmdSetDepthCompareOpEXT" => {
            vkCmdSetDepthCompareOpEXT::HOST.bind(address);
            Some(vkCmdSetDepthCompareOpEXT as *const () as usize)
        }
        "vkCmdSetDepthTestEnableEXT" => {
            vkCmdSetDepthTestEnableEXT::HOST.bind(address);
            Some(vkCmdSetDepthTestEnableEXT as *const () as usize)
        }
        "vkCmdSetDepthWriteEnableEXT" => {
            vkCmdSetDepthWriteEnableEXT::HOST.bind(address);
            Some(vkCmdSetDepthWriteEnableEXT as *const () as usize)
        }
        "vkCmdSetDescriptorBufferOffsets2EXT" => {
            vkCmdSetDescriptorBufferOffsets2EXT::HOST.bind(address);
            Some(vkCmdSetDescriptorBufferOffsets2EXT as *const () as usize)
        }
        "vkCmdSetDescriptorBufferOffsetsEXT" => {
            vkCmdSetDescriptorBufferOffsetsEXT::HOST.bind(address);
            Some(vkCmdSetDescriptorBufferOffsetsEXT as *const () as usize)
        }
        "vkCmdSetDeviceMaskKHR" => {
            vkCmdSetDeviceMaskKHR::HOST.bind(address);
            Some(vkCmdSetDeviceMaskKHR as *const () as usize)
        }
        "vkCmdSetDiscardRectangleEXT" => {
            vkCmdSetDiscardRectangleEXT::HOST.bind(address);
            Some(vkCmdSetDiscardRectangleEXT as *const () as usize)
        }
        "vkCmdSetDiscardRectangleEnableEXT" => {
            vkCmdSetDiscardRectangleEnableEXT::HOST.bind(address);
            Some(vkCmdSetDiscardRectangleEnableEXT as *const () as usize)
        }
        "vkCmdSetDiscardRectangleModeEXT" => {
            vkCmdSetDiscardRectangleModeEXT::HOST.bind(address);
            Some(vkCmdSetDiscardRectangleModeEXT as *const () as usize)
        }
        "vkCmdSetDispatchParametersARM" => {
            vkCmdSetDispatchParametersARM::HOST.bind(address);
            Some(vkCmdSetDispatchParametersARM as *const () as usize)
        }
        "vkCmdSetEvent2KHR" => {
            vkCmdSetEvent2KHR::HOST.bind(address);
            Some(vkCmdSetEvent2KHR as *const () as usize)
        }
        "vkCmdSetExclusiveScissorEnableNV" => {
            vkCmdSetExclusiveScissorEnableNV::HOST.bind(address);
            Some(vkCmdSetExclusiveScissorEnableNV as *const () as usize)
        }
        "vkCmdSetExclusiveScissorNV" => {
            vkCmdSetExclusiveScissorNV::HOST.bind(address);
            Some(vkCmdSetExclusiveScissorNV as *const () as usize)
        }
        "vkCmdSetExtraPrimitiveOverestimationSizeEXT" => {
            vkCmdSetExtraPrimitiveOverestimationSizeEXT::HOST.bind(address);
            Some(vkCmdSetExtraPrimitiveOverestimationSizeEXT as *const () as usize)
        }
        "vkCmdSetFragmentShadingRateEnumNV" => {
            vkCmdSetFragmentShadingRateEnumNV::HOST.bind(address);
            Some(vkCmdSetFragmentShadingRateEnumNV as *const () as usize)
        }
        "vkCmdSetFragmentShadingRateKHR" => {
            vkCmdSetFragmentShadingRateKHR::HOST.bind(address);
            Some(vkCmdSetFragmentShadingRateKHR as *const () as usize)
        }
        "vkCmdSetFrontFaceEXT" => {
            vkCmdSetFrontFaceEXT::HOST.bind(address);
            Some(vkCmdSetFrontFaceEXT as *const () as usize)
        }
        "vkCmdSetLineRasterizationModeEXT" => {
            vkCmdSetLineRasterizationModeEXT::HOST.bind(address);
            Some(vkCmdSetLineRasterizationModeEXT as *const () as usize)
        }
        "vkCmdSetLineStippleEXT" => {
            vkCmdSetLineStippleEXT::HOST.bind(address);
            Some(vkCmdSetLineStippleEXT as *const () as usize)
        }
        "vkCmdSetLineStippleEnableEXT" => {
            vkCmdSetLineStippleEnableEXT::HOST.bind(address);
            Some(vkCmdSetLineStippleEnableEXT as *const () as usize)
        }
        "vkCmdSetLineStippleKHR" => {
            vkCmdSetLineStippleKHR::HOST.bind(address);
            Some(vkCmdSetLineStippleKHR as *const () as usize)
        }
        "vkCmdSetLogicOpEXT" => {
            vkCmdSetLogicOpEXT::HOST.bind(address);
            Some(vkCmdSetLogicOpEXT as *const () as usize)
        }
        "vkCmdSetLogicOpEnableEXT" => {
            vkCmdSetLogicOpEnableEXT::HOST.bind(address);
            Some(vkCmdSetLogicOpEnableEXT as *const () as usize)
        }
        "vkCmdSetPatchControlPointsEXT" => {
            vkCmdSetPatchControlPointsEXT::HOST.bind(address);
            Some(vkCmdSetPatchControlPointsEXT as *const () as usize)
        }
        "vkCmdSetPerformanceMarkerINTEL" => {
            vkCmdSetPerformanceMarkerINTEL::HOST.bind(address);
            Some(vkCmdSetPerformanceMarkerINTEL as *const () as usize)
        }
        "vkCmdSetPerformanceOverrideINTEL" => {
            vkCmdSetPerformanceOverrideINTEL::HOST.bind(address);
            Some(vkCmdSetPerformanceOverrideINTEL as *const () as usize)
        }
        "vkCmdSetPerformanceStreamMarkerINTEL" => {
            vkCmdSetPerformanceStreamMarkerINTEL::HOST.bind(address);
            Some(vkCmdSetPerformanceStreamMarkerINTEL as *const () as usize)
        }
        "vkCmdSetPolygonModeEXT" => {
            vkCmdSetPolygonModeEXT::HOST.bind(address);
            Some(vkCmdSetPolygonModeEXT as *const () as usize)
        }
        "vkCmdSetPrimitiveRestartEnableEXT" => {
            vkCmdSetPrimitiveRestartEnableEXT::HOST.bind(address);
            Some(vkCmdSetPrimitiveRestartEnableEXT as *const () as usize)
        }
        "vkCmdSetPrimitiveRestartIndexEXT" => {
            vkCmdSetPrimitiveRestartIndexEXT::HOST.bind(address);
            Some(vkCmdSetPrimitiveRestartIndexEXT as *const () as usize)
        }
        "vkCmdSetPrimitiveTopologyEXT" => {
            vkCmdSetPrimitiveTopologyEXT::HOST.bind(address);
            Some(vkCmdSetPrimitiveTopologyEXT as *const () as usize)
        }
        "vkCmdSetProvokingVertexModeEXT" => {
            vkCmdSetProvokingVertexModeEXT::HOST.bind(address);
            Some(vkCmdSetProvokingVertexModeEXT as *const () as usize)
        }
        "vkCmdSetRasterizationSamplesEXT" => {
            vkCmdSetRasterizationSamplesEXT::HOST.bind(address);
            Some(vkCmdSetRasterizationSamplesEXT as *const () as usize)
        }
        "vkCmdSetRasterizationStreamEXT" => {
            vkCmdSetRasterizationStreamEXT::HOST.bind(address);
            Some(vkCmdSetRasterizationStreamEXT as *const () as usize)
        }
        "vkCmdSetRasterizerDiscardEnableEXT" => {
            vkCmdSetRasterizerDiscardEnableEXT::HOST.bind(address);
            Some(vkCmdSetRasterizerDiscardEnableEXT as *const () as usize)
        }
        "vkCmdSetRayTracingPipelineStackSizeKHR" => {
            vkCmdSetRayTracingPipelineStackSizeKHR::HOST.bind(address);
            Some(vkCmdSetRayTracingPipelineStackSizeKHR as *const () as usize)
        }
        "vkCmdSetRenderingAttachmentLocationsKHR" => {
            vkCmdSetRenderingAttachmentLocationsKHR::HOST.bind(address);
            Some(vkCmdSetRenderingAttachmentLocationsKHR as *const () as usize)
        }
        "vkCmdSetRenderingInputAttachmentIndicesKHR" => {
            vkCmdSetRenderingInputAttachmentIndicesKHR::HOST.bind(address);
            Some(vkCmdSetRenderingInputAttachmentIndicesKHR as *const () as usize)
        }
        "vkCmdSetRepresentativeFragmentTestEnableNV" => {
            vkCmdSetRepresentativeFragmentTestEnableNV::HOST.bind(address);
            Some(vkCmdSetRepresentativeFragmentTestEnableNV as *const () as usize)
        }
        "vkCmdSetSampleLocationsEXT" => {
            vkCmdSetSampleLocationsEXT::HOST.bind(address);
            Some(vkCmdSetSampleLocationsEXT as *const () as usize)
        }
        "vkCmdSetSampleLocationsEnableEXT" => {
            vkCmdSetSampleLocationsEnableEXT::HOST.bind(address);
            Some(vkCmdSetSampleLocationsEnableEXT as *const () as usize)
        }
        "vkCmdSetSampleMaskEXT" => {
            vkCmdSetSampleMaskEXT::HOST.bind(address);
            Some(vkCmdSetSampleMaskEXT as *const () as usize)
        }
        "vkCmdSetScissorWithCountEXT" => {
            vkCmdSetScissorWithCountEXT::HOST.bind(address);
            Some(vkCmdSetScissorWithCountEXT as *const () as usize)
        }
        "vkCmdSetShadingRateImageEnableNV" => {
            vkCmdSetShadingRateImageEnableNV::HOST.bind(address);
            Some(vkCmdSetShadingRateImageEnableNV as *const () as usize)
        }
        "vkCmdSetStencilOpEXT" => {
            vkCmdSetStencilOpEXT::HOST.bind(address);
            Some(vkCmdSetStencilOpEXT as *const () as usize)
        }
        "vkCmdSetStencilTestEnableEXT" => {
            vkCmdSetStencilTestEnableEXT::HOST.bind(address);
            Some(vkCmdSetStencilTestEnableEXT as *const () as usize)
        }
        "vkCmdSetTessellationDomainOriginEXT" => {
            vkCmdSetTessellationDomainOriginEXT::HOST.bind(address);
            Some(vkCmdSetTessellationDomainOriginEXT as *const () as usize)
        }
        "vkCmdSetVertexInputEXT" => {
            vkCmdSetVertexInputEXT::HOST.bind(address);
            Some(vkCmdSetVertexInputEXT as *const () as usize)
        }
        "vkCmdSetViewportShadingRatePaletteNV" => {
            vkCmdSetViewportShadingRatePaletteNV::HOST.bind(address);
            Some(vkCmdSetViewportShadingRatePaletteNV as *const () as usize)
        }
        "vkCmdSetViewportSwizzleNV" => {
            vkCmdSetViewportSwizzleNV::HOST.bind(address);
            Some(vkCmdSetViewportSwizzleNV as *const () as usize)
        }
        "vkCmdSetViewportWScalingEnableNV" => {
            vkCmdSetViewportWScalingEnableNV::HOST.bind(address);
            Some(vkCmdSetViewportWScalingEnableNV as *const () as usize)
        }
        "vkCmdSetViewportWScalingNV" => {
            vkCmdSetViewportWScalingNV::HOST.bind(address);
            Some(vkCmdSetViewportWScalingNV as *const () as usize)
        }
        "vkCmdSetViewportWithCountEXT" => {
            vkCmdSetViewportWithCountEXT::HOST.bind(address);
            Some(vkCmdSetViewportWithCountEXT as *const () as usize)
        }
        "vkCmdSubpassShadingHUAWEI" => {
            vkCmdSubpassShadingHUAWEI::HOST.bind(address);
            Some(vkCmdSubpassShadingHUAWEI as *const () as usize)
        }
        "vkCmdTraceRaysIndirect2KHR" => {
            vkCmdTraceRaysIndirect2KHR::HOST.bind(address);
            Some(vkCmdTraceRaysIndirect2KHR as *const () as usize)
        }
        "vkCmdTraceRaysIndirectKHR" => {
            vkCmdTraceRaysIndirectKHR::HOST.bind(address);
            Some(vkCmdTraceRaysIndirectKHR as *const () as usize)
        }
        "vkCmdTraceRaysKHR" => {
            vkCmdTraceRaysKHR::HOST.bind(address);
            Some(vkCmdTraceRaysKHR as *const () as usize)
        }
        "vkCmdTraceRaysNV" => {
            vkCmdTraceRaysNV::HOST.bind(address);
            Some(vkCmdTraceRaysNV as *const () as usize)
        }
        "vkCmdUpdateMemoryKHR" => {
            vkCmdUpdateMemoryKHR::HOST.bind(address);
            Some(vkCmdUpdateMemoryKHR as *const () as usize)
        }
        "vkCmdUpdatePipelineIndirectBufferNV" => {
            vkCmdUpdatePipelineIndirectBufferNV::HOST.bind(address);
            Some(vkCmdUpdatePipelineIndirectBufferNV as *const () as usize)
        }
        "vkCmdWaitEvents2KHR" => {
            vkCmdWaitEvents2KHR::HOST.bind(address);
            Some(vkCmdWaitEvents2KHR as *const () as usize)
        }
        "vkCmdWriteAccelerationStructuresPropertiesKHR" => {
            vkCmdWriteAccelerationStructuresPropertiesKHR::HOST.bind(address);
            Some(vkCmdWriteAccelerationStructuresPropertiesKHR as *const () as usize)
        }
        "vkCmdWriteAccelerationStructuresPropertiesNV" => {
            vkCmdWriteAccelerationStructuresPropertiesNV::HOST.bind(address);
            Some(vkCmdWriteAccelerationStructuresPropertiesNV as *const () as usize)
        }
        "vkCmdWriteBufferMarker2AMD" => {
            vkCmdWriteBufferMarker2AMD::HOST.bind(address);
            Some(vkCmdWriteBufferMarker2AMD as *const () as usize)
        }
        "vkCmdWriteBufferMarkerAMD" => {
            vkCmdWriteBufferMarkerAMD::HOST.bind(address);
            Some(vkCmdWriteBufferMarkerAMD as *const () as usize)
        }
        "vkCmdWriteMarkerToMemoryAMD" => {
            vkCmdWriteMarkerToMemoryAMD::HOST.bind(address);
            Some(vkCmdWriteMarkerToMemoryAMD as *const () as usize)
        }
        "vkCmdWriteMicromapsPropertiesEXT" => {
            vkCmdWriteMicromapsPropertiesEXT::HOST.bind(address);
            Some(vkCmdWriteMicromapsPropertiesEXT as *const () as usize)
        }
        "vkCmdWriteTimestamp2KHR" => {
            vkCmdWriteTimestamp2KHR::HOST.bind(address);
            Some(vkCmdWriteTimestamp2KHR as *const () as usize)
        }
        "vkCompileDeferredNV" => {
            vkCompileDeferredNV::HOST.bind(address);
            Some(vkCompileDeferredNV as *const () as usize)
        }
        "vkConvertCooperativeVectorMatrixNV" => {
            vkConvertCooperativeVectorMatrixNV::HOST.bind(address);
            Some(vkConvertCooperativeVectorMatrixNV as *const () as usize)
        }
        "vkCopyAccelerationStructureKHR" => {
            vkCopyAccelerationStructureKHR::HOST.bind(address);
            Some(vkCopyAccelerationStructureKHR as *const () as usize)
        }
        "vkCopyAccelerationStructureToMemoryKHR" => {
            vkCopyAccelerationStructureToMemoryKHR::HOST.bind(address);
            Some(vkCopyAccelerationStructureToMemoryKHR as *const () as usize)
        }
        "vkCopyImageToImageEXT" => {
            vkCopyImageToImageEXT::HOST.bind(address);
            Some(vkCopyImageToImageEXT as *const () as usize)
        }
        "vkCopyImageToMemoryEXT" => {
            vkCopyImageToMemoryEXT::HOST.bind(address);
            Some(vkCopyImageToMemoryEXT as *const () as usize)
        }
        "vkCopyMemoryToAccelerationStructureKHR" => {
            vkCopyMemoryToAccelerationStructureKHR::HOST.bind(address);
            Some(vkCopyMemoryToAccelerationStructureKHR as *const () as usize)
        }
        "vkCopyMemoryToImageEXT" => {
            vkCopyMemoryToImageEXT::HOST.bind(address);
            Some(vkCopyMemoryToImageEXT as *const () as usize)
        }
        "vkCopyMemoryToMicromapEXT" => {
            vkCopyMemoryToMicromapEXT::HOST.bind(address);
            Some(vkCopyMemoryToMicromapEXT as *const () as usize)
        }
        "vkCopyMicromapEXT" => {
            vkCopyMicromapEXT::HOST.bind(address);
            Some(vkCopyMicromapEXT as *const () as usize)
        }
        "vkCopyMicromapToMemoryEXT" => {
            vkCopyMicromapToMemoryEXT::HOST.bind(address);
            Some(vkCopyMicromapToMemoryEXT as *const () as usize)
        }
        "vkCreateAccelerationStructure2KHR" => {
            vkCreateAccelerationStructure2KHR::HOST.bind(address);
            Some(vkCreateAccelerationStructure2KHR as *const () as usize)
        }
        "vkCreateAccelerationStructureKHR" => {
            vkCreateAccelerationStructureKHR::HOST.bind(address);
            Some(vkCreateAccelerationStructureKHR as *const () as usize)
        }
        "vkCreateAccelerationStructureNV" => {
            vkCreateAccelerationStructureNV::HOST.bind(address);
            Some(vkCreateAccelerationStructureNV as *const () as usize)
        }
        "vkCreateAndroidSurfaceKHR" => {
            vkCreateAndroidSurfaceKHR::HOST.bind(address);
            Some(vkCreateAndroidSurfaceKHR as *const () as usize)
        }
        "vkCreateBufferCollectionFUCHSIA" => {
            vkCreateBufferCollectionFUCHSIA::HOST.bind(address);
            Some(vkCreateBufferCollectionFUCHSIA as *const () as usize)
        }
        "vkCreateCuFunctionNVX" => {
            vkCreateCuFunctionNVX::HOST.bind(address);
            Some(vkCreateCuFunctionNVX as *const () as usize)
        }
        "vkCreateCuModuleNVX" => {
            vkCreateCuModuleNVX::HOST.bind(address);
            Some(vkCreateCuModuleNVX as *const () as usize)
        }
        "vkCreateCudaFunctionNV" => {
            vkCreateCudaFunctionNV::HOST.bind(address);
            Some(vkCreateCudaFunctionNV as *const () as usize)
        }
        "vkCreateCudaModuleNV" => {
            vkCreateCudaModuleNV::HOST.bind(address);
            Some(vkCreateCudaModuleNV as *const () as usize)
        }
        "vkCreateDataGraphPipelineSessionARM" => {
            vkCreateDataGraphPipelineSessionARM::HOST.bind(address);
            Some(vkCreateDataGraphPipelineSessionARM as *const () as usize)
        }
        "vkCreateDataGraphPipelinesARM" => {
            vkCreateDataGraphPipelinesARM::HOST.bind(address);
            Some(vkCreateDataGraphPipelinesARM as *const () as usize)
        }
        "vkCreateDeferredOperationKHR" => {
            vkCreateDeferredOperationKHR::HOST.bind(address);
            Some(vkCreateDeferredOperationKHR as *const () as usize)
        }
        "vkCreateDescriptorUpdateTemplateKHR" => {
            vkCreateDescriptorUpdateTemplateKHR::HOST.bind(address);
            Some(vkCreateDescriptorUpdateTemplateKHR as *const () as usize)
        }
        "vkCreateDirectFBSurfaceEXT" => {
            vkCreateDirectFBSurfaceEXT::HOST.bind(address);
            Some(vkCreateDirectFBSurfaceEXT as *const () as usize)
        }
        "vkCreateExecutionGraphPipelinesAMDX" => {
            vkCreateExecutionGraphPipelinesAMDX::HOST.bind(address);
            Some(vkCreateExecutionGraphPipelinesAMDX as *const () as usize)
        }
        "vkCreateExternalComputeQueueNV" => {
            vkCreateExternalComputeQueueNV::HOST.bind(address);
            Some(vkCreateExternalComputeQueueNV as *const () as usize)
        }
        "vkCreateGpaSessionAMD" => {
            vkCreateGpaSessionAMD::HOST.bind(address);
            Some(vkCreateGpaSessionAMD as *const () as usize)
        }
        "vkCreateIOSSurfaceMVK" => {
            vkCreateIOSSurfaceMVK::HOST.bind(address);
            Some(vkCreateIOSSurfaceMVK as *const () as usize)
        }
        "vkCreateImagePipeSurfaceFUCHSIA" => {
            vkCreateImagePipeSurfaceFUCHSIA::HOST.bind(address);
            Some(vkCreateImagePipeSurfaceFUCHSIA as *const () as usize)
        }
        "vkCreateIndirectCommandsLayoutEXT" => {
            vkCreateIndirectCommandsLayoutEXT::HOST.bind(address);
            Some(vkCreateIndirectCommandsLayoutEXT as *const () as usize)
        }
        "vkCreateIndirectCommandsLayoutNV" => {
            vkCreateIndirectCommandsLayoutNV::HOST.bind(address);
            Some(vkCreateIndirectCommandsLayoutNV as *const () as usize)
        }
        "vkCreateIndirectExecutionSetEXT" => {
            vkCreateIndirectExecutionSetEXT::HOST.bind(address);
            Some(vkCreateIndirectExecutionSetEXT as *const () as usize)
        }
        "vkCreateMacOSSurfaceMVK" => {
            vkCreateMacOSSurfaceMVK::HOST.bind(address);
            Some(vkCreateMacOSSurfaceMVK as *const () as usize)
        }
        "vkCreateMetalSurfaceEXT" => {
            vkCreateMetalSurfaceEXT::HOST.bind(address);
            Some(vkCreateMetalSurfaceEXT as *const () as usize)
        }
        "vkCreateMicromapEXT" => {
            vkCreateMicromapEXT::HOST.bind(address);
            Some(vkCreateMicromapEXT as *const () as usize)
        }
        "vkCreateOpticalFlowSessionNV" => {
            vkCreateOpticalFlowSessionNV::HOST.bind(address);
            Some(vkCreateOpticalFlowSessionNV as *const () as usize)
        }
        "vkCreatePipelineBinariesKHR" => {
            vkCreatePipelineBinariesKHR::HOST.bind(address);
            Some(vkCreatePipelineBinariesKHR as *const () as usize)
        }
        "vkCreatePrivateDataSlotEXT" => {
            vkCreatePrivateDataSlotEXT::HOST.bind(address);
            Some(vkCreatePrivateDataSlotEXT as *const () as usize)
        }
        "vkCreateRayTracingPipelinesKHR" => {
            vkCreateRayTracingPipelinesKHR::HOST.bind(address);
            Some(vkCreateRayTracingPipelinesKHR as *const () as usize)
        }
        "vkCreateRayTracingPipelinesNV" => {
            vkCreateRayTracingPipelinesNV::HOST.bind(address);
            Some(vkCreateRayTracingPipelinesNV as *const () as usize)
        }
        "vkCreateRenderPass2KHR" => {
            vkCreateRenderPass2KHR::HOST.bind(address);
            Some(vkCreateRenderPass2KHR as *const () as usize)
        }
        "vkCreateSamplerYcbcrConversionKHR" => {
            vkCreateSamplerYcbcrConversionKHR::HOST.bind(address);
            Some(vkCreateSamplerYcbcrConversionKHR as *const () as usize)
        }
        "vkCreateScreenSurfaceQNX" => {
            vkCreateScreenSurfaceQNX::HOST.bind(address);
            Some(vkCreateScreenSurfaceQNX as *const () as usize)
        }
        "vkCreateSemaphoreSciSyncPoolNV" => {
            vkCreateSemaphoreSciSyncPoolNV::HOST.bind(address);
            Some(vkCreateSemaphoreSciSyncPoolNV as *const () as usize)
        }
        "vkCreateShaderInstrumentationARM" => {
            vkCreateShaderInstrumentationARM::HOST.bind(address);
            Some(vkCreateShaderInstrumentationARM as *const () as usize)
        }
        "vkCreateShadersEXT" => {
            vkCreateShadersEXT::HOST.bind(address);
            Some(vkCreateShadersEXT as *const () as usize)
        }
        "vkCreateStreamDescriptorSurfaceGGP" => {
            vkCreateStreamDescriptorSurfaceGGP::HOST.bind(address);
            Some(vkCreateStreamDescriptorSurfaceGGP as *const () as usize)
        }
        "vkCreateSurfaceOHOS" => {
            vkCreateSurfaceOHOS::HOST.bind(address);
            Some(vkCreateSurfaceOHOS as *const () as usize)
        }
        "vkCreateTensorARM" => {
            vkCreateTensorARM::HOST.bind(address);
            Some(vkCreateTensorARM as *const () as usize)
        }
        "vkCreateTensorViewARM" => {
            vkCreateTensorViewARM::HOST.bind(address);
            Some(vkCreateTensorViewARM as *const () as usize)
        }
        "vkCreateUbmSurfaceSEC" => {
            vkCreateUbmSurfaceSEC::HOST.bind(address);
            Some(vkCreateUbmSurfaceSEC as *const () as usize)
        }
        "vkCreateValidationCacheEXT" => {
            vkCreateValidationCacheEXT::HOST.bind(address);
            Some(vkCreateValidationCacheEXT as *const () as usize)
        }
        "vkCreateViSurfaceNN" => {
            vkCreateViSurfaceNN::HOST.bind(address);
            Some(vkCreateViSurfaceNN as *const () as usize)
        }
        "vkCreateVideoSessionKHR" => {
            vkCreateVideoSessionKHR::HOST.bind(address);
            Some(vkCreateVideoSessionKHR as *const () as usize)
        }
        "vkCreateVideoSessionParametersKHR" => {
            vkCreateVideoSessionParametersKHR::HOST.bind(address);
            Some(vkCreateVideoSessionParametersKHR as *const () as usize)
        }
        "vkDebugMarkerSetObjectNameEXT" => {
            vkDebugMarkerSetObjectNameEXT::HOST.bind(address);
            Some(vkDebugMarkerSetObjectNameEXT as *const () as usize)
        }
        "vkDebugMarkerSetObjectTagEXT" => {
            vkDebugMarkerSetObjectTagEXT::HOST.bind(address);
            Some(vkDebugMarkerSetObjectTagEXT as *const () as usize)
        }
        "vkDebugReportMessageEXT" => {
            vkDebugReportMessageEXT::HOST.bind(address);
            Some(vkDebugReportMessageEXT as *const () as usize)
        }
        "vkDeferredOperationJoinKHR" => {
            vkDeferredOperationJoinKHR::HOST.bind(address);
            Some(vkDeferredOperationJoinKHR as *const () as usize)
        }
        "vkDestroyAccelerationStructureKHR" => {
            vkDestroyAccelerationStructureKHR::HOST.bind(address);
            Some(vkDestroyAccelerationStructureKHR as *const () as usize)
        }
        "vkDestroyAccelerationStructureNV" => {
            vkDestroyAccelerationStructureNV::HOST.bind(address);
            Some(vkDestroyAccelerationStructureNV as *const () as usize)
        }
        "vkDestroyBufferCollectionFUCHSIA" => {
            vkDestroyBufferCollectionFUCHSIA::HOST.bind(address);
            Some(vkDestroyBufferCollectionFUCHSIA as *const () as usize)
        }
        "vkDestroyCuFunctionNVX" => {
            vkDestroyCuFunctionNVX::HOST.bind(address);
            Some(vkDestroyCuFunctionNVX as *const () as usize)
        }
        "vkDestroyCuModuleNVX" => {
            vkDestroyCuModuleNVX::HOST.bind(address);
            Some(vkDestroyCuModuleNVX as *const () as usize)
        }
        "vkDestroyCudaFunctionNV" => {
            vkDestroyCudaFunctionNV::HOST.bind(address);
            Some(vkDestroyCudaFunctionNV as *const () as usize)
        }
        "vkDestroyCudaModuleNV" => {
            vkDestroyCudaModuleNV::HOST.bind(address);
            Some(vkDestroyCudaModuleNV as *const () as usize)
        }
        "vkDestroyDataGraphPipelineSessionARM" => {
            vkDestroyDataGraphPipelineSessionARM::HOST.bind(address);
            Some(vkDestroyDataGraphPipelineSessionARM as *const () as usize)
        }
        "vkDestroyDebugReportCallbackEXT" => {
            vkDestroyDebugReportCallbackEXT::HOST.bind(address);
            Some(vkDestroyDebugReportCallbackEXT as *const () as usize)
        }
        "vkDestroyDebugUtilsMessengerEXT" => {
            vkDestroyDebugUtilsMessengerEXT::HOST.bind(address);
            Some(vkDestroyDebugUtilsMessengerEXT as *const () as usize)
        }
        "vkDestroyDeferredOperationKHR" => {
            vkDestroyDeferredOperationKHR::HOST.bind(address);
            Some(vkDestroyDeferredOperationKHR as *const () as usize)
        }
        "vkDestroyDescriptorUpdateTemplateKHR" => {
            vkDestroyDescriptorUpdateTemplateKHR::HOST.bind(address);
            Some(vkDestroyDescriptorUpdateTemplateKHR as *const () as usize)
        }
        "vkDestroyExternalComputeQueueNV" => {
            vkDestroyExternalComputeQueueNV::HOST.bind(address);
            Some(vkDestroyExternalComputeQueueNV as *const () as usize)
        }
        "vkDestroyGpaSessionAMD" => {
            vkDestroyGpaSessionAMD::HOST.bind(address);
            Some(vkDestroyGpaSessionAMD as *const () as usize)
        }
        "vkDestroyIndirectCommandsLayoutEXT" => {
            vkDestroyIndirectCommandsLayoutEXT::HOST.bind(address);
            Some(vkDestroyIndirectCommandsLayoutEXT as *const () as usize)
        }
        "vkDestroyIndirectCommandsLayoutNV" => {
            vkDestroyIndirectCommandsLayoutNV::HOST.bind(address);
            Some(vkDestroyIndirectCommandsLayoutNV as *const () as usize)
        }
        "vkDestroyIndirectExecutionSetEXT" => {
            vkDestroyIndirectExecutionSetEXT::HOST.bind(address);
            Some(vkDestroyIndirectExecutionSetEXT as *const () as usize)
        }
        "vkDestroyMicromapEXT" => {
            vkDestroyMicromapEXT::HOST.bind(address);
            Some(vkDestroyMicromapEXT as *const () as usize)
        }
        "vkDestroyOpticalFlowSessionNV" => {
            vkDestroyOpticalFlowSessionNV::HOST.bind(address);
            Some(vkDestroyOpticalFlowSessionNV as *const () as usize)
        }
        "vkDestroyPipelineBinaryKHR" => {
            vkDestroyPipelineBinaryKHR::HOST.bind(address);
            Some(vkDestroyPipelineBinaryKHR as *const () as usize)
        }
        "vkDestroyPrivateDataSlotEXT" => {
            vkDestroyPrivateDataSlotEXT::HOST.bind(address);
            Some(vkDestroyPrivateDataSlotEXT as *const () as usize)
        }
        "vkDestroySamplerYcbcrConversionKHR" => {
            vkDestroySamplerYcbcrConversionKHR::HOST.bind(address);
            Some(vkDestroySamplerYcbcrConversionKHR as *const () as usize)
        }
        "vkDestroySemaphoreSciSyncPoolNV" => {
            vkDestroySemaphoreSciSyncPoolNV::HOST.bind(address);
            Some(vkDestroySemaphoreSciSyncPoolNV as *const () as usize)
        }
        "vkDestroyShaderEXT" => {
            vkDestroyShaderEXT::HOST.bind(address);
            Some(vkDestroyShaderEXT as *const () as usize)
        }
        "vkDestroyShaderInstrumentationARM" => {
            vkDestroyShaderInstrumentationARM::HOST.bind(address);
            Some(vkDestroyShaderInstrumentationARM as *const () as usize)
        }
        "vkDestroyTensorARM" => {
            vkDestroyTensorARM::HOST.bind(address);
            Some(vkDestroyTensorARM as *const () as usize)
        }
        "vkDestroyTensorViewARM" => {
            vkDestroyTensorViewARM::HOST.bind(address);
            Some(vkDestroyTensorViewARM as *const () as usize)
        }
        "vkDestroyValidationCacheEXT" => {
            vkDestroyValidationCacheEXT::HOST.bind(address);
            Some(vkDestroyValidationCacheEXT as *const () as usize)
        }
        "vkDestroyVideoSessionKHR" => {
            vkDestroyVideoSessionKHR::HOST.bind(address);
            Some(vkDestroyVideoSessionKHR as *const () as usize)
        }
        "vkDestroyVideoSessionParametersKHR" => {
            vkDestroyVideoSessionParametersKHR::HOST.bind(address);
            Some(vkDestroyVideoSessionParametersKHR as *const () as usize)
        }
        "vkDisplayPowerControlEXT" => {
            vkDisplayPowerControlEXT::HOST.bind(address);
            Some(vkDisplayPowerControlEXT as *const () as usize)
        }
        "vkEnumeratePhysicalDeviceGroupsKHR" => {
            vkEnumeratePhysicalDeviceGroupsKHR::HOST.bind(address);
            Some(vkEnumeratePhysicalDeviceGroupsKHR as *const () as usize)
        }
        "vkEnumeratePhysicalDeviceQueueFamilyPerformanceCountersByRegionARM" => {
            vkEnumeratePhysicalDeviceQueueFamilyPerformanceCountersByRegionARM::HOST.bind(address);
            Some(
                vkEnumeratePhysicalDeviceQueueFamilyPerformanceCountersByRegionARM as *const ()
                    as usize,
            )
        }
        "vkEnumeratePhysicalDeviceQueueFamilyPerformanceQueryCountersKHR" => {
            vkEnumeratePhysicalDeviceQueueFamilyPerformanceQueryCountersKHR::HOST.bind(address);
            Some(
                vkEnumeratePhysicalDeviceQueueFamilyPerformanceQueryCountersKHR as *const ()
                    as usize,
            )
        }
        "vkEnumeratePhysicalDeviceShaderInstrumentationMetricsARM" => {
            vkEnumeratePhysicalDeviceShaderInstrumentationMetricsARM::HOST.bind(address);
            Some(vkEnumeratePhysicalDeviceShaderInstrumentationMetricsARM as *const () as usize)
        }
        "vkExportMetalObjectsEXT" => {
            vkExportMetalObjectsEXT::HOST.bind(address);
            Some(vkExportMetalObjectsEXT as *const () as usize)
        }
        "vkGetAccelerationStructureBuildSizesKHR" => {
            vkGetAccelerationStructureBuildSizesKHR::HOST.bind(address);
            Some(vkGetAccelerationStructureBuildSizesKHR as *const () as usize)
        }
        "vkGetAccelerationStructureDeviceAddressKHR" => {
            vkGetAccelerationStructureDeviceAddressKHR::HOST.bind(address);
            Some(vkGetAccelerationStructureDeviceAddressKHR as *const () as usize)
        }
        "vkGetAccelerationStructureHandleNV" => {
            vkGetAccelerationStructureHandleNV::HOST.bind(address);
            Some(vkGetAccelerationStructureHandleNV as *const () as usize)
        }
        "vkGetAccelerationStructureMemoryRequirementsNV" => {
            vkGetAccelerationStructureMemoryRequirementsNV::HOST.bind(address);
            Some(vkGetAccelerationStructureMemoryRequirementsNV as *const () as usize)
        }
        "vkGetAccelerationStructureOpaqueCaptureDescriptorDataEXT" => {
            vkGetAccelerationStructureOpaqueCaptureDescriptorDataEXT::HOST.bind(address);
            Some(vkGetAccelerationStructureOpaqueCaptureDescriptorDataEXT as *const () as usize)
        }
        "vkGetAndroidHardwareBufferPropertiesANDROID" => {
            vkGetAndroidHardwareBufferPropertiesANDROID::HOST.bind(address);
            Some(vkGetAndroidHardwareBufferPropertiesANDROID as *const () as usize)
        }
        "vkGetBufferCollectionPropertiesFUCHSIA" => {
            vkGetBufferCollectionPropertiesFUCHSIA::HOST.bind(address);
            Some(vkGetBufferCollectionPropertiesFUCHSIA as *const () as usize)
        }
        "vkGetBufferDeviceAddressEXT" => {
            vkGetBufferDeviceAddressEXT::HOST.bind(address);
            Some(vkGetBufferDeviceAddressEXT as *const () as usize)
        }
        "vkGetBufferDeviceAddressKHR" => {
            vkGetBufferDeviceAddressKHR::HOST.bind(address);
            Some(vkGetBufferDeviceAddressKHR as *const () as usize)
        }
        "vkGetBufferMemoryRequirements2KHR" => {
            vkGetBufferMemoryRequirements2KHR::HOST.bind(address);
            Some(vkGetBufferMemoryRequirements2KHR as *const () as usize)
        }
        "vkGetBufferOpaqueCaptureAddressKHR" => {
            vkGetBufferOpaqueCaptureAddressKHR::HOST.bind(address);
            Some(vkGetBufferOpaqueCaptureAddressKHR as *const () as usize)
        }
        "vkGetBufferOpaqueCaptureDescriptorDataEXT" => {
            vkGetBufferOpaqueCaptureDescriptorDataEXT::HOST.bind(address);
            Some(vkGetBufferOpaqueCaptureDescriptorDataEXT as *const () as usize)
        }
        "vkGetCalibratedTimestampsEXT" => {
            vkGetCalibratedTimestampsEXT::HOST.bind(address);
            Some(vkGetCalibratedTimestampsEXT as *const () as usize)
        }
        "vkGetCalibratedTimestampsKHR" => {
            vkGetCalibratedTimestampsKHR::HOST.bind(address);
            Some(vkGetCalibratedTimestampsKHR as *const () as usize)
        }
        "vkGetClusterAccelerationStructureBuildSizesNV" => {
            vkGetClusterAccelerationStructureBuildSizesNV::HOST.bind(address);
            Some(vkGetClusterAccelerationStructureBuildSizesNV as *const () as usize)
        }
        "vkGetCommandPoolMemoryConsumption" => {
            vkGetCommandPoolMemoryConsumption::HOST.bind(address);
            Some(vkGetCommandPoolMemoryConsumption as *const () as usize)
        }
        "vkGetCudaModuleCacheNV" => {
            vkGetCudaModuleCacheNV::HOST.bind(address);
            Some(vkGetCudaModuleCacheNV as *const () as usize)
        }
        "vkGetDataGraphPipelineAvailablePropertiesARM" => {
            vkGetDataGraphPipelineAvailablePropertiesARM::HOST.bind(address);
            Some(vkGetDataGraphPipelineAvailablePropertiesARM as *const () as usize)
        }
        "vkGetDataGraphPipelinePropertiesARM" => {
            vkGetDataGraphPipelinePropertiesARM::HOST.bind(address);
            Some(vkGetDataGraphPipelinePropertiesARM as *const () as usize)
        }
        "vkGetDataGraphPipelineSessionBindPointRequirementsARM" => {
            vkGetDataGraphPipelineSessionBindPointRequirementsARM::HOST.bind(address);
            Some(vkGetDataGraphPipelineSessionBindPointRequirementsARM as *const () as usize)
        }
        "vkGetDataGraphPipelineSessionMemoryRequirementsARM" => {
            vkGetDataGraphPipelineSessionMemoryRequirementsARM::HOST.bind(address);
            Some(vkGetDataGraphPipelineSessionMemoryRequirementsARM as *const () as usize)
        }
        "vkGetDeferredOperationMaxConcurrencyKHR" => {
            vkGetDeferredOperationMaxConcurrencyKHR::HOST.bind(address);
            Some(vkGetDeferredOperationMaxConcurrencyKHR as *const () as usize)
        }
        "vkGetDeferredOperationResultKHR" => {
            vkGetDeferredOperationResultKHR::HOST.bind(address);
            Some(vkGetDeferredOperationResultKHR as *const () as usize)
        }
        "vkGetDescriptorEXT" => {
            vkGetDescriptorEXT::HOST.bind(address);
            Some(vkGetDescriptorEXT as *const () as usize)
        }
        "vkGetDescriptorSetHostMappingVALVE" => {
            vkGetDescriptorSetHostMappingVALVE::HOST.bind(address);
            Some(vkGetDescriptorSetHostMappingVALVE as *const () as usize)
        }
        "vkGetDescriptorSetLayoutBindingOffsetEXT" => {
            vkGetDescriptorSetLayoutBindingOffsetEXT::HOST.bind(address);
            Some(vkGetDescriptorSetLayoutBindingOffsetEXT as *const () as usize)
        }
        "vkGetDescriptorSetLayoutHostMappingInfoVALVE" => {
            vkGetDescriptorSetLayoutHostMappingInfoVALVE::HOST.bind(address);
            Some(vkGetDescriptorSetLayoutHostMappingInfoVALVE as *const () as usize)
        }
        "vkGetDescriptorSetLayoutSizeEXT" => {
            vkGetDescriptorSetLayoutSizeEXT::HOST.bind(address);
            Some(vkGetDescriptorSetLayoutSizeEXT as *const () as usize)
        }
        "vkGetDescriptorSetLayoutSupportKHR" => {
            vkGetDescriptorSetLayoutSupportKHR::HOST.bind(address);
            Some(vkGetDescriptorSetLayoutSupportKHR as *const () as usize)
        }
        "vkGetDeviceAccelerationStructureCompatibilityKHR" => {
            vkGetDeviceAccelerationStructureCompatibilityKHR::HOST.bind(address);
            Some(vkGetDeviceAccelerationStructureCompatibilityKHR as *const () as usize)
        }
        "vkGetDeviceBufferMemoryRequirementsKHR" => {
            vkGetDeviceBufferMemoryRequirementsKHR::HOST.bind(address);
            Some(vkGetDeviceBufferMemoryRequirementsKHR as *const () as usize)
        }
        "vkGetDeviceCombinedImageSamplerIndexNVX" => {
            vkGetDeviceCombinedImageSamplerIndexNVX::HOST.bind(address);
            Some(vkGetDeviceCombinedImageSamplerIndexNVX as *const () as usize)
        }
        "vkGetDeviceFaultDebugInfoKHR" => {
            vkGetDeviceFaultDebugInfoKHR::HOST.bind(address);
            Some(vkGetDeviceFaultDebugInfoKHR as *const () as usize)
        }
        "vkGetDeviceFaultInfoEXT" => {
            vkGetDeviceFaultInfoEXT::HOST.bind(address);
            Some(vkGetDeviceFaultInfoEXT as *const () as usize)
        }
        "vkGetDeviceFaultReportsKHR" => {
            vkGetDeviceFaultReportsKHR::HOST.bind(address);
            Some(vkGetDeviceFaultReportsKHR as *const () as usize)
        }
        "vkGetDeviceGroupPeerMemoryFeaturesKHR" => {
            vkGetDeviceGroupPeerMemoryFeaturesKHR::HOST.bind(address);
            Some(vkGetDeviceGroupPeerMemoryFeaturesKHR as *const () as usize)
        }
        "vkGetDeviceGroupSurfacePresentModes2EXT" => {
            vkGetDeviceGroupSurfacePresentModes2EXT::HOST.bind(address);
            Some(vkGetDeviceGroupSurfacePresentModes2EXT as *const () as usize)
        }
        "vkGetDeviceImageMemoryRequirementsKHR" => {
            vkGetDeviceImageMemoryRequirementsKHR::HOST.bind(address);
            Some(vkGetDeviceImageMemoryRequirementsKHR as *const () as usize)
        }
        "vkGetDeviceImageSparseMemoryRequirementsKHR" => {
            vkGetDeviceImageSparseMemoryRequirementsKHR::HOST.bind(address);
            Some(vkGetDeviceImageSparseMemoryRequirementsKHR as *const () as usize)
        }
        "vkGetDeviceImageSubresourceLayoutKHR" => {
            vkGetDeviceImageSubresourceLayoutKHR::HOST.bind(address);
            Some(vkGetDeviceImageSubresourceLayoutKHR as *const () as usize)
        }
        "vkGetDeviceMemoryOpaqueCaptureAddressKHR" => {
            vkGetDeviceMemoryOpaqueCaptureAddressKHR::HOST.bind(address);
            Some(vkGetDeviceMemoryOpaqueCaptureAddressKHR as *const () as usize)
        }
        "vkGetDeviceMicromapCompatibilityEXT" => {
            vkGetDeviceMicromapCompatibilityEXT::HOST.bind(address);
            Some(vkGetDeviceMicromapCompatibilityEXT as *const () as usize)
        }
        "vkGetDeviceSubpassShadingMaxWorkgroupSizeHUAWEI" => {
            vkGetDeviceSubpassShadingMaxWorkgroupSizeHUAWEI::HOST.bind(address);
            Some(vkGetDeviceSubpassShadingMaxWorkgroupSizeHUAWEI as *const () as usize)
        }
        "vkGetDeviceTensorMemoryRequirementsARM" => {
            vkGetDeviceTensorMemoryRequirementsARM::HOST.bind(address);
            Some(vkGetDeviceTensorMemoryRequirementsARM as *const () as usize)
        }
        "vkGetDrmDisplayEXT" => {
            vkGetDrmDisplayEXT::HOST.bind(address);
            Some(vkGetDrmDisplayEXT as *const () as usize)
        }
        "vkGetDynamicRenderingTilePropertiesQCOM" => {
            vkGetDynamicRenderingTilePropertiesQCOM::HOST.bind(address);
            Some(vkGetDynamicRenderingTilePropertiesQCOM as *const () as usize)
        }
        "vkGetEncodedVideoSessionParametersKHR" => {
            vkGetEncodedVideoSessionParametersKHR::HOST.bind(address);
            Some(vkGetEncodedVideoSessionParametersKHR as *const () as usize)
        }
        "vkGetExecutionGraphPipelineNodeIndexAMDX" => {
            vkGetExecutionGraphPipelineNodeIndexAMDX::HOST.bind(address);
            Some(vkGetExecutionGraphPipelineNodeIndexAMDX as *const () as usize)
        }
        "vkGetExecutionGraphPipelineScratchSizeAMDX" => {
            vkGetExecutionGraphPipelineScratchSizeAMDX::HOST.bind(address);
            Some(vkGetExecutionGraphPipelineScratchSizeAMDX as *const () as usize)
        }
        "vkGetExternalComputeQueueDataNV" => {
            vkGetExternalComputeQueueDataNV::HOST.bind(address);
            Some(vkGetExternalComputeQueueDataNV as *const () as usize)
        }
        "vkGetFaultData" => {
            vkGetFaultData::HOST.bind(address);
            Some(vkGetFaultData as *const () as usize)
        }
        "vkGetFenceFdKHR" => {
            vkGetFenceFdKHR::HOST.bind(address);
            Some(vkGetFenceFdKHR as *const () as usize)
        }
        "vkGetFenceSciSyncFenceNV" => {
            vkGetFenceSciSyncFenceNV::HOST.bind(address);
            Some(vkGetFenceSciSyncFenceNV as *const () as usize)
        }
        "vkGetFenceSciSyncObjNV" => {
            vkGetFenceSciSyncObjNV::HOST.bind(address);
            Some(vkGetFenceSciSyncObjNV as *const () as usize)
        }
        "vkGetFenceWin32HandleKHR" => {
            vkGetFenceWin32HandleKHR::HOST.bind(address);
            Some(vkGetFenceWin32HandleKHR as *const () as usize)
        }
        "vkGetFramebufferTilePropertiesQCOM" => {
            vkGetFramebufferTilePropertiesQCOM::HOST.bind(address);
            Some(vkGetFramebufferTilePropertiesQCOM as *const () as usize)
        }
        "vkGetGeneratedCommandsMemoryRequirementsEXT" => {
            vkGetGeneratedCommandsMemoryRequirementsEXT::HOST.bind(address);
            Some(vkGetGeneratedCommandsMemoryRequirementsEXT as *const () as usize)
        }
        "vkGetGeneratedCommandsMemoryRequirementsNV" => {
            vkGetGeneratedCommandsMemoryRequirementsNV::HOST.bind(address);
            Some(vkGetGeneratedCommandsMemoryRequirementsNV as *const () as usize)
        }
        "vkGetGpaDeviceClockInfoAMD" => {
            vkGetGpaDeviceClockInfoAMD::HOST.bind(address);
            Some(vkGetGpaDeviceClockInfoAMD as *const () as usize)
        }
        "vkGetGpaSessionResultsAMD" => {
            vkGetGpaSessionResultsAMD::HOST.bind(address);
            Some(vkGetGpaSessionResultsAMD as *const () as usize)
        }
        "vkGetGpaSessionStatusAMD" => {
            vkGetGpaSessionStatusAMD::HOST.bind(address);
            Some(vkGetGpaSessionStatusAMD as *const () as usize)
        }
        "vkGetImageDrmFormatModifierPropertiesEXT" => {
            vkGetImageDrmFormatModifierPropertiesEXT::HOST.bind(address);
            Some(vkGetImageDrmFormatModifierPropertiesEXT as *const () as usize)
        }
        "vkGetImageMemoryRequirements2KHR" => {
            vkGetImageMemoryRequirements2KHR::HOST.bind(address);
            Some(vkGetImageMemoryRequirements2KHR as *const () as usize)
        }
        "vkGetImageOpaqueCaptureDataEXT" => {
            vkGetImageOpaqueCaptureDataEXT::HOST.bind(address);
            Some(vkGetImageOpaqueCaptureDataEXT as *const () as usize)
        }
        "vkGetImageOpaqueCaptureDescriptorDataEXT" => {
            vkGetImageOpaqueCaptureDescriptorDataEXT::HOST.bind(address);
            Some(vkGetImageOpaqueCaptureDescriptorDataEXT as *const () as usize)
        }
        "vkGetImageSparseMemoryRequirements2KHR" => {
            vkGetImageSparseMemoryRequirements2KHR::HOST.bind(address);
            Some(vkGetImageSparseMemoryRequirements2KHR as *const () as usize)
        }
        "vkGetImageSubresourceLayout2EXT" => {
            vkGetImageSubresourceLayout2EXT::HOST.bind(address);
            Some(vkGetImageSubresourceLayout2EXT as *const () as usize)
        }
        "vkGetImageSubresourceLayout2KHR" => {
            vkGetImageSubresourceLayout2KHR::HOST.bind(address);
            Some(vkGetImageSubresourceLayout2KHR as *const () as usize)
        }
        "vkGetImageViewAddressNVX" => {
            vkGetImageViewAddressNVX::HOST.bind(address);
            Some(vkGetImageViewAddressNVX as *const () as usize)
        }
        "vkGetImageViewHandle64NVX" => {
            vkGetImageViewHandle64NVX::HOST.bind(address);
            Some(vkGetImageViewHandle64NVX as *const () as usize)
        }
        "vkGetImageViewHandleNVX" => {
            vkGetImageViewHandleNVX::HOST.bind(address);
            Some(vkGetImageViewHandleNVX as *const () as usize)
        }
        "vkGetImageViewOpaqueCaptureDescriptorDataEXT" => {
            vkGetImageViewOpaqueCaptureDescriptorDataEXT::HOST.bind(address);
            Some(vkGetImageViewOpaqueCaptureDescriptorDataEXT as *const () as usize)
        }
        "vkGetLatencyTimingsLegacyNV" => {
            vkGetLatencyTimingsLegacyNV::HOST.bind(address);
            Some(vkGetLatencyTimingsLegacyNV as *const () as usize)
        }
        "vkGetLatencyTimingsNV" => {
            vkGetLatencyTimingsNV::HOST.bind(address);
            Some(vkGetLatencyTimingsNV as *const () as usize)
        }
        "vkGetMemoryAndroidHardwareBufferANDROID" => {
            vkGetMemoryAndroidHardwareBufferANDROID::HOST.bind(address);
            Some(vkGetMemoryAndroidHardwareBufferANDROID as *const () as usize)
        }
        "vkGetMemoryFdKHR" => {
            vkGetMemoryFdKHR::HOST.bind(address);
            Some(vkGetMemoryFdKHR as *const () as usize)
        }
        "vkGetMemoryFdPropertiesKHR" => {
            vkGetMemoryFdPropertiesKHR::HOST.bind(address);
            Some(vkGetMemoryFdPropertiesKHR as *const () as usize)
        }
        "vkGetMemoryHostPointerPropertiesEXT" => {
            vkGetMemoryHostPointerPropertiesEXT::HOST.bind(address);
            Some(vkGetMemoryHostPointerPropertiesEXT as *const () as usize)
        }
        "vkGetMemoryMetalHandleEXT" => {
            vkGetMemoryMetalHandleEXT::HOST.bind(address);
            Some(vkGetMemoryMetalHandleEXT as *const () as usize)
        }
        "vkGetMemoryMetalHandlePropertiesEXT" => {
            vkGetMemoryMetalHandlePropertiesEXT::HOST.bind(address);
            Some(vkGetMemoryMetalHandlePropertiesEXT as *const () as usize)
        }
        "vkGetMemoryNativeBufferOHOS" => {
            vkGetMemoryNativeBufferOHOS::HOST.bind(address);
            Some(vkGetMemoryNativeBufferOHOS as *const () as usize)
        }
        "vkGetMemoryRemoteAddressNV" => {
            vkGetMemoryRemoteAddressNV::HOST.bind(address);
            Some(vkGetMemoryRemoteAddressNV as *const () as usize)
        }
        "vkGetMemorySciBufNV" => {
            vkGetMemorySciBufNV::HOST.bind(address);
            Some(vkGetMemorySciBufNV as *const () as usize)
        }
        "vkGetMemoryWin32HandleKHR" => {
            vkGetMemoryWin32HandleKHR::HOST.bind(address);
            Some(vkGetMemoryWin32HandleKHR as *const () as usize)
        }
        "vkGetMemoryWin32HandleNV" => {
            vkGetMemoryWin32HandleNV::HOST.bind(address);
            Some(vkGetMemoryWin32HandleNV as *const () as usize)
        }
        "vkGetMemoryWin32HandlePropertiesKHR" => {
            vkGetMemoryWin32HandlePropertiesKHR::HOST.bind(address);
            Some(vkGetMemoryWin32HandlePropertiesKHR as *const () as usize)
        }
        "vkGetMemoryZirconHandleFUCHSIA" => {
            vkGetMemoryZirconHandleFUCHSIA::HOST.bind(address);
            Some(vkGetMemoryZirconHandleFUCHSIA as *const () as usize)
        }
        "vkGetMemoryZirconHandlePropertiesFUCHSIA" => {
            vkGetMemoryZirconHandlePropertiesFUCHSIA::HOST.bind(address);
            Some(vkGetMemoryZirconHandlePropertiesFUCHSIA as *const () as usize)
        }
        "vkGetMicromapBuildSizesEXT" => {
            vkGetMicromapBuildSizesEXT::HOST.bind(address);
            Some(vkGetMicromapBuildSizesEXT as *const () as usize)
        }
        "vkGetNativeBufferPropertiesOHOS" => {
            vkGetNativeBufferPropertiesOHOS::HOST.bind(address);
            Some(vkGetNativeBufferPropertiesOHOS as *const () as usize)
        }
        "vkGetPartitionedAccelerationStructuresBuildSizesNV" => {
            vkGetPartitionedAccelerationStructuresBuildSizesNV::HOST.bind(address);
            Some(vkGetPartitionedAccelerationStructuresBuildSizesNV as *const () as usize)
        }
        "vkGetPastPresentationTimingEXT" => {
            vkGetPastPresentationTimingEXT::HOST.bind(address);
            Some(vkGetPastPresentationTimingEXT as *const () as usize)
        }
        "vkGetPastPresentationTimingGOOGLE" => {
            vkGetPastPresentationTimingGOOGLE::HOST.bind(address);
            Some(vkGetPastPresentationTimingGOOGLE as *const () as usize)
        }
        "vkGetPerformanceParameterINTEL" => {
            vkGetPerformanceParameterINTEL::HOST.bind(address);
            Some(vkGetPerformanceParameterINTEL as *const () as usize)
        }
        "vkGetPhysicalDeviceCalibrateableTimeDomainsEXT" => {
            vkGetPhysicalDeviceCalibrateableTimeDomainsEXT::HOST.bind(address);
            Some(vkGetPhysicalDeviceCalibrateableTimeDomainsEXT as *const () as usize)
        }
        "vkGetPhysicalDeviceCalibrateableTimeDomainsKHR" => {
            vkGetPhysicalDeviceCalibrateableTimeDomainsKHR::HOST.bind(address);
            Some(vkGetPhysicalDeviceCalibrateableTimeDomainsKHR as *const () as usize)
        }
        "vkGetPhysicalDeviceCooperativeMatrixFlexibleDimensionsPropertiesNV" => {
            vkGetPhysicalDeviceCooperativeMatrixFlexibleDimensionsPropertiesNV::HOST.bind(address);
            Some(
                vkGetPhysicalDeviceCooperativeMatrixFlexibleDimensionsPropertiesNV as *const ()
                    as usize,
            )
        }
        "vkGetPhysicalDeviceCooperativeMatrixProperties2EXT" => {
            vkGetPhysicalDeviceCooperativeMatrixProperties2EXT::HOST.bind(address);
            Some(vkGetPhysicalDeviceCooperativeMatrixProperties2EXT as *const () as usize)
        }
        "vkGetPhysicalDeviceCooperativeMatrixPropertiesKHR" => {
            vkGetPhysicalDeviceCooperativeMatrixPropertiesKHR::HOST.bind(address);
            Some(vkGetPhysicalDeviceCooperativeMatrixPropertiesKHR as *const () as usize)
        }
        "vkGetPhysicalDeviceCooperativeMatrixPropertiesNV" => {
            vkGetPhysicalDeviceCooperativeMatrixPropertiesNV::HOST.bind(address);
            Some(vkGetPhysicalDeviceCooperativeMatrixPropertiesNV as *const () as usize)
        }
        "vkGetPhysicalDeviceCooperativeVectorPropertiesNV" => {
            vkGetPhysicalDeviceCooperativeVectorPropertiesNV::HOST.bind(address);
            Some(vkGetPhysicalDeviceCooperativeVectorPropertiesNV as *const () as usize)
        }
        "vkGetPhysicalDeviceDescriptorSizeEXT" => {
            vkGetPhysicalDeviceDescriptorSizeEXT::HOST.bind(address);
            Some(vkGetPhysicalDeviceDescriptorSizeEXT as *const () as usize)
        }
        "vkGetPhysicalDeviceDirectFBPresentationSupportEXT" => {
            vkGetPhysicalDeviceDirectFBPresentationSupportEXT::HOST.bind(address);
            Some(vkGetPhysicalDeviceDirectFBPresentationSupportEXT as *const () as usize)
        }
        "vkGetPhysicalDeviceExternalBufferPropertiesKHR" => {
            vkGetPhysicalDeviceExternalBufferPropertiesKHR::HOST.bind(address);
            Some(vkGetPhysicalDeviceExternalBufferPropertiesKHR as *const () as usize)
        }
        "vkGetPhysicalDeviceExternalFencePropertiesKHR" => {
            vkGetPhysicalDeviceExternalFencePropertiesKHR::HOST.bind(address);
            Some(vkGetPhysicalDeviceExternalFencePropertiesKHR as *const () as usize)
        }
        "vkGetPhysicalDeviceExternalImageFormatPropertiesNV" => {
            vkGetPhysicalDeviceExternalImageFormatPropertiesNV::HOST.bind(address);
            Some(vkGetPhysicalDeviceExternalImageFormatPropertiesNV as *const () as usize)
        }
        "vkGetPhysicalDeviceExternalMemorySciBufPropertiesNV" => {
            vkGetPhysicalDeviceExternalMemorySciBufPropertiesNV::HOST.bind(address);
            Some(vkGetPhysicalDeviceExternalMemorySciBufPropertiesNV as *const () as usize)
        }
        "vkGetPhysicalDeviceExternalSemaphorePropertiesKHR" => {
            vkGetPhysicalDeviceExternalSemaphorePropertiesKHR::HOST.bind(address);
            Some(vkGetPhysicalDeviceExternalSemaphorePropertiesKHR as *const () as usize)
        }
        "vkGetPhysicalDeviceExternalTensorPropertiesARM" => {
            vkGetPhysicalDeviceExternalTensorPropertiesARM::HOST.bind(address);
            Some(vkGetPhysicalDeviceExternalTensorPropertiesARM as *const () as usize)
        }
        "vkGetPhysicalDeviceFeatures2KHR" => {
            vkGetPhysicalDeviceFeatures2KHR::HOST.bind(address);
            Some(vkGetPhysicalDeviceFeatures2KHR as *const () as usize)
        }
        "vkGetPhysicalDeviceFormatProperties2KHR" => {
            vkGetPhysicalDeviceFormatProperties2KHR::HOST.bind(address);
            Some(vkGetPhysicalDeviceFormatProperties2KHR as *const () as usize)
        }
        "vkGetPhysicalDeviceFragmentShadingRatesKHR" => {
            vkGetPhysicalDeviceFragmentShadingRatesKHR::HOST.bind(address);
            Some(vkGetPhysicalDeviceFragmentShadingRatesKHR as *const () as usize)
        }
        "vkGetPhysicalDeviceImageFormatProperties2KHR" => {
            vkGetPhysicalDeviceImageFormatProperties2KHR::HOST.bind(address);
            Some(vkGetPhysicalDeviceImageFormatProperties2KHR as *const () as usize)
        }
        "vkGetPhysicalDeviceMemoryProperties2KHR" => {
            vkGetPhysicalDeviceMemoryProperties2KHR::HOST.bind(address);
            Some(vkGetPhysicalDeviceMemoryProperties2KHR as *const () as usize)
        }
        "vkGetPhysicalDeviceMultisamplePropertiesEXT" => {
            vkGetPhysicalDeviceMultisamplePropertiesEXT::HOST.bind(address);
            Some(vkGetPhysicalDeviceMultisamplePropertiesEXT as *const () as usize)
        }
        "vkGetPhysicalDeviceOpticalFlowImageFormatsNV" => {
            vkGetPhysicalDeviceOpticalFlowImageFormatsNV::HOST.bind(address);
            Some(vkGetPhysicalDeviceOpticalFlowImageFormatsNV as *const () as usize)
        }
        "vkGetPhysicalDeviceProperties2KHR" => {
            vkGetPhysicalDeviceProperties2KHR::HOST.bind(address);
            Some(vkGetPhysicalDeviceProperties2KHR as *const () as usize)
        }
        "vkGetPhysicalDeviceQueueFamilyDataGraphEngineOperationPropertiesARM" => {
            vkGetPhysicalDeviceQueueFamilyDataGraphEngineOperationPropertiesARM::HOST.bind(address);
            Some(
                vkGetPhysicalDeviceQueueFamilyDataGraphEngineOperationPropertiesARM as *const ()
                    as usize,
            )
        }
        "vkGetPhysicalDeviceQueueFamilyDataGraphOpticalFlowImageFormatsARM" => {
            vkGetPhysicalDeviceQueueFamilyDataGraphOpticalFlowImageFormatsARM::HOST.bind(address);
            Some(
                vkGetPhysicalDeviceQueueFamilyDataGraphOpticalFlowImageFormatsARM as *const ()
                    as usize,
            )
        }
        "vkGetPhysicalDeviceQueueFamilyDataGraphProcessingEnginePropertiesARM" => {
            vkGetPhysicalDeviceQueueFamilyDataGraphProcessingEnginePropertiesARM::HOST
                .bind(address);
            Some(
                vkGetPhysicalDeviceQueueFamilyDataGraphProcessingEnginePropertiesARM as *const ()
                    as usize,
            )
        }
        "vkGetPhysicalDeviceQueueFamilyDataGraphPropertiesARM" => {
            vkGetPhysicalDeviceQueueFamilyDataGraphPropertiesARM::HOST.bind(address);
            Some(vkGetPhysicalDeviceQueueFamilyDataGraphPropertiesARM as *const () as usize)
        }
        "vkGetPhysicalDeviceQueueFamilyPerformanceQueryPassesKHR" => {
            vkGetPhysicalDeviceQueueFamilyPerformanceQueryPassesKHR::HOST.bind(address);
            Some(vkGetPhysicalDeviceQueueFamilyPerformanceQueryPassesKHR as *const () as usize)
        }
        "vkGetPhysicalDeviceQueueFamilyProperties2KHR" => {
            vkGetPhysicalDeviceQueueFamilyProperties2KHR::HOST.bind(address);
            Some(vkGetPhysicalDeviceQueueFamilyProperties2KHR as *const () as usize)
        }
        "vkGetPhysicalDeviceRefreshableObjectTypesKHR" => {
            vkGetPhysicalDeviceRefreshableObjectTypesKHR::HOST.bind(address);
            Some(vkGetPhysicalDeviceRefreshableObjectTypesKHR as *const () as usize)
        }
        "vkGetPhysicalDeviceSciBufAttributesNV" => {
            vkGetPhysicalDeviceSciBufAttributesNV::HOST.bind(address);
            Some(vkGetPhysicalDeviceSciBufAttributesNV as *const () as usize)
        }
        "vkGetPhysicalDeviceSciSyncAttributesNV" => {
            vkGetPhysicalDeviceSciSyncAttributesNV::HOST.bind(address);
            Some(vkGetPhysicalDeviceSciSyncAttributesNV as *const () as usize)
        }
        "vkGetPhysicalDeviceScreenPresentationSupportQNX" => {
            vkGetPhysicalDeviceScreenPresentationSupportQNX::HOST.bind(address);
            Some(vkGetPhysicalDeviceScreenPresentationSupportQNX as *const () as usize)
        }
        "vkGetPhysicalDeviceSparseImageFormatProperties2KHR" => {
            vkGetPhysicalDeviceSparseImageFormatProperties2KHR::HOST.bind(address);
            Some(vkGetPhysicalDeviceSparseImageFormatProperties2KHR as *const () as usize)
        }
        "vkGetPhysicalDeviceSupportedFramebufferMixedSamplesCombinationsNV" => {
            vkGetPhysicalDeviceSupportedFramebufferMixedSamplesCombinationsNV::HOST.bind(address);
            Some(
                vkGetPhysicalDeviceSupportedFramebufferMixedSamplesCombinationsNV as *const ()
                    as usize,
            )
        }
        "vkGetPhysicalDeviceSurfaceCapabilities2EXT" => {
            vkGetPhysicalDeviceSurfaceCapabilities2EXT::HOST.bind(address);
            Some(vkGetPhysicalDeviceSurfaceCapabilities2EXT as *const () as usize)
        }
        "vkGetPhysicalDeviceSurfacePresentModes2EXT" => {
            vkGetPhysicalDeviceSurfacePresentModes2EXT::HOST.bind(address);
            Some(vkGetPhysicalDeviceSurfacePresentModes2EXT as *const () as usize)
        }
        "vkGetPhysicalDeviceToolPropertiesEXT" => {
            vkGetPhysicalDeviceToolPropertiesEXT::HOST.bind(address);
            Some(vkGetPhysicalDeviceToolPropertiesEXT as *const () as usize)
        }
        "vkGetPhysicalDeviceUbmPresentationSupportSEC" => {
            vkGetPhysicalDeviceUbmPresentationSupportSEC::HOST.bind(address);
            Some(vkGetPhysicalDeviceUbmPresentationSupportSEC as *const () as usize)
        }
        "vkGetPhysicalDeviceVideoCapabilitiesKHR" => {
            vkGetPhysicalDeviceVideoCapabilitiesKHR::HOST.bind(address);
            Some(vkGetPhysicalDeviceVideoCapabilitiesKHR as *const () as usize)
        }
        "vkGetPhysicalDeviceVideoEncodeQualityLevelPropertiesKHR" => {
            vkGetPhysicalDeviceVideoEncodeQualityLevelPropertiesKHR::HOST.bind(address);
            Some(vkGetPhysicalDeviceVideoEncodeQualityLevelPropertiesKHR as *const () as usize)
        }
        "vkGetPhysicalDeviceVideoFormatPropertiesKHR" => {
            vkGetPhysicalDeviceVideoFormatPropertiesKHR::HOST.bind(address);
            Some(vkGetPhysicalDeviceVideoFormatPropertiesKHR as *const () as usize)
        }
        "vkGetPipelineBinaryDataKHR" => {
            vkGetPipelineBinaryDataKHR::HOST.bind(address);
            Some(vkGetPipelineBinaryDataKHR as *const () as usize)
        }
        "vkGetPipelineExecutableInternalRepresentationsKHR" => {
            vkGetPipelineExecutableInternalRepresentationsKHR::HOST.bind(address);
            Some(vkGetPipelineExecutableInternalRepresentationsKHR as *const () as usize)
        }
        "vkGetPipelineExecutablePropertiesKHR" => {
            vkGetPipelineExecutablePropertiesKHR::HOST.bind(address);
            Some(vkGetPipelineExecutablePropertiesKHR as *const () as usize)
        }
        "vkGetPipelineExecutableStatisticsKHR" => {
            vkGetPipelineExecutableStatisticsKHR::HOST.bind(address);
            Some(vkGetPipelineExecutableStatisticsKHR as *const () as usize)
        }
        "vkGetPipelineIndirectDeviceAddressNV" => {
            vkGetPipelineIndirectDeviceAddressNV::HOST.bind(address);
            Some(vkGetPipelineIndirectDeviceAddressNV as *const () as usize)
        }
        "vkGetPipelineIndirectMemoryRequirementsNV" => {
            vkGetPipelineIndirectMemoryRequirementsNV::HOST.bind(address);
            Some(vkGetPipelineIndirectMemoryRequirementsNV as *const () as usize)
        }
        "vkGetPipelineKeyKHR" => {
            vkGetPipelineKeyKHR::HOST.bind(address);
            Some(vkGetPipelineKeyKHR as *const () as usize)
        }
        "vkGetPipelinePropertiesEXT" => {
            vkGetPipelinePropertiesEXT::HOST.bind(address);
            Some(vkGetPipelinePropertiesEXT as *const () as usize)
        }
        "vkGetPrivateDataEXT" => {
            vkGetPrivateDataEXT::HOST.bind(address);
            Some(vkGetPrivateDataEXT as *const () as usize)
        }
        "vkGetQueueCheckpointData2NV" => {
            vkGetQueueCheckpointData2NV::HOST.bind(address);
            Some(vkGetQueueCheckpointData2NV as *const () as usize)
        }
        "vkGetQueueCheckpointDataNV" => {
            vkGetQueueCheckpointDataNV::HOST.bind(address);
            Some(vkGetQueueCheckpointDataNV as *const () as usize)
        }
        "vkGetRandROutputDisplayEXT" => {
            vkGetRandROutputDisplayEXT::HOST.bind(address);
            Some(vkGetRandROutputDisplayEXT as *const () as usize)
        }
        "vkGetRayTracingCaptureReplayShaderGroupHandlesKHR" => {
            vkGetRayTracingCaptureReplayShaderGroupHandlesKHR::HOST.bind(address);
            Some(vkGetRayTracingCaptureReplayShaderGroupHandlesKHR as *const () as usize)
        }
        "vkGetRayTracingShaderGroupHandlesKHR" => {
            vkGetRayTracingShaderGroupHandlesKHR::HOST.bind(address);
            Some(vkGetRayTracingShaderGroupHandlesKHR as *const () as usize)
        }
        "vkGetRayTracingShaderGroupHandlesNV" => {
            vkGetRayTracingShaderGroupHandlesNV::HOST.bind(address);
            Some(vkGetRayTracingShaderGroupHandlesNV as *const () as usize)
        }
        "vkGetRayTracingShaderGroupStackSizeKHR" => {
            vkGetRayTracingShaderGroupStackSizeKHR::HOST.bind(address);
            Some(vkGetRayTracingShaderGroupStackSizeKHR as *const () as usize)
        }
        "vkGetRefreshCycleDurationGOOGLE" => {
            vkGetRefreshCycleDurationGOOGLE::HOST.bind(address);
            Some(vkGetRefreshCycleDurationGOOGLE as *const () as usize)
        }
        "vkGetRenderingAreaGranularityKHR" => {
            vkGetRenderingAreaGranularityKHR::HOST.bind(address);
            Some(vkGetRenderingAreaGranularityKHR as *const () as usize)
        }
        "vkGetSamplerOpaqueCaptureDescriptorDataEXT" => {
            vkGetSamplerOpaqueCaptureDescriptorDataEXT::HOST.bind(address);
            Some(vkGetSamplerOpaqueCaptureDescriptorDataEXT as *const () as usize)
        }
        "vkGetScreenBufferPropertiesQNX" => {
            vkGetScreenBufferPropertiesQNX::HOST.bind(address);
            Some(vkGetScreenBufferPropertiesQNX as *const () as usize)
        }
        "vkGetSemaphoreCounterValueKHR" => {
            vkGetSemaphoreCounterValueKHR::HOST.bind(address);
            Some(vkGetSemaphoreCounterValueKHR as *const () as usize)
        }
        "vkGetSemaphoreFdKHR" => {
            vkGetSemaphoreFdKHR::HOST.bind(address);
            Some(vkGetSemaphoreFdKHR as *const () as usize)
        }
        "vkGetSemaphoreSciSyncObjNV" => {
            vkGetSemaphoreSciSyncObjNV::HOST.bind(address);
            Some(vkGetSemaphoreSciSyncObjNV as *const () as usize)
        }
        "vkGetSemaphoreWin32HandleKHR" => {
            vkGetSemaphoreWin32HandleKHR::HOST.bind(address);
            Some(vkGetSemaphoreWin32HandleKHR as *const () as usize)
        }
        "vkGetSemaphoreZirconHandleFUCHSIA" => {
            vkGetSemaphoreZirconHandleFUCHSIA::HOST.bind(address);
            Some(vkGetSemaphoreZirconHandleFUCHSIA as *const () as usize)
        }
        "vkGetShaderBinaryDataEXT" => {
            vkGetShaderBinaryDataEXT::HOST.bind(address);
            Some(vkGetShaderBinaryDataEXT as *const () as usize)
        }
        "vkGetShaderInfoAMD" => {
            vkGetShaderInfoAMD::HOST.bind(address);
            Some(vkGetShaderInfoAMD as *const () as usize)
        }
        "vkGetShaderInstrumentationValuesARM" => {
            vkGetShaderInstrumentationValuesARM::HOST.bind(address);
            Some(vkGetShaderInstrumentationValuesARM as *const () as usize)
        }
        "vkGetShaderModuleCreateInfoIdentifierEXT" => {
            vkGetShaderModuleCreateInfoIdentifierEXT::HOST.bind(address);
            Some(vkGetShaderModuleCreateInfoIdentifierEXT as *const () as usize)
        }
        "vkGetShaderModuleIdentifierEXT" => {
            vkGetShaderModuleIdentifierEXT::HOST.bind(address);
            Some(vkGetShaderModuleIdentifierEXT as *const () as usize)
        }
        "vkGetSleepStatusLegacyNV" => {
            vkGetSleepStatusLegacyNV::HOST.bind(address);
            Some(vkGetSleepStatusLegacyNV as *const () as usize)
        }
        "vkGetSwapchainCounterEXT" => {
            vkGetSwapchainCounterEXT::HOST.bind(address);
            Some(vkGetSwapchainCounterEXT as *const () as usize)
        }
        "vkGetSwapchainGrallocUsage2ANDROID" => {
            vkGetSwapchainGrallocUsage2ANDROID::HOST.bind(address);
            Some(vkGetSwapchainGrallocUsage2ANDROID as *const () as usize)
        }
        "vkGetSwapchainGrallocUsageANDROID" => {
            vkGetSwapchainGrallocUsageANDROID::HOST.bind(address);
            Some(vkGetSwapchainGrallocUsageANDROID as *const () as usize)
        }
        "vkGetSwapchainGrallocUsageOHOS" => {
            vkGetSwapchainGrallocUsageOHOS::HOST.bind(address);
            Some(vkGetSwapchainGrallocUsageOHOS as *const () as usize)
        }
        "vkGetSwapchainStatusKHR" => {
            vkGetSwapchainStatusKHR::HOST.bind(address);
            Some(vkGetSwapchainStatusKHR as *const () as usize)
        }
        "vkGetSwapchainTimeDomainPropertiesEXT" => {
            vkGetSwapchainTimeDomainPropertiesEXT::HOST.bind(address);
            Some(vkGetSwapchainTimeDomainPropertiesEXT as *const () as usize)
        }
        "vkGetSwapchainTimingPropertiesEXT" => {
            vkGetSwapchainTimingPropertiesEXT::HOST.bind(address);
            Some(vkGetSwapchainTimingPropertiesEXT as *const () as usize)
        }
        "vkGetTensorMemoryRequirementsARM" => {
            vkGetTensorMemoryRequirementsARM::HOST.bind(address);
            Some(vkGetTensorMemoryRequirementsARM as *const () as usize)
        }
        "vkGetTensorOpaqueCaptureDataARM" => {
            vkGetTensorOpaqueCaptureDataARM::HOST.bind(address);
            Some(vkGetTensorOpaqueCaptureDataARM as *const () as usize)
        }
        "vkGetTensorOpaqueCaptureDescriptorDataARM" => {
            vkGetTensorOpaqueCaptureDescriptorDataARM::HOST.bind(address);
            Some(vkGetTensorOpaqueCaptureDescriptorDataARM as *const () as usize)
        }
        "vkGetTensorViewOpaqueCaptureDescriptorDataARM" => {
            vkGetTensorViewOpaqueCaptureDescriptorDataARM::HOST.bind(address);
            Some(vkGetTensorViewOpaqueCaptureDescriptorDataARM as *const () as usize)
        }
        "vkGetValidationCacheDataEXT" => {
            vkGetValidationCacheDataEXT::HOST.bind(address);
            Some(vkGetValidationCacheDataEXT as *const () as usize)
        }
        "vkGetVideoSessionMemoryRequirementsKHR" => {
            vkGetVideoSessionMemoryRequirementsKHR::HOST.bind(address);
            Some(vkGetVideoSessionMemoryRequirementsKHR as *const () as usize)
        }
        "vkGetWinrtDisplayNV" => {
            vkGetWinrtDisplayNV::HOST.bind(address);
            Some(vkGetWinrtDisplayNV as *const () as usize)
        }
        "vkImportFenceFdKHR" => {
            vkImportFenceFdKHR::HOST.bind(address);
            Some(vkImportFenceFdKHR as *const () as usize)
        }
        "vkImportFenceSciSyncFenceNV" => {
            vkImportFenceSciSyncFenceNV::HOST.bind(address);
            Some(vkImportFenceSciSyncFenceNV as *const () as usize)
        }
        "vkImportFenceSciSyncObjNV" => {
            vkImportFenceSciSyncObjNV::HOST.bind(address);
            Some(vkImportFenceSciSyncObjNV as *const () as usize)
        }
        "vkImportFenceWin32HandleKHR" => {
            vkImportFenceWin32HandleKHR::HOST.bind(address);
            Some(vkImportFenceWin32HandleKHR as *const () as usize)
        }
        "vkImportSemaphoreFdKHR" => {
            vkImportSemaphoreFdKHR::HOST.bind(address);
            Some(vkImportSemaphoreFdKHR as *const () as usize)
        }
        "vkImportSemaphoreSciSyncObjNV" => {
            vkImportSemaphoreSciSyncObjNV::HOST.bind(address);
            Some(vkImportSemaphoreSciSyncObjNV as *const () as usize)
        }
        "vkImportSemaphoreWin32HandleKHR" => {
            vkImportSemaphoreWin32HandleKHR::HOST.bind(address);
            Some(vkImportSemaphoreWin32HandleKHR as *const () as usize)
        }
        "vkImportSemaphoreZirconHandleFUCHSIA" => {
            vkImportSemaphoreZirconHandleFUCHSIA::HOST.bind(address);
            Some(vkImportSemaphoreZirconHandleFUCHSIA as *const () as usize)
        }
        "vkInitializePerformanceApiINTEL" => {
            vkInitializePerformanceApiINTEL::HOST.bind(address);
            Some(vkInitializePerformanceApiINTEL as *const () as usize)
        }
        "vkLatencySleepLegacyNV" => {
            vkLatencySleepLegacyNV::HOST.bind(address);
            Some(vkLatencySleepLegacyNV as *const () as usize)
        }
        "vkLatencySleepNV" => {
            vkLatencySleepNV::HOST.bind(address);
            Some(vkLatencySleepNV as *const () as usize)
        }
        "vkMapMemory2KHR" => {
            vkMapMemory2KHR::HOST.bind(address);
            Some(vkMapMemory2KHR as *const () as usize)
        }
        "vkMergeValidationCachesEXT" => {
            vkMergeValidationCachesEXT::HOST.bind(address);
            Some(vkMergeValidationCachesEXT as *const () as usize)
        }
        "vkQueueBeginDebugUtilsLabelEXT" => {
            vkQueueBeginDebugUtilsLabelEXT::HOST.bind(address);
            Some(vkQueueBeginDebugUtilsLabelEXT as *const () as usize)
        }
        "vkQueueEndDebugUtilsLabelEXT" => {
            vkQueueEndDebugUtilsLabelEXT::HOST.bind(address);
            Some(vkQueueEndDebugUtilsLabelEXT as *const () as usize)
        }
        "vkQueueInsertDebugUtilsLabelEXT" => {
            vkQueueInsertDebugUtilsLabelEXT::HOST.bind(address);
            Some(vkQueueInsertDebugUtilsLabelEXT as *const () as usize)
        }
        "vkQueueNotifyOutOfBandLegacyNV" => {
            vkQueueNotifyOutOfBandLegacyNV::HOST.bind(address);
            Some(vkQueueNotifyOutOfBandLegacyNV as *const () as usize)
        }
        "vkQueueNotifyOutOfBandNV" => {
            vkQueueNotifyOutOfBandNV::HOST.bind(address);
            Some(vkQueueNotifyOutOfBandNV as *const () as usize)
        }
        "vkQueueSetPerfHintQCOM" => {
            vkQueueSetPerfHintQCOM::HOST.bind(address);
            Some(vkQueueSetPerfHintQCOM as *const () as usize)
        }
        "vkQueueSetPerformanceConfigurationINTEL" => {
            vkQueueSetPerformanceConfigurationINTEL::HOST.bind(address);
            Some(vkQueueSetPerformanceConfigurationINTEL as *const () as usize)
        }
        "vkQueueSignalReleaseImageANDROID" => {
            vkQueueSignalReleaseImageANDROID::HOST.bind(address);
            Some(vkQueueSignalReleaseImageANDROID as *const () as usize)
        }
        "vkQueueSignalReleaseImageOHOS" => {
            vkQueueSignalReleaseImageOHOS::HOST.bind(address);
            Some(vkQueueSignalReleaseImageOHOS as *const () as usize)
        }
        "vkQueueSubmit2KHR" => {
            vkQueueSubmit2KHR::HOST.bind(address);
            Some(vkQueueSubmit2KHR as *const () as usize)
        }
        "vkRegisterCustomBorderColorEXT" => {
            vkRegisterCustomBorderColorEXT::HOST.bind(address);
            Some(vkRegisterCustomBorderColorEXT as *const () as usize)
        }
        "vkRegisterDeviceEventEXT" => {
            vkRegisterDeviceEventEXT::HOST.bind(address);
            Some(vkRegisterDeviceEventEXT as *const () as usize)
        }
        "vkRegisterDisplayEventEXT" => {
            vkRegisterDisplayEventEXT::HOST.bind(address);
            Some(vkRegisterDisplayEventEXT as *const () as usize)
        }
        "vkReleaseCapturedPipelineDataKHR" => {
            vkReleaseCapturedPipelineDataKHR::HOST.bind(address);
            Some(vkReleaseCapturedPipelineDataKHR as *const () as usize)
        }
        "vkReleaseDisplayEXT" => {
            vkReleaseDisplayEXT::HOST.bind(address);
            Some(vkReleaseDisplayEXT as *const () as usize)
        }
        "vkReleaseFullScreenExclusiveModeEXT" => {
            vkReleaseFullScreenExclusiveModeEXT::HOST.bind(address);
            Some(vkReleaseFullScreenExclusiveModeEXT as *const () as usize)
        }
        "vkReleasePerformanceConfigurationINTEL" => {
            vkReleasePerformanceConfigurationINTEL::HOST.bind(address);
            Some(vkReleasePerformanceConfigurationINTEL as *const () as usize)
        }
        "vkReleaseProfilingLockKHR" => {
            vkReleaseProfilingLockKHR::HOST.bind(address);
            Some(vkReleaseProfilingLockKHR as *const () as usize)
        }
        "vkReleaseSwapchainImagesEXT" => {
            vkReleaseSwapchainImagesEXT::HOST.bind(address);
            Some(vkReleaseSwapchainImagesEXT as *const () as usize)
        }
        "vkReleaseSwapchainImagesKHR" => {
            vkReleaseSwapchainImagesKHR::HOST.bind(address);
            Some(vkReleaseSwapchainImagesKHR as *const () as usize)
        }
        "vkResetGpaSessionAMD" => {
            vkResetGpaSessionAMD::HOST.bind(address);
            Some(vkResetGpaSessionAMD as *const () as usize)
        }
        "vkResetQueryPoolEXT" => {
            vkResetQueryPoolEXT::HOST.bind(address);
            Some(vkResetQueryPoolEXT as *const () as usize)
        }
        "vkSetBufferCollectionBufferConstraintsFUCHSIA" => {
            vkSetBufferCollectionBufferConstraintsFUCHSIA::HOST.bind(address);
            Some(vkSetBufferCollectionBufferConstraintsFUCHSIA as *const () as usize)
        }
        "vkSetBufferCollectionImageConstraintsFUCHSIA" => {
            vkSetBufferCollectionImageConstraintsFUCHSIA::HOST.bind(address);
            Some(vkSetBufferCollectionImageConstraintsFUCHSIA as *const () as usize)
        }
        "vkSetDebugUtilsObjectNameEXT" => {
            vkSetDebugUtilsObjectNameEXT::HOST.bind(address);
            Some(vkSetDebugUtilsObjectNameEXT as *const () as usize)
        }
        "vkSetDebugUtilsObjectTagEXT" => {
            vkSetDebugUtilsObjectTagEXT::HOST.bind(address);
            Some(vkSetDebugUtilsObjectTagEXT as *const () as usize)
        }
        "vkSetDeviceMemoryPriorityEXT" => {
            vkSetDeviceMemoryPriorityEXT::HOST.bind(address);
            Some(vkSetDeviceMemoryPriorityEXT as *const () as usize)
        }
        "vkSetGpaDeviceClockModeAMD" => {
            vkSetGpaDeviceClockModeAMD::HOST.bind(address);
            Some(vkSetGpaDeviceClockModeAMD as *const () as usize)
        }
        "vkSetHdrMetadataEXT" => {
            vkSetHdrMetadataEXT::HOST.bind(address);
            Some(vkSetHdrMetadataEXT as *const () as usize)
        }
        "vkSetLatencyMarkerLegacyNV" => {
            vkSetLatencyMarkerLegacyNV::HOST.bind(address);
            Some(vkSetLatencyMarkerLegacyNV as *const () as usize)
        }
        "vkSetLatencyMarkerNV" => {
            vkSetLatencyMarkerNV::HOST.bind(address);
            Some(vkSetLatencyMarkerNV as *const () as usize)
        }
        "vkSetLatencySleepModeLegacyNV" => {
            vkSetLatencySleepModeLegacyNV::HOST.bind(address);
            Some(vkSetLatencySleepModeLegacyNV as *const () as usize)
        }
        "vkSetLatencySleepModeNV" => {
            vkSetLatencySleepModeNV::HOST.bind(address);
            Some(vkSetLatencySleepModeNV as *const () as usize)
        }
        "vkSetLocalDimmingAMD" => {
            vkSetLocalDimmingAMD::HOST.bind(address);
            Some(vkSetLocalDimmingAMD as *const () as usize)
        }
        "vkSetPrivateDataEXT" => {
            vkSetPrivateDataEXT::HOST.bind(address);
            Some(vkSetPrivateDataEXT as *const () as usize)
        }
        "vkSetSwapchainPresentTimingQueueSizeEXT" => {
            vkSetSwapchainPresentTimingQueueSizeEXT::HOST.bind(address);
            Some(vkSetSwapchainPresentTimingQueueSizeEXT as *const () as usize)
        }
        "vkShutdownLatencyDeviceLegacyNV" => {
            vkShutdownLatencyDeviceLegacyNV::HOST.bind(address);
            Some(vkShutdownLatencyDeviceLegacyNV as *const () as usize)
        }
        "vkSignalSemaphoreKHR" => {
            vkSignalSemaphoreKHR::HOST.bind(address);
            Some(vkSignalSemaphoreKHR as *const () as usize)
        }
        "vkSubmitDebugUtilsMessageEXT" => {
            vkSubmitDebugUtilsMessageEXT::HOST.bind(address);
            Some(vkSubmitDebugUtilsMessageEXT as *const () as usize)
        }
        "vkTransitionImageLayoutEXT" => {
            vkTransitionImageLayoutEXT::HOST.bind(address);
            Some(vkTransitionImageLayoutEXT as *const () as usize)
        }
        "vkTrimCommandPoolKHR" => {
            vkTrimCommandPoolKHR::HOST.bind(address);
            Some(vkTrimCommandPoolKHR as *const () as usize)
        }
        "vkUninitializePerformanceApiINTEL" => {
            vkUninitializePerformanceApiINTEL::HOST.bind(address);
            Some(vkUninitializePerformanceApiINTEL as *const () as usize)
        }
        "vkUnmapMemory2KHR" => {
            vkUnmapMemory2KHR::HOST.bind(address);
            Some(vkUnmapMemory2KHR as *const () as usize)
        }
        "vkUnregisterCustomBorderColorEXT" => {
            vkUnregisterCustomBorderColorEXT::HOST.bind(address);
            Some(vkUnregisterCustomBorderColorEXT as *const () as usize)
        }
        "vkUpdateDescriptorSetWithTemplateKHR" => {
            vkUpdateDescriptorSetWithTemplateKHR::HOST.bind(address);
            Some(vkUpdateDescriptorSetWithTemplateKHR as *const () as usize)
        }
        "vkUpdateIndirectExecutionSetPipelineEXT" => {
            vkUpdateIndirectExecutionSetPipelineEXT::HOST.bind(address);
            Some(vkUpdateIndirectExecutionSetPipelineEXT as *const () as usize)
        }
        "vkUpdateIndirectExecutionSetShaderEXT" => {
            vkUpdateIndirectExecutionSetShaderEXT::HOST.bind(address);
            Some(vkUpdateIndirectExecutionSetShaderEXT as *const () as usize)
        }
        "vkUpdateVideoSessionParametersKHR" => {
            vkUpdateVideoSessionParametersKHR::HOST.bind(address);
            Some(vkUpdateVideoSessionParametersKHR as *const () as usize)
        }
        "vkWaitForPresent2KHR" => {
            vkWaitForPresent2KHR::HOST.bind(address);
            Some(vkWaitForPresent2KHR as *const () as usize)
        }
        "vkWaitForPresentKHR" => {
            vkWaitForPresentKHR::HOST.bind(address);
            Some(vkWaitForPresentKHR as *const () as usize)
        }
        "vkWaitSemaphoresKHR" => {
            vkWaitSemaphoresKHR::HOST.bind(address);
            Some(vkWaitSemaphoresKHR as *const () as usize)
        }
        "vkWriteAccelerationStructuresPropertiesKHR" => {
            vkWriteAccelerationStructuresPropertiesKHR::HOST.bind(address);
            Some(vkWriteAccelerationStructuresPropertiesKHR as *const () as usize)
        }
        "vkWriteMicromapsPropertiesEXT" => {
            vkWriteMicromapsPropertiesEXT::HOST.bind(address);
            Some(vkWriteMicromapsPropertiesEXT as *const () as usize)
        }
        "vkWriteResourceDescriptorsEXT" => {
            vkWriteResourceDescriptorsEXT::HOST.bind(address);
            Some(vkWriteResourceDescriptorsEXT as *const () as usize)
        }
        "vkWriteSamplerDescriptorsEXT" => {
            vkWriteSamplerDescriptorsEXT::HOST.bind(address);
            Some(vkWriteSamplerDescriptorsEXT as *const () as usize)
        }
        _ => None,
    }
}

pub const REGISTRY_COMMAND_COUNT: usize = 865;

pub const NAMES: &[&str] = &[
    "vkAcquireDrmDisplayEXT",
    "vkAcquireFullScreenExclusiveModeEXT",
    "vkAcquireImageANDROID",
    "vkAcquireImageOHOS",
    "vkAcquirePerformanceConfigurationINTEL",
    "vkAcquireProfilingLockKHR",
    "vkAcquireWinrtDisplayNV",
    "vkAcquireXlibDisplayEXT",
    "vkAntiLagUpdateAMD",
    "vkBindAccelerationStructureMemoryNV",
    "vkBindBufferMemory2KHR",
    "vkBindDataGraphPipelineSessionMemoryARM",
    "vkBindImageMemory2KHR",
    "vkBindOpticalFlowSessionImageNV",
    "vkBindTensorMemoryARM",
    "vkBindVideoSessionMemoryKHR",
    "vkBuildAccelerationStructuresKHR",
    "vkBuildMicromapsEXT",
    "vkClearShaderInstrumentationMetricsARM",
    "vkCmdBeginConditionalRendering2EXT",
    "vkCmdBeginConditionalRenderingEXT",
    "vkCmdBeginCustomResolveEXT",
    "vkCmdBeginDebugUtilsLabelEXT",
    "vkCmdBeginGpaSampleAMD",
    "vkCmdBeginGpaSessionAMD",
    "vkCmdBeginPerTileExecutionQCOM",
    "vkCmdBeginQueryIndexedEXT",
    "vkCmdBeginRenderPass2KHR",
    "vkCmdBeginRenderingKHR",
    "vkCmdBeginShaderInstrumentationARM",
    "vkCmdBeginTransformFeedback2EXT",
    "vkCmdBeginTransformFeedbackEXT",
    "vkCmdBeginVideoCodingKHR",
    "vkCmdBindDescriptorBufferEmbeddedSamplers2EXT",
    "vkCmdBindDescriptorBufferEmbeddedSamplersEXT",
    "vkCmdBindDescriptorBuffersEXT",
    "vkCmdBindDescriptorSets2KHR",
    "vkCmdBindIndexBuffer2KHR",
    "vkCmdBindIndexBuffer3KHR",
    "vkCmdBindInvocationMaskHUAWEI",
    "vkCmdBindPipelineShaderGroupNV",
    "vkCmdBindResourceHeapEXT",
    "vkCmdBindSamplerHeapEXT",
    "vkCmdBindShadersEXT",
    "vkCmdBindShadingRateImageNV",
    "vkCmdBindTileMemoryQCOM",
    "vkCmdBindTransformFeedbackBuffers2EXT",
    "vkCmdBindTransformFeedbackBuffersEXT",
    "vkCmdBindVertexBuffers2EXT",
    "vkCmdBindVertexBuffers3KHR",
    "vkCmdBlitImage2KHR",
    "vkCmdBuildAccelerationStructureNV",
    "vkCmdBuildAccelerationStructuresIndirectKHR",
    "vkCmdBuildAccelerationStructuresKHR",
    "vkCmdBuildClusterAccelerationStructureIndirectNV",
    "vkCmdBuildMicromapsEXT",
    "vkCmdBuildPartitionedAccelerationStructuresNV",
    "vkCmdControlVideoCodingKHR",
    "vkCmdConvertCooperativeVectorMatrixNV",
    "vkCmdCopyAccelerationStructureKHR",
    "vkCmdCopyAccelerationStructureNV",
    "vkCmdCopyAccelerationStructureToMemoryKHR",
    "vkCmdCopyBuffer2KHR",
    "vkCmdCopyBufferToImage2KHR",
    "vkCmdCopyGpaSessionResultsAMD",
    "vkCmdCopyImage2KHR",
    "vkCmdCopyImageToBuffer2KHR",
    "vkCmdCopyImageToMemoryKHR",
    "vkCmdCopyMemoryIndirectKHR",
    "vkCmdCopyMemoryIndirectNV",
    "vkCmdCopyMemoryKHR",
    "vkCmdCopyMemoryToAccelerationStructureKHR",
    "vkCmdCopyMemoryToImageIndirectKHR",
    "vkCmdCopyMemoryToImageIndirectNV",
    "vkCmdCopyMemoryToImageKHR",
    "vkCmdCopyMemoryToMicromapEXT",
    "vkCmdCopyMicromapEXT",
    "vkCmdCopyMicromapToMemoryEXT",
    "vkCmdCopyQueryPoolResultsToMemoryKHR",
    "vkCmdCopyTensorARM",
    "vkCmdCuLaunchKernelNVX",
    "vkCmdCudaLaunchKernelNV",
    "vkCmdDebugMarkerBeginEXT",
    "vkCmdDebugMarkerEndEXT",
    "vkCmdDebugMarkerInsertEXT",
    "vkCmdDecodeVideoKHR",
    "vkCmdDecompressMemoryEXT",
    "vkCmdDecompressMemoryIndirectCountEXT",
    "vkCmdDecompressMemoryIndirectCountNV",
    "vkCmdDecompressMemoryNV",
    "vkCmdDispatchBaseKHR",
    "vkCmdDispatchDataGraphARM",
    "vkCmdDispatchGraphAMDX",
    "vkCmdDispatchGraphIndirectAMDX",
    "vkCmdDispatchGraphIndirectCountAMDX",
    "vkCmdDispatchIndirect2KHR",
    "vkCmdDispatchTileQCOM",
    "vkCmdDrawClusterHUAWEI",
    "vkCmdDrawClusterIndirectHUAWEI",
    "vkCmdDrawIndexedIndirect2KHR",
    "vkCmdDrawIndexedIndirectCount2KHR",
    "vkCmdDrawIndexedIndirectCountAMD",
    "vkCmdDrawIndexedIndirectCountKHR",
    "vkCmdDrawIndirect2KHR",
    "vkCmdDrawIndirectByteCount2EXT",
    "vkCmdDrawIndirectByteCountEXT",
    "vkCmdDrawIndirectCount2KHR",
    "vkCmdDrawIndirectCountAMD",
    "vkCmdDrawIndirectCountKHR",
    "vkCmdDrawMeshTasksEXT",
    "vkCmdDrawMeshTasksIndirect2EXT",
    "vkCmdDrawMeshTasksIndirectCount2EXT",
    "vkCmdDrawMeshTasksIndirectCountEXT",
    "vkCmdDrawMeshTasksIndirectCountNV",
    "vkCmdDrawMeshTasksIndirectEXT",
    "vkCmdDrawMeshTasksIndirectNV",
    "vkCmdDrawMeshTasksNV",
    "vkCmdDrawMultiEXT",
    "vkCmdDrawMultiIndexedEXT",
    "vkCmdEncodeVideoKHR",
    "vkCmdEndConditionalRenderingEXT",
    "vkCmdEndDebugUtilsLabelEXT",
    "vkCmdEndGpaSampleAMD",
    "vkCmdEndGpaSessionAMD",
    "vkCmdEndPerTileExecutionQCOM",
    "vkCmdEndQueryIndexedEXT",
    "vkCmdEndRenderPass2KHR",
    "vkCmdEndRendering2EXT",
    "vkCmdEndRendering2KHR",
    "vkCmdEndRenderingKHR",
    "vkCmdEndShaderInstrumentationARM",
    "vkCmdEndTransformFeedback2EXT",
    "vkCmdEndTransformFeedbackEXT",
    "vkCmdEndVideoCodingKHR",
    "vkCmdExecuteGeneratedCommandsEXT",
    "vkCmdExecuteGeneratedCommandsNV",
    "vkCmdFillMemoryKHR",
    "vkCmdInitializeGraphScratchMemoryAMDX",
    "vkCmdInsertDebugUtilsLabelEXT",
    "vkCmdNextSubpass2KHR",
    "vkCmdOpticalFlowExecuteNV",
    "vkCmdPipelineBarrier2KHR",
    "vkCmdPreprocessGeneratedCommandsEXT",
    "vkCmdPreprocessGeneratedCommandsNV",
    "vkCmdPushConstants2KHR",
    "vkCmdPushDataEXT",
    "vkCmdPushDescriptorSet2KHR",
    "vkCmdPushDescriptorSetKHR",
    "vkCmdPushDescriptorSetWithTemplate2KHR",
    "vkCmdPushDescriptorSetWithTemplateKHR",
    "vkCmdRefreshObjectsKHR",
    "vkCmdResetEvent2KHR",
    "vkCmdResolveImage2KHR",
    "vkCmdSetAlphaToCoverageEnableEXT",
    "vkCmdSetAlphaToOneEnableEXT",
    "vkCmdSetAttachmentFeedbackLoopEnableEXT",
    "vkCmdSetCheckpointNV",
    "vkCmdSetCoarseSampleOrderNV",
    "vkCmdSetColorBlendAdvancedEXT",
    "vkCmdSetColorBlendEnableEXT",
    "vkCmdSetColorBlendEquationEXT",
    "vkCmdSetColorWriteEnableEXT",
    "vkCmdSetColorWriteMaskEXT",
    "vkCmdSetComputeOccupancyPriorityNV",
    "vkCmdSetConservativeRasterizationModeEXT",
    "vkCmdSetCoverageModulationModeNV",
    "vkCmdSetCoverageModulationTableEnableNV",
    "vkCmdSetCoverageModulationTableNV",
    "vkCmdSetCoverageReductionModeNV",
    "vkCmdSetCoverageToColorEnableNV",
    "vkCmdSetCoverageToColorLocationNV",
    "vkCmdSetCullModeEXT",
    "vkCmdSetDepthBias2EXT",
    "vkCmdSetDepthBiasEnableEXT",
    "vkCmdSetDepthBoundsTestEnableEXT",
    "vkCmdSetDepthClampEnableEXT",
    "vkCmdSetDepthClampRangeEXT",
    "vkCmdSetDepthClipEnableEXT",
    "vkCmdSetDepthClipNegativeOneToOneEXT",
    "vkCmdSetDepthCompareOpEXT",
    "vkCmdSetDepthTestEnableEXT",
    "vkCmdSetDepthWriteEnableEXT",
    "vkCmdSetDescriptorBufferOffsets2EXT",
    "vkCmdSetDescriptorBufferOffsetsEXT",
    "vkCmdSetDeviceMaskKHR",
    "vkCmdSetDiscardRectangleEXT",
    "vkCmdSetDiscardRectangleEnableEXT",
    "vkCmdSetDiscardRectangleModeEXT",
    "vkCmdSetDispatchParametersARM",
    "vkCmdSetEvent2KHR",
    "vkCmdSetExclusiveScissorEnableNV",
    "vkCmdSetExclusiveScissorNV",
    "vkCmdSetExtraPrimitiveOverestimationSizeEXT",
    "vkCmdSetFragmentShadingRateEnumNV",
    "vkCmdSetFragmentShadingRateKHR",
    "vkCmdSetFrontFaceEXT",
    "vkCmdSetLineRasterizationModeEXT",
    "vkCmdSetLineStippleEXT",
    "vkCmdSetLineStippleEnableEXT",
    "vkCmdSetLineStippleKHR",
    "vkCmdSetLogicOpEXT",
    "vkCmdSetLogicOpEnableEXT",
    "vkCmdSetPatchControlPointsEXT",
    "vkCmdSetPerformanceMarkerINTEL",
    "vkCmdSetPerformanceOverrideINTEL",
    "vkCmdSetPerformanceStreamMarkerINTEL",
    "vkCmdSetPolygonModeEXT",
    "vkCmdSetPrimitiveRestartEnableEXT",
    "vkCmdSetPrimitiveRestartIndexEXT",
    "vkCmdSetPrimitiveTopologyEXT",
    "vkCmdSetProvokingVertexModeEXT",
    "vkCmdSetRasterizationSamplesEXT",
    "vkCmdSetRasterizationStreamEXT",
    "vkCmdSetRasterizerDiscardEnableEXT",
    "vkCmdSetRayTracingPipelineStackSizeKHR",
    "vkCmdSetRenderingAttachmentLocationsKHR",
    "vkCmdSetRenderingInputAttachmentIndicesKHR",
    "vkCmdSetRepresentativeFragmentTestEnableNV",
    "vkCmdSetSampleLocationsEXT",
    "vkCmdSetSampleLocationsEnableEXT",
    "vkCmdSetSampleMaskEXT",
    "vkCmdSetScissorWithCountEXT",
    "vkCmdSetShadingRateImageEnableNV",
    "vkCmdSetStencilOpEXT",
    "vkCmdSetStencilTestEnableEXT",
    "vkCmdSetTessellationDomainOriginEXT",
    "vkCmdSetVertexInputEXT",
    "vkCmdSetViewportShadingRatePaletteNV",
    "vkCmdSetViewportSwizzleNV",
    "vkCmdSetViewportWScalingEnableNV",
    "vkCmdSetViewportWScalingNV",
    "vkCmdSetViewportWithCountEXT",
    "vkCmdSubpassShadingHUAWEI",
    "vkCmdTraceRaysIndirect2KHR",
    "vkCmdTraceRaysIndirectKHR",
    "vkCmdTraceRaysKHR",
    "vkCmdTraceRaysNV",
    "vkCmdUpdateMemoryKHR",
    "vkCmdUpdatePipelineIndirectBufferNV",
    "vkCmdWaitEvents2KHR",
    "vkCmdWriteAccelerationStructuresPropertiesKHR",
    "vkCmdWriteAccelerationStructuresPropertiesNV",
    "vkCmdWriteBufferMarker2AMD",
    "vkCmdWriteBufferMarkerAMD",
    "vkCmdWriteMarkerToMemoryAMD",
    "vkCmdWriteMicromapsPropertiesEXT",
    "vkCmdWriteTimestamp2KHR",
    "vkCompileDeferredNV",
    "vkConvertCooperativeVectorMatrixNV",
    "vkCopyAccelerationStructureKHR",
    "vkCopyAccelerationStructureToMemoryKHR",
    "vkCopyImageToImageEXT",
    "vkCopyImageToMemoryEXT",
    "vkCopyMemoryToAccelerationStructureKHR",
    "vkCopyMemoryToImageEXT",
    "vkCopyMemoryToMicromapEXT",
    "vkCopyMicromapEXT",
    "vkCopyMicromapToMemoryEXT",
    "vkCreateAccelerationStructure2KHR",
    "vkCreateAccelerationStructureKHR",
    "vkCreateAccelerationStructureNV",
    "vkCreateAndroidSurfaceKHR",
    "vkCreateBufferCollectionFUCHSIA",
    "vkCreateCuFunctionNVX",
    "vkCreateCuModuleNVX",
    "vkCreateCudaFunctionNV",
    "vkCreateCudaModuleNV",
    "vkCreateDataGraphPipelineSessionARM",
    "vkCreateDataGraphPipelinesARM",
    "vkCreateDeferredOperationKHR",
    "vkCreateDescriptorUpdateTemplateKHR",
    "vkCreateDirectFBSurfaceEXT",
    "vkCreateExecutionGraphPipelinesAMDX",
    "vkCreateExternalComputeQueueNV",
    "vkCreateGpaSessionAMD",
    "vkCreateIOSSurfaceMVK",
    "vkCreateImagePipeSurfaceFUCHSIA",
    "vkCreateIndirectCommandsLayoutEXT",
    "vkCreateIndirectCommandsLayoutNV",
    "vkCreateIndirectExecutionSetEXT",
    "vkCreateMacOSSurfaceMVK",
    "vkCreateMetalSurfaceEXT",
    "vkCreateMicromapEXT",
    "vkCreateOpticalFlowSessionNV",
    "vkCreatePipelineBinariesKHR",
    "vkCreatePrivateDataSlotEXT",
    "vkCreateRayTracingPipelinesKHR",
    "vkCreateRayTracingPipelinesNV",
    "vkCreateRenderPass2KHR",
    "vkCreateSamplerYcbcrConversionKHR",
    "vkCreateScreenSurfaceQNX",
    "vkCreateSemaphoreSciSyncPoolNV",
    "vkCreateShaderInstrumentationARM",
    "vkCreateShadersEXT",
    "vkCreateStreamDescriptorSurfaceGGP",
    "vkCreateSurfaceOHOS",
    "vkCreateTensorARM",
    "vkCreateTensorViewARM",
    "vkCreateUbmSurfaceSEC",
    "vkCreateValidationCacheEXT",
    "vkCreateViSurfaceNN",
    "vkCreateVideoSessionKHR",
    "vkCreateVideoSessionParametersKHR",
    "vkDebugMarkerSetObjectNameEXT",
    "vkDebugMarkerSetObjectTagEXT",
    "vkDebugReportMessageEXT",
    "vkDeferredOperationJoinKHR",
    "vkDestroyAccelerationStructureKHR",
    "vkDestroyAccelerationStructureNV",
    "vkDestroyBufferCollectionFUCHSIA",
    "vkDestroyCuFunctionNVX",
    "vkDestroyCuModuleNVX",
    "vkDestroyCudaFunctionNV",
    "vkDestroyCudaModuleNV",
    "vkDestroyDataGraphPipelineSessionARM",
    "vkDestroyDebugReportCallbackEXT",
    "vkDestroyDebugUtilsMessengerEXT",
    "vkDestroyDeferredOperationKHR",
    "vkDestroyDescriptorUpdateTemplateKHR",
    "vkDestroyExternalComputeQueueNV",
    "vkDestroyGpaSessionAMD",
    "vkDestroyIndirectCommandsLayoutEXT",
    "vkDestroyIndirectCommandsLayoutNV",
    "vkDestroyIndirectExecutionSetEXT",
    "vkDestroyMicromapEXT",
    "vkDestroyOpticalFlowSessionNV",
    "vkDestroyPipelineBinaryKHR",
    "vkDestroyPrivateDataSlotEXT",
    "vkDestroySamplerYcbcrConversionKHR",
    "vkDestroySemaphoreSciSyncPoolNV",
    "vkDestroyShaderEXT",
    "vkDestroyShaderInstrumentationARM",
    "vkDestroyTensorARM",
    "vkDestroyTensorViewARM",
    "vkDestroyValidationCacheEXT",
    "vkDestroyVideoSessionKHR",
    "vkDestroyVideoSessionParametersKHR",
    "vkDisplayPowerControlEXT",
    "vkEnumeratePhysicalDeviceGroupsKHR",
    "vkEnumeratePhysicalDeviceQueueFamilyPerformanceCountersByRegionARM",
    "vkEnumeratePhysicalDeviceQueueFamilyPerformanceQueryCountersKHR",
    "vkEnumeratePhysicalDeviceShaderInstrumentationMetricsARM",
    "vkExportMetalObjectsEXT",
    "vkGetAccelerationStructureBuildSizesKHR",
    "vkGetAccelerationStructureDeviceAddressKHR",
    "vkGetAccelerationStructureHandleNV",
    "vkGetAccelerationStructureMemoryRequirementsNV",
    "vkGetAccelerationStructureOpaqueCaptureDescriptorDataEXT",
    "vkGetAndroidHardwareBufferPropertiesANDROID",
    "vkGetBufferCollectionPropertiesFUCHSIA",
    "vkGetBufferDeviceAddressEXT",
    "vkGetBufferDeviceAddressKHR",
    "vkGetBufferMemoryRequirements2KHR",
    "vkGetBufferOpaqueCaptureAddressKHR",
    "vkGetBufferOpaqueCaptureDescriptorDataEXT",
    "vkGetCalibratedTimestampsEXT",
    "vkGetCalibratedTimestampsKHR",
    "vkGetClusterAccelerationStructureBuildSizesNV",
    "vkGetCommandPoolMemoryConsumption",
    "vkGetCudaModuleCacheNV",
    "vkGetDataGraphPipelineAvailablePropertiesARM",
    "vkGetDataGraphPipelinePropertiesARM",
    "vkGetDataGraphPipelineSessionBindPointRequirementsARM",
    "vkGetDataGraphPipelineSessionMemoryRequirementsARM",
    "vkGetDeferredOperationMaxConcurrencyKHR",
    "vkGetDeferredOperationResultKHR",
    "vkGetDescriptorEXT",
    "vkGetDescriptorSetHostMappingVALVE",
    "vkGetDescriptorSetLayoutBindingOffsetEXT",
    "vkGetDescriptorSetLayoutHostMappingInfoVALVE",
    "vkGetDescriptorSetLayoutSizeEXT",
    "vkGetDescriptorSetLayoutSupportKHR",
    "vkGetDeviceAccelerationStructureCompatibilityKHR",
    "vkGetDeviceBufferMemoryRequirementsKHR",
    "vkGetDeviceCombinedImageSamplerIndexNVX",
    "vkGetDeviceFaultDebugInfoKHR",
    "vkGetDeviceFaultInfoEXT",
    "vkGetDeviceFaultReportsKHR",
    "vkGetDeviceGroupPeerMemoryFeaturesKHR",
    "vkGetDeviceGroupSurfacePresentModes2EXT",
    "vkGetDeviceImageMemoryRequirementsKHR",
    "vkGetDeviceImageSparseMemoryRequirementsKHR",
    "vkGetDeviceImageSubresourceLayoutKHR",
    "vkGetDeviceMemoryOpaqueCaptureAddressKHR",
    "vkGetDeviceMicromapCompatibilityEXT",
    "vkGetDeviceSubpassShadingMaxWorkgroupSizeHUAWEI",
    "vkGetDeviceTensorMemoryRequirementsARM",
    "vkGetDrmDisplayEXT",
    "vkGetDynamicRenderingTilePropertiesQCOM",
    "vkGetEncodedVideoSessionParametersKHR",
    "vkGetExecutionGraphPipelineNodeIndexAMDX",
    "vkGetExecutionGraphPipelineScratchSizeAMDX",
    "vkGetExternalComputeQueueDataNV",
    "vkGetFaultData",
    "vkGetFenceFdKHR",
    "vkGetFenceSciSyncFenceNV",
    "vkGetFenceSciSyncObjNV",
    "vkGetFenceWin32HandleKHR",
    "vkGetFramebufferTilePropertiesQCOM",
    "vkGetGeneratedCommandsMemoryRequirementsEXT",
    "vkGetGeneratedCommandsMemoryRequirementsNV",
    "vkGetGpaDeviceClockInfoAMD",
    "vkGetGpaSessionResultsAMD",
    "vkGetGpaSessionStatusAMD",
    "vkGetImageDrmFormatModifierPropertiesEXT",
    "vkGetImageMemoryRequirements2KHR",
    "vkGetImageOpaqueCaptureDataEXT",
    "vkGetImageOpaqueCaptureDescriptorDataEXT",
    "vkGetImageSparseMemoryRequirements2KHR",
    "vkGetImageSubresourceLayout2EXT",
    "vkGetImageSubresourceLayout2KHR",
    "vkGetImageViewAddressNVX",
    "vkGetImageViewHandle64NVX",
    "vkGetImageViewHandleNVX",
    "vkGetImageViewOpaqueCaptureDescriptorDataEXT",
    "vkGetLatencyTimingsLegacyNV",
    "vkGetLatencyTimingsNV",
    "vkGetMemoryAndroidHardwareBufferANDROID",
    "vkGetMemoryFdKHR",
    "vkGetMemoryFdPropertiesKHR",
    "vkGetMemoryHostPointerPropertiesEXT",
    "vkGetMemoryMetalHandleEXT",
    "vkGetMemoryMetalHandlePropertiesEXT",
    "vkGetMemoryNativeBufferOHOS",
    "vkGetMemoryRemoteAddressNV",
    "vkGetMemorySciBufNV",
    "vkGetMemoryWin32HandleKHR",
    "vkGetMemoryWin32HandleNV",
    "vkGetMemoryWin32HandlePropertiesKHR",
    "vkGetMemoryZirconHandleFUCHSIA",
    "vkGetMemoryZirconHandlePropertiesFUCHSIA",
    "vkGetMicromapBuildSizesEXT",
    "vkGetNativeBufferPropertiesOHOS",
    "vkGetPartitionedAccelerationStructuresBuildSizesNV",
    "vkGetPastPresentationTimingEXT",
    "vkGetPastPresentationTimingGOOGLE",
    "vkGetPerformanceParameterINTEL",
    "vkGetPhysicalDeviceCalibrateableTimeDomainsEXT",
    "vkGetPhysicalDeviceCalibrateableTimeDomainsKHR",
    "vkGetPhysicalDeviceCooperativeMatrixFlexibleDimensionsPropertiesNV",
    "vkGetPhysicalDeviceCooperativeMatrixProperties2EXT",
    "vkGetPhysicalDeviceCooperativeMatrixPropertiesKHR",
    "vkGetPhysicalDeviceCooperativeMatrixPropertiesNV",
    "vkGetPhysicalDeviceCooperativeVectorPropertiesNV",
    "vkGetPhysicalDeviceDescriptorSizeEXT",
    "vkGetPhysicalDeviceDirectFBPresentationSupportEXT",
    "vkGetPhysicalDeviceExternalBufferPropertiesKHR",
    "vkGetPhysicalDeviceExternalFencePropertiesKHR",
    "vkGetPhysicalDeviceExternalImageFormatPropertiesNV",
    "vkGetPhysicalDeviceExternalMemorySciBufPropertiesNV",
    "vkGetPhysicalDeviceExternalSemaphorePropertiesKHR",
    "vkGetPhysicalDeviceExternalTensorPropertiesARM",
    "vkGetPhysicalDeviceFeatures2KHR",
    "vkGetPhysicalDeviceFormatProperties2KHR",
    "vkGetPhysicalDeviceFragmentShadingRatesKHR",
    "vkGetPhysicalDeviceImageFormatProperties2KHR",
    "vkGetPhysicalDeviceMemoryProperties2KHR",
    "vkGetPhysicalDeviceMultisamplePropertiesEXT",
    "vkGetPhysicalDeviceOpticalFlowImageFormatsNV",
    "vkGetPhysicalDeviceProperties2KHR",
    "vkGetPhysicalDeviceQueueFamilyDataGraphEngineOperationPropertiesARM",
    "vkGetPhysicalDeviceQueueFamilyDataGraphOpticalFlowImageFormatsARM",
    "vkGetPhysicalDeviceQueueFamilyDataGraphProcessingEnginePropertiesARM",
    "vkGetPhysicalDeviceQueueFamilyDataGraphPropertiesARM",
    "vkGetPhysicalDeviceQueueFamilyPerformanceQueryPassesKHR",
    "vkGetPhysicalDeviceQueueFamilyProperties2KHR",
    "vkGetPhysicalDeviceRefreshableObjectTypesKHR",
    "vkGetPhysicalDeviceSciBufAttributesNV",
    "vkGetPhysicalDeviceSciSyncAttributesNV",
    "vkGetPhysicalDeviceScreenPresentationSupportQNX",
    "vkGetPhysicalDeviceSparseImageFormatProperties2KHR",
    "vkGetPhysicalDeviceSupportedFramebufferMixedSamplesCombinationsNV",
    "vkGetPhysicalDeviceSurfaceCapabilities2EXT",
    "vkGetPhysicalDeviceSurfacePresentModes2EXT",
    "vkGetPhysicalDeviceToolPropertiesEXT",
    "vkGetPhysicalDeviceUbmPresentationSupportSEC",
    "vkGetPhysicalDeviceVideoCapabilitiesKHR",
    "vkGetPhysicalDeviceVideoEncodeQualityLevelPropertiesKHR",
    "vkGetPhysicalDeviceVideoFormatPropertiesKHR",
    "vkGetPipelineBinaryDataKHR",
    "vkGetPipelineExecutableInternalRepresentationsKHR",
    "vkGetPipelineExecutablePropertiesKHR",
    "vkGetPipelineExecutableStatisticsKHR",
    "vkGetPipelineIndirectDeviceAddressNV",
    "vkGetPipelineIndirectMemoryRequirementsNV",
    "vkGetPipelineKeyKHR",
    "vkGetPipelinePropertiesEXT",
    "vkGetPrivateDataEXT",
    "vkGetQueueCheckpointData2NV",
    "vkGetQueueCheckpointDataNV",
    "vkGetRandROutputDisplayEXT",
    "vkGetRayTracingCaptureReplayShaderGroupHandlesKHR",
    "vkGetRayTracingShaderGroupHandlesKHR",
    "vkGetRayTracingShaderGroupHandlesNV",
    "vkGetRayTracingShaderGroupStackSizeKHR",
    "vkGetRefreshCycleDurationGOOGLE",
    "vkGetRenderingAreaGranularityKHR",
    "vkGetSamplerOpaqueCaptureDescriptorDataEXT",
    "vkGetScreenBufferPropertiesQNX",
    "vkGetSemaphoreCounterValueKHR",
    "vkGetSemaphoreFdKHR",
    "vkGetSemaphoreSciSyncObjNV",
    "vkGetSemaphoreWin32HandleKHR",
    "vkGetSemaphoreZirconHandleFUCHSIA",
    "vkGetShaderBinaryDataEXT",
    "vkGetShaderInfoAMD",
    "vkGetShaderInstrumentationValuesARM",
    "vkGetShaderModuleCreateInfoIdentifierEXT",
    "vkGetShaderModuleIdentifierEXT",
    "vkGetSleepStatusLegacyNV",
    "vkGetSwapchainCounterEXT",
    "vkGetSwapchainGrallocUsage2ANDROID",
    "vkGetSwapchainGrallocUsageANDROID",
    "vkGetSwapchainGrallocUsageOHOS",
    "vkGetSwapchainStatusKHR",
    "vkGetSwapchainTimeDomainPropertiesEXT",
    "vkGetSwapchainTimingPropertiesEXT",
    "vkGetTensorMemoryRequirementsARM",
    "vkGetTensorOpaqueCaptureDataARM",
    "vkGetTensorOpaqueCaptureDescriptorDataARM",
    "vkGetTensorViewOpaqueCaptureDescriptorDataARM",
    "vkGetValidationCacheDataEXT",
    "vkGetVideoSessionMemoryRequirementsKHR",
    "vkGetWinrtDisplayNV",
    "vkImportFenceFdKHR",
    "vkImportFenceSciSyncFenceNV",
    "vkImportFenceSciSyncObjNV",
    "vkImportFenceWin32HandleKHR",
    "vkImportSemaphoreFdKHR",
    "vkImportSemaphoreSciSyncObjNV",
    "vkImportSemaphoreWin32HandleKHR",
    "vkImportSemaphoreZirconHandleFUCHSIA",
    "vkInitializePerformanceApiINTEL",
    "vkLatencySleepLegacyNV",
    "vkLatencySleepNV",
    "vkMapMemory2KHR",
    "vkMergeValidationCachesEXT",
    "vkQueueBeginDebugUtilsLabelEXT",
    "vkQueueEndDebugUtilsLabelEXT",
    "vkQueueInsertDebugUtilsLabelEXT",
    "vkQueueNotifyOutOfBandLegacyNV",
    "vkQueueNotifyOutOfBandNV",
    "vkQueueSetPerfHintQCOM",
    "vkQueueSetPerformanceConfigurationINTEL",
    "vkQueueSignalReleaseImageANDROID",
    "vkQueueSignalReleaseImageOHOS",
    "vkQueueSubmit2KHR",
    "vkRegisterCustomBorderColorEXT",
    "vkRegisterDeviceEventEXT",
    "vkRegisterDisplayEventEXT",
    "vkReleaseCapturedPipelineDataKHR",
    "vkReleaseDisplayEXT",
    "vkReleaseFullScreenExclusiveModeEXT",
    "vkReleasePerformanceConfigurationINTEL",
    "vkReleaseProfilingLockKHR",
    "vkReleaseSwapchainImagesEXT",
    "vkReleaseSwapchainImagesKHR",
    "vkResetGpaSessionAMD",
    "vkResetQueryPoolEXT",
    "vkSetBufferCollectionBufferConstraintsFUCHSIA",
    "vkSetBufferCollectionImageConstraintsFUCHSIA",
    "vkSetDebugUtilsObjectNameEXT",
    "vkSetDebugUtilsObjectTagEXT",
    "vkSetDeviceMemoryPriorityEXT",
    "vkSetGpaDeviceClockModeAMD",
    "vkSetHdrMetadataEXT",
    "vkSetLatencyMarkerLegacyNV",
    "vkSetLatencyMarkerNV",
    "vkSetLatencySleepModeLegacyNV",
    "vkSetLatencySleepModeNV",
    "vkSetLocalDimmingAMD",
    "vkSetPrivateDataEXT",
    "vkSetSwapchainPresentTimingQueueSizeEXT",
    "vkShutdownLatencyDeviceLegacyNV",
    "vkSignalSemaphoreKHR",
    "vkSubmitDebugUtilsMessageEXT",
    "vkTransitionImageLayoutEXT",
    "vkTrimCommandPoolKHR",
    "vkUninitializePerformanceApiINTEL",
    "vkUnmapMemory2KHR",
    "vkUnregisterCustomBorderColorEXT",
    "vkUpdateDescriptorSetWithTemplateKHR",
    "vkUpdateIndirectExecutionSetPipelineEXT",
    "vkUpdateIndirectExecutionSetShaderEXT",
    "vkUpdateVideoSessionParametersKHR",
    "vkWaitForPresent2KHR",
    "vkWaitForPresentKHR",
    "vkWaitSemaphoresKHR",
    "vkWriteAccelerationStructuresPropertiesKHR",
    "vkWriteMicromapsPropertiesEXT",
    "vkWriteResourceDescriptorsEXT",
    "vkWriteSamplerDescriptorsEXT",
];

pub fn contains(name: &str) -> bool {
    matches!(
        name,
        "vkAcquireDrmDisplayEXT"
            | "vkAcquireFullScreenExclusiveModeEXT"
            | "vkAcquireImageANDROID"
            | "vkAcquireImageOHOS"
            | "vkAcquirePerformanceConfigurationINTEL"
            | "vkAcquireProfilingLockKHR"
            | "vkAcquireWinrtDisplayNV"
            | "vkAcquireXlibDisplayEXT"
            | "vkAntiLagUpdateAMD"
            | "vkBindAccelerationStructureMemoryNV"
            | "vkBindBufferMemory2KHR"
            | "vkBindDataGraphPipelineSessionMemoryARM"
            | "vkBindImageMemory2KHR"
            | "vkBindOpticalFlowSessionImageNV"
            | "vkBindTensorMemoryARM"
            | "vkBindVideoSessionMemoryKHR"
            | "vkBuildAccelerationStructuresKHR"
            | "vkBuildMicromapsEXT"
            | "vkClearShaderInstrumentationMetricsARM"
            | "vkCmdBeginConditionalRendering2EXT"
            | "vkCmdBeginConditionalRenderingEXT"
            | "vkCmdBeginCustomResolveEXT"
            | "vkCmdBeginDebugUtilsLabelEXT"
            | "vkCmdBeginGpaSampleAMD"
            | "vkCmdBeginGpaSessionAMD"
            | "vkCmdBeginPerTileExecutionQCOM"
            | "vkCmdBeginQueryIndexedEXT"
            | "vkCmdBeginRenderPass2KHR"
            | "vkCmdBeginRenderingKHR"
            | "vkCmdBeginShaderInstrumentationARM"
            | "vkCmdBeginTransformFeedback2EXT"
            | "vkCmdBeginTransformFeedbackEXT"
            | "vkCmdBeginVideoCodingKHR"
            | "vkCmdBindDescriptorBufferEmbeddedSamplers2EXT"
            | "vkCmdBindDescriptorBufferEmbeddedSamplersEXT"
            | "vkCmdBindDescriptorBuffersEXT"
            | "vkCmdBindDescriptorSets2KHR"
            | "vkCmdBindIndexBuffer2KHR"
            | "vkCmdBindIndexBuffer3KHR"
            | "vkCmdBindInvocationMaskHUAWEI"
            | "vkCmdBindPipelineShaderGroupNV"
            | "vkCmdBindResourceHeapEXT"
            | "vkCmdBindSamplerHeapEXT"
            | "vkCmdBindShadersEXT"
            | "vkCmdBindShadingRateImageNV"
            | "vkCmdBindTileMemoryQCOM"
            | "vkCmdBindTransformFeedbackBuffers2EXT"
            | "vkCmdBindTransformFeedbackBuffersEXT"
            | "vkCmdBindVertexBuffers2EXT"
            | "vkCmdBindVertexBuffers3KHR"
            | "vkCmdBlitImage2KHR"
            | "vkCmdBuildAccelerationStructureNV"
            | "vkCmdBuildAccelerationStructuresIndirectKHR"
            | "vkCmdBuildAccelerationStructuresKHR"
            | "vkCmdBuildClusterAccelerationStructureIndirectNV"
            | "vkCmdBuildMicromapsEXT"
            | "vkCmdBuildPartitionedAccelerationStructuresNV"
            | "vkCmdControlVideoCodingKHR"
            | "vkCmdConvertCooperativeVectorMatrixNV"
            | "vkCmdCopyAccelerationStructureKHR"
            | "vkCmdCopyAccelerationStructureNV"
            | "vkCmdCopyAccelerationStructureToMemoryKHR"
            | "vkCmdCopyBuffer2KHR"
            | "vkCmdCopyBufferToImage2KHR"
            | "vkCmdCopyGpaSessionResultsAMD"
            | "vkCmdCopyImage2KHR"
            | "vkCmdCopyImageToBuffer2KHR"
            | "vkCmdCopyImageToMemoryKHR"
            | "vkCmdCopyMemoryIndirectKHR"
            | "vkCmdCopyMemoryIndirectNV"
            | "vkCmdCopyMemoryKHR"
            | "vkCmdCopyMemoryToAccelerationStructureKHR"
            | "vkCmdCopyMemoryToImageIndirectKHR"
            | "vkCmdCopyMemoryToImageIndirectNV"
            | "vkCmdCopyMemoryToImageKHR"
            | "vkCmdCopyMemoryToMicromapEXT"
            | "vkCmdCopyMicromapEXT"
            | "vkCmdCopyMicromapToMemoryEXT"
            | "vkCmdCopyQueryPoolResultsToMemoryKHR"
            | "vkCmdCopyTensorARM"
            | "vkCmdCuLaunchKernelNVX"
            | "vkCmdCudaLaunchKernelNV"
            | "vkCmdDebugMarkerBeginEXT"
            | "vkCmdDebugMarkerEndEXT"
            | "vkCmdDebugMarkerInsertEXT"
            | "vkCmdDecodeVideoKHR"
            | "vkCmdDecompressMemoryEXT"
            | "vkCmdDecompressMemoryIndirectCountEXT"
            | "vkCmdDecompressMemoryIndirectCountNV"
            | "vkCmdDecompressMemoryNV"
            | "vkCmdDispatchBaseKHR"
            | "vkCmdDispatchDataGraphARM"
            | "vkCmdDispatchGraphAMDX"
            | "vkCmdDispatchGraphIndirectAMDX"
            | "vkCmdDispatchGraphIndirectCountAMDX"
            | "vkCmdDispatchIndirect2KHR"
            | "vkCmdDispatchTileQCOM"
            | "vkCmdDrawClusterHUAWEI"
            | "vkCmdDrawClusterIndirectHUAWEI"
            | "vkCmdDrawIndexedIndirect2KHR"
            | "vkCmdDrawIndexedIndirectCount2KHR"
            | "vkCmdDrawIndexedIndirectCountAMD"
            | "vkCmdDrawIndexedIndirectCountKHR"
            | "vkCmdDrawIndirect2KHR"
            | "vkCmdDrawIndirectByteCount2EXT"
            | "vkCmdDrawIndirectByteCountEXT"
            | "vkCmdDrawIndirectCount2KHR"
            | "vkCmdDrawIndirectCountAMD"
            | "vkCmdDrawIndirectCountKHR"
            | "vkCmdDrawMeshTasksEXT"
            | "vkCmdDrawMeshTasksIndirect2EXT"
            | "vkCmdDrawMeshTasksIndirectCount2EXT"
            | "vkCmdDrawMeshTasksIndirectCountEXT"
            | "vkCmdDrawMeshTasksIndirectCountNV"
            | "vkCmdDrawMeshTasksIndirectEXT"
            | "vkCmdDrawMeshTasksIndirectNV"
            | "vkCmdDrawMeshTasksNV"
            | "vkCmdDrawMultiEXT"
            | "vkCmdDrawMultiIndexedEXT"
            | "vkCmdEncodeVideoKHR"
            | "vkCmdEndConditionalRenderingEXT"
            | "vkCmdEndDebugUtilsLabelEXT"
            | "vkCmdEndGpaSampleAMD"
            | "vkCmdEndGpaSessionAMD"
            | "vkCmdEndPerTileExecutionQCOM"
            | "vkCmdEndQueryIndexedEXT"
            | "vkCmdEndRenderPass2KHR"
            | "vkCmdEndRendering2EXT"
            | "vkCmdEndRendering2KHR"
            | "vkCmdEndRenderingKHR"
            | "vkCmdEndShaderInstrumentationARM"
            | "vkCmdEndTransformFeedback2EXT"
            | "vkCmdEndTransformFeedbackEXT"
            | "vkCmdEndVideoCodingKHR"
            | "vkCmdExecuteGeneratedCommandsEXT"
            | "vkCmdExecuteGeneratedCommandsNV"
            | "vkCmdFillMemoryKHR"
            | "vkCmdInitializeGraphScratchMemoryAMDX"
            | "vkCmdInsertDebugUtilsLabelEXT"
            | "vkCmdNextSubpass2KHR"
            | "vkCmdOpticalFlowExecuteNV"
            | "vkCmdPipelineBarrier2KHR"
            | "vkCmdPreprocessGeneratedCommandsEXT"
            | "vkCmdPreprocessGeneratedCommandsNV"
            | "vkCmdPushConstants2KHR"
            | "vkCmdPushDataEXT"
            | "vkCmdPushDescriptorSet2KHR"
            | "vkCmdPushDescriptorSetKHR"
            | "vkCmdPushDescriptorSetWithTemplate2KHR"
            | "vkCmdPushDescriptorSetWithTemplateKHR"
            | "vkCmdRefreshObjectsKHR"
            | "vkCmdResetEvent2KHR"
            | "vkCmdResolveImage2KHR"
            | "vkCmdSetAlphaToCoverageEnableEXT"
            | "vkCmdSetAlphaToOneEnableEXT"
            | "vkCmdSetAttachmentFeedbackLoopEnableEXT"
            | "vkCmdSetCheckpointNV"
            | "vkCmdSetCoarseSampleOrderNV"
            | "vkCmdSetColorBlendAdvancedEXT"
            | "vkCmdSetColorBlendEnableEXT"
            | "vkCmdSetColorBlendEquationEXT"
            | "vkCmdSetColorWriteEnableEXT"
            | "vkCmdSetColorWriteMaskEXT"
            | "vkCmdSetComputeOccupancyPriorityNV"
            | "vkCmdSetConservativeRasterizationModeEXT"
            | "vkCmdSetCoverageModulationModeNV"
            | "vkCmdSetCoverageModulationTableEnableNV"
            | "vkCmdSetCoverageModulationTableNV"
            | "vkCmdSetCoverageReductionModeNV"
            | "vkCmdSetCoverageToColorEnableNV"
            | "vkCmdSetCoverageToColorLocationNV"
            | "vkCmdSetCullModeEXT"
            | "vkCmdSetDepthBias2EXT"
            | "vkCmdSetDepthBiasEnableEXT"
            | "vkCmdSetDepthBoundsTestEnableEXT"
            | "vkCmdSetDepthClampEnableEXT"
            | "vkCmdSetDepthClampRangeEXT"
            | "vkCmdSetDepthClipEnableEXT"
            | "vkCmdSetDepthClipNegativeOneToOneEXT"
            | "vkCmdSetDepthCompareOpEXT"
            | "vkCmdSetDepthTestEnableEXT"
            | "vkCmdSetDepthWriteEnableEXT"
            | "vkCmdSetDescriptorBufferOffsets2EXT"
            | "vkCmdSetDescriptorBufferOffsetsEXT"
            | "vkCmdSetDeviceMaskKHR"
            | "vkCmdSetDiscardRectangleEXT"
            | "vkCmdSetDiscardRectangleEnableEXT"
            | "vkCmdSetDiscardRectangleModeEXT"
            | "vkCmdSetDispatchParametersARM"
            | "vkCmdSetEvent2KHR"
            | "vkCmdSetExclusiveScissorEnableNV"
            | "vkCmdSetExclusiveScissorNV"
            | "vkCmdSetExtraPrimitiveOverestimationSizeEXT"
            | "vkCmdSetFragmentShadingRateEnumNV"
            | "vkCmdSetFragmentShadingRateKHR"
            | "vkCmdSetFrontFaceEXT"
            | "vkCmdSetLineRasterizationModeEXT"
            | "vkCmdSetLineStippleEXT"
            | "vkCmdSetLineStippleEnableEXT"
            | "vkCmdSetLineStippleKHR"
            | "vkCmdSetLogicOpEXT"
            | "vkCmdSetLogicOpEnableEXT"
            | "vkCmdSetPatchControlPointsEXT"
            | "vkCmdSetPerformanceMarkerINTEL"
            | "vkCmdSetPerformanceOverrideINTEL"
            | "vkCmdSetPerformanceStreamMarkerINTEL"
            | "vkCmdSetPolygonModeEXT"
            | "vkCmdSetPrimitiveRestartEnableEXT"
            | "vkCmdSetPrimitiveRestartIndexEXT"
            | "vkCmdSetPrimitiveTopologyEXT"
            | "vkCmdSetProvokingVertexModeEXT"
            | "vkCmdSetRasterizationSamplesEXT"
            | "vkCmdSetRasterizationStreamEXT"
            | "vkCmdSetRasterizerDiscardEnableEXT"
            | "vkCmdSetRayTracingPipelineStackSizeKHR"
            | "vkCmdSetRenderingAttachmentLocationsKHR"
            | "vkCmdSetRenderingInputAttachmentIndicesKHR"
            | "vkCmdSetRepresentativeFragmentTestEnableNV"
            | "vkCmdSetSampleLocationsEXT"
            | "vkCmdSetSampleLocationsEnableEXT"
            | "vkCmdSetSampleMaskEXT"
            | "vkCmdSetScissorWithCountEXT"
            | "vkCmdSetShadingRateImageEnableNV"
            | "vkCmdSetStencilOpEXT"
            | "vkCmdSetStencilTestEnableEXT"
            | "vkCmdSetTessellationDomainOriginEXT"
            | "vkCmdSetVertexInputEXT"
            | "vkCmdSetViewportShadingRatePaletteNV"
            | "vkCmdSetViewportSwizzleNV"
            | "vkCmdSetViewportWScalingEnableNV"
            | "vkCmdSetViewportWScalingNV"
            | "vkCmdSetViewportWithCountEXT"
            | "vkCmdSubpassShadingHUAWEI"
            | "vkCmdTraceRaysIndirect2KHR"
            | "vkCmdTraceRaysIndirectKHR"
            | "vkCmdTraceRaysKHR"
            | "vkCmdTraceRaysNV"
            | "vkCmdUpdateMemoryKHR"
            | "vkCmdUpdatePipelineIndirectBufferNV"
            | "vkCmdWaitEvents2KHR"
            | "vkCmdWriteAccelerationStructuresPropertiesKHR"
            | "vkCmdWriteAccelerationStructuresPropertiesNV"
            | "vkCmdWriteBufferMarker2AMD"
            | "vkCmdWriteBufferMarkerAMD"
            | "vkCmdWriteMarkerToMemoryAMD"
            | "vkCmdWriteMicromapsPropertiesEXT"
            | "vkCmdWriteTimestamp2KHR"
            | "vkCompileDeferredNV"
            | "vkConvertCooperativeVectorMatrixNV"
            | "vkCopyAccelerationStructureKHR"
            | "vkCopyAccelerationStructureToMemoryKHR"
            | "vkCopyImageToImageEXT"
            | "vkCopyImageToMemoryEXT"
            | "vkCopyMemoryToAccelerationStructureKHR"
            | "vkCopyMemoryToImageEXT"
            | "vkCopyMemoryToMicromapEXT"
            | "vkCopyMicromapEXT"
            | "vkCopyMicromapToMemoryEXT"
            | "vkCreateAccelerationStructure2KHR"
            | "vkCreateAccelerationStructureKHR"
            | "vkCreateAccelerationStructureNV"
            | "vkCreateAndroidSurfaceKHR"
            | "vkCreateBufferCollectionFUCHSIA"
            | "vkCreateCuFunctionNVX"
            | "vkCreateCuModuleNVX"
            | "vkCreateCudaFunctionNV"
            | "vkCreateCudaModuleNV"
            | "vkCreateDataGraphPipelineSessionARM"
            | "vkCreateDataGraphPipelinesARM"
            | "vkCreateDeferredOperationKHR"
            | "vkCreateDescriptorUpdateTemplateKHR"
            | "vkCreateDirectFBSurfaceEXT"
            | "vkCreateExecutionGraphPipelinesAMDX"
            | "vkCreateExternalComputeQueueNV"
            | "vkCreateGpaSessionAMD"
            | "vkCreateIOSSurfaceMVK"
            | "vkCreateImagePipeSurfaceFUCHSIA"
            | "vkCreateIndirectCommandsLayoutEXT"
            | "vkCreateIndirectCommandsLayoutNV"
            | "vkCreateIndirectExecutionSetEXT"
            | "vkCreateMacOSSurfaceMVK"
            | "vkCreateMetalSurfaceEXT"
            | "vkCreateMicromapEXT"
            | "vkCreateOpticalFlowSessionNV"
            | "vkCreatePipelineBinariesKHR"
            | "vkCreatePrivateDataSlotEXT"
            | "vkCreateRayTracingPipelinesKHR"
            | "vkCreateRayTracingPipelinesNV"
            | "vkCreateRenderPass2KHR"
            | "vkCreateSamplerYcbcrConversionKHR"
            | "vkCreateScreenSurfaceQNX"
            | "vkCreateSemaphoreSciSyncPoolNV"
            | "vkCreateShaderInstrumentationARM"
            | "vkCreateShadersEXT"
            | "vkCreateStreamDescriptorSurfaceGGP"
            | "vkCreateSurfaceOHOS"
            | "vkCreateTensorARM"
            | "vkCreateTensorViewARM"
            | "vkCreateUbmSurfaceSEC"
            | "vkCreateValidationCacheEXT"
            | "vkCreateViSurfaceNN"
            | "vkCreateVideoSessionKHR"
            | "vkCreateVideoSessionParametersKHR"
            | "vkDebugMarkerSetObjectNameEXT"
            | "vkDebugMarkerSetObjectTagEXT"
            | "vkDebugReportMessageEXT"
            | "vkDeferredOperationJoinKHR"
            | "vkDestroyAccelerationStructureKHR"
            | "vkDestroyAccelerationStructureNV"
            | "vkDestroyBufferCollectionFUCHSIA"
            | "vkDestroyCuFunctionNVX"
            | "vkDestroyCuModuleNVX"
            | "vkDestroyCudaFunctionNV"
            | "vkDestroyCudaModuleNV"
            | "vkDestroyDataGraphPipelineSessionARM"
            | "vkDestroyDebugReportCallbackEXT"
            | "vkDestroyDebugUtilsMessengerEXT"
            | "vkDestroyDeferredOperationKHR"
            | "vkDestroyDescriptorUpdateTemplateKHR"
            | "vkDestroyExternalComputeQueueNV"
            | "vkDestroyGpaSessionAMD"
            | "vkDestroyIndirectCommandsLayoutEXT"
            | "vkDestroyIndirectCommandsLayoutNV"
            | "vkDestroyIndirectExecutionSetEXT"
            | "vkDestroyMicromapEXT"
            | "vkDestroyOpticalFlowSessionNV"
            | "vkDestroyPipelineBinaryKHR"
            | "vkDestroyPrivateDataSlotEXT"
            | "vkDestroySamplerYcbcrConversionKHR"
            | "vkDestroySemaphoreSciSyncPoolNV"
            | "vkDestroyShaderEXT"
            | "vkDestroyShaderInstrumentationARM"
            | "vkDestroyTensorARM"
            | "vkDestroyTensorViewARM"
            | "vkDestroyValidationCacheEXT"
            | "vkDestroyVideoSessionKHR"
            | "vkDestroyVideoSessionParametersKHR"
            | "vkDisplayPowerControlEXT"
            | "vkEnumeratePhysicalDeviceGroupsKHR"
            | "vkEnumeratePhysicalDeviceQueueFamilyPerformanceCountersByRegionARM"
            | "vkEnumeratePhysicalDeviceQueueFamilyPerformanceQueryCountersKHR"
            | "vkEnumeratePhysicalDeviceShaderInstrumentationMetricsARM"
            | "vkExportMetalObjectsEXT"
            | "vkGetAccelerationStructureBuildSizesKHR"
            | "vkGetAccelerationStructureDeviceAddressKHR"
            | "vkGetAccelerationStructureHandleNV"
            | "vkGetAccelerationStructureMemoryRequirementsNV"
            | "vkGetAccelerationStructureOpaqueCaptureDescriptorDataEXT"
            | "vkGetAndroidHardwareBufferPropertiesANDROID"
            | "vkGetBufferCollectionPropertiesFUCHSIA"
            | "vkGetBufferDeviceAddressEXT"
            | "vkGetBufferDeviceAddressKHR"
            | "vkGetBufferMemoryRequirements2KHR"
            | "vkGetBufferOpaqueCaptureAddressKHR"
            | "vkGetBufferOpaqueCaptureDescriptorDataEXT"
            | "vkGetCalibratedTimestampsEXT"
            | "vkGetCalibratedTimestampsKHR"
            | "vkGetClusterAccelerationStructureBuildSizesNV"
            | "vkGetCommandPoolMemoryConsumption"
            | "vkGetCudaModuleCacheNV"
            | "vkGetDataGraphPipelineAvailablePropertiesARM"
            | "vkGetDataGraphPipelinePropertiesARM"
            | "vkGetDataGraphPipelineSessionBindPointRequirementsARM"
            | "vkGetDataGraphPipelineSessionMemoryRequirementsARM"
            | "vkGetDeferredOperationMaxConcurrencyKHR"
            | "vkGetDeferredOperationResultKHR"
            | "vkGetDescriptorEXT"
            | "vkGetDescriptorSetHostMappingVALVE"
            | "vkGetDescriptorSetLayoutBindingOffsetEXT"
            | "vkGetDescriptorSetLayoutHostMappingInfoVALVE"
            | "vkGetDescriptorSetLayoutSizeEXT"
            | "vkGetDescriptorSetLayoutSupportKHR"
            | "vkGetDeviceAccelerationStructureCompatibilityKHR"
            | "vkGetDeviceBufferMemoryRequirementsKHR"
            | "vkGetDeviceCombinedImageSamplerIndexNVX"
            | "vkGetDeviceFaultDebugInfoKHR"
            | "vkGetDeviceFaultInfoEXT"
            | "vkGetDeviceFaultReportsKHR"
            | "vkGetDeviceGroupPeerMemoryFeaturesKHR"
            | "vkGetDeviceGroupSurfacePresentModes2EXT"
            | "vkGetDeviceImageMemoryRequirementsKHR"
            | "vkGetDeviceImageSparseMemoryRequirementsKHR"
            | "vkGetDeviceImageSubresourceLayoutKHR"
            | "vkGetDeviceMemoryOpaqueCaptureAddressKHR"
            | "vkGetDeviceMicromapCompatibilityEXT"
            | "vkGetDeviceSubpassShadingMaxWorkgroupSizeHUAWEI"
            | "vkGetDeviceTensorMemoryRequirementsARM"
            | "vkGetDrmDisplayEXT"
            | "vkGetDynamicRenderingTilePropertiesQCOM"
            | "vkGetEncodedVideoSessionParametersKHR"
            | "vkGetExecutionGraphPipelineNodeIndexAMDX"
            | "vkGetExecutionGraphPipelineScratchSizeAMDX"
            | "vkGetExternalComputeQueueDataNV"
            | "vkGetFaultData"
            | "vkGetFenceFdKHR"
            | "vkGetFenceSciSyncFenceNV"
            | "vkGetFenceSciSyncObjNV"
            | "vkGetFenceWin32HandleKHR"
            | "vkGetFramebufferTilePropertiesQCOM"
            | "vkGetGeneratedCommandsMemoryRequirementsEXT"
            | "vkGetGeneratedCommandsMemoryRequirementsNV"
            | "vkGetGpaDeviceClockInfoAMD"
            | "vkGetGpaSessionResultsAMD"
            | "vkGetGpaSessionStatusAMD"
            | "vkGetImageDrmFormatModifierPropertiesEXT"
            | "vkGetImageMemoryRequirements2KHR"
            | "vkGetImageOpaqueCaptureDataEXT"
            | "vkGetImageOpaqueCaptureDescriptorDataEXT"
            | "vkGetImageSparseMemoryRequirements2KHR"
            | "vkGetImageSubresourceLayout2EXT"
            | "vkGetImageSubresourceLayout2KHR"
            | "vkGetImageViewAddressNVX"
            | "vkGetImageViewHandle64NVX"
            | "vkGetImageViewHandleNVX"
            | "vkGetImageViewOpaqueCaptureDescriptorDataEXT"
            | "vkGetLatencyTimingsLegacyNV"
            | "vkGetLatencyTimingsNV"
            | "vkGetMemoryAndroidHardwareBufferANDROID"
            | "vkGetMemoryFdKHR"
            | "vkGetMemoryFdPropertiesKHR"
            | "vkGetMemoryHostPointerPropertiesEXT"
            | "vkGetMemoryMetalHandleEXT"
            | "vkGetMemoryMetalHandlePropertiesEXT"
            | "vkGetMemoryNativeBufferOHOS"
            | "vkGetMemoryRemoteAddressNV"
            | "vkGetMemorySciBufNV"
            | "vkGetMemoryWin32HandleKHR"
            | "vkGetMemoryWin32HandleNV"
            | "vkGetMemoryWin32HandlePropertiesKHR"
            | "vkGetMemoryZirconHandleFUCHSIA"
            | "vkGetMemoryZirconHandlePropertiesFUCHSIA"
            | "vkGetMicromapBuildSizesEXT"
            | "vkGetNativeBufferPropertiesOHOS"
            | "vkGetPartitionedAccelerationStructuresBuildSizesNV"
            | "vkGetPastPresentationTimingEXT"
            | "vkGetPastPresentationTimingGOOGLE"
            | "vkGetPerformanceParameterINTEL"
            | "vkGetPhysicalDeviceCalibrateableTimeDomainsEXT"
            | "vkGetPhysicalDeviceCalibrateableTimeDomainsKHR"
            | "vkGetPhysicalDeviceCooperativeMatrixFlexibleDimensionsPropertiesNV"
            | "vkGetPhysicalDeviceCooperativeMatrixProperties2EXT"
            | "vkGetPhysicalDeviceCooperativeMatrixPropertiesKHR"
            | "vkGetPhysicalDeviceCooperativeMatrixPropertiesNV"
            | "vkGetPhysicalDeviceCooperativeVectorPropertiesNV"
            | "vkGetPhysicalDeviceDescriptorSizeEXT"
            | "vkGetPhysicalDeviceDirectFBPresentationSupportEXT"
            | "vkGetPhysicalDeviceExternalBufferPropertiesKHR"
            | "vkGetPhysicalDeviceExternalFencePropertiesKHR"
            | "vkGetPhysicalDeviceExternalImageFormatPropertiesNV"
            | "vkGetPhysicalDeviceExternalMemorySciBufPropertiesNV"
            | "vkGetPhysicalDeviceExternalSemaphorePropertiesKHR"
            | "vkGetPhysicalDeviceExternalTensorPropertiesARM"
            | "vkGetPhysicalDeviceFeatures2KHR"
            | "vkGetPhysicalDeviceFormatProperties2KHR"
            | "vkGetPhysicalDeviceFragmentShadingRatesKHR"
            | "vkGetPhysicalDeviceImageFormatProperties2KHR"
            | "vkGetPhysicalDeviceMemoryProperties2KHR"
            | "vkGetPhysicalDeviceMultisamplePropertiesEXT"
            | "vkGetPhysicalDeviceOpticalFlowImageFormatsNV"
            | "vkGetPhysicalDeviceProperties2KHR"
            | "vkGetPhysicalDeviceQueueFamilyDataGraphEngineOperationPropertiesARM"
            | "vkGetPhysicalDeviceQueueFamilyDataGraphOpticalFlowImageFormatsARM"
            | "vkGetPhysicalDeviceQueueFamilyDataGraphProcessingEnginePropertiesARM"
            | "vkGetPhysicalDeviceQueueFamilyDataGraphPropertiesARM"
            | "vkGetPhysicalDeviceQueueFamilyPerformanceQueryPassesKHR"
            | "vkGetPhysicalDeviceQueueFamilyProperties2KHR"
            | "vkGetPhysicalDeviceRefreshableObjectTypesKHR"
            | "vkGetPhysicalDeviceSciBufAttributesNV"
            | "vkGetPhysicalDeviceSciSyncAttributesNV"
            | "vkGetPhysicalDeviceScreenPresentationSupportQNX"
            | "vkGetPhysicalDeviceSparseImageFormatProperties2KHR"
            | "vkGetPhysicalDeviceSupportedFramebufferMixedSamplesCombinationsNV"
            | "vkGetPhysicalDeviceSurfaceCapabilities2EXT"
            | "vkGetPhysicalDeviceSurfacePresentModes2EXT"
            | "vkGetPhysicalDeviceToolPropertiesEXT"
            | "vkGetPhysicalDeviceUbmPresentationSupportSEC"
            | "vkGetPhysicalDeviceVideoCapabilitiesKHR"
            | "vkGetPhysicalDeviceVideoEncodeQualityLevelPropertiesKHR"
            | "vkGetPhysicalDeviceVideoFormatPropertiesKHR"
            | "vkGetPipelineBinaryDataKHR"
            | "vkGetPipelineExecutableInternalRepresentationsKHR"
            | "vkGetPipelineExecutablePropertiesKHR"
            | "vkGetPipelineExecutableStatisticsKHR"
            | "vkGetPipelineIndirectDeviceAddressNV"
            | "vkGetPipelineIndirectMemoryRequirementsNV"
            | "vkGetPipelineKeyKHR"
            | "vkGetPipelinePropertiesEXT"
            | "vkGetPrivateDataEXT"
            | "vkGetQueueCheckpointData2NV"
            | "vkGetQueueCheckpointDataNV"
            | "vkGetRandROutputDisplayEXT"
            | "vkGetRayTracingCaptureReplayShaderGroupHandlesKHR"
            | "vkGetRayTracingShaderGroupHandlesKHR"
            | "vkGetRayTracingShaderGroupHandlesNV"
            | "vkGetRayTracingShaderGroupStackSizeKHR"
            | "vkGetRefreshCycleDurationGOOGLE"
            | "vkGetRenderingAreaGranularityKHR"
            | "vkGetSamplerOpaqueCaptureDescriptorDataEXT"
            | "vkGetScreenBufferPropertiesQNX"
            | "vkGetSemaphoreCounterValueKHR"
            | "vkGetSemaphoreFdKHR"
            | "vkGetSemaphoreSciSyncObjNV"
            | "vkGetSemaphoreWin32HandleKHR"
            | "vkGetSemaphoreZirconHandleFUCHSIA"
            | "vkGetShaderBinaryDataEXT"
            | "vkGetShaderInfoAMD"
            | "vkGetShaderInstrumentationValuesARM"
            | "vkGetShaderModuleCreateInfoIdentifierEXT"
            | "vkGetShaderModuleIdentifierEXT"
            | "vkGetSleepStatusLegacyNV"
            | "vkGetSwapchainCounterEXT"
            | "vkGetSwapchainGrallocUsage2ANDROID"
            | "vkGetSwapchainGrallocUsageANDROID"
            | "vkGetSwapchainGrallocUsageOHOS"
            | "vkGetSwapchainStatusKHR"
            | "vkGetSwapchainTimeDomainPropertiesEXT"
            | "vkGetSwapchainTimingPropertiesEXT"
            | "vkGetTensorMemoryRequirementsARM"
            | "vkGetTensorOpaqueCaptureDataARM"
            | "vkGetTensorOpaqueCaptureDescriptorDataARM"
            | "vkGetTensorViewOpaqueCaptureDescriptorDataARM"
            | "vkGetValidationCacheDataEXT"
            | "vkGetVideoSessionMemoryRequirementsKHR"
            | "vkGetWinrtDisplayNV"
            | "vkImportFenceFdKHR"
            | "vkImportFenceSciSyncFenceNV"
            | "vkImportFenceSciSyncObjNV"
            | "vkImportFenceWin32HandleKHR"
            | "vkImportSemaphoreFdKHR"
            | "vkImportSemaphoreSciSyncObjNV"
            | "vkImportSemaphoreWin32HandleKHR"
            | "vkImportSemaphoreZirconHandleFUCHSIA"
            | "vkInitializePerformanceApiINTEL"
            | "vkLatencySleepLegacyNV"
            | "vkLatencySleepNV"
            | "vkMapMemory2KHR"
            | "vkMergeValidationCachesEXT"
            | "vkQueueBeginDebugUtilsLabelEXT"
            | "vkQueueEndDebugUtilsLabelEXT"
            | "vkQueueInsertDebugUtilsLabelEXT"
            | "vkQueueNotifyOutOfBandLegacyNV"
            | "vkQueueNotifyOutOfBandNV"
            | "vkQueueSetPerfHintQCOM"
            | "vkQueueSetPerformanceConfigurationINTEL"
            | "vkQueueSignalReleaseImageANDROID"
            | "vkQueueSignalReleaseImageOHOS"
            | "vkQueueSubmit2KHR"
            | "vkRegisterCustomBorderColorEXT"
            | "vkRegisterDeviceEventEXT"
            | "vkRegisterDisplayEventEXT"
            | "vkReleaseCapturedPipelineDataKHR"
            | "vkReleaseDisplayEXT"
            | "vkReleaseFullScreenExclusiveModeEXT"
            | "vkReleasePerformanceConfigurationINTEL"
            | "vkReleaseProfilingLockKHR"
            | "vkReleaseSwapchainImagesEXT"
            | "vkReleaseSwapchainImagesKHR"
            | "vkResetGpaSessionAMD"
            | "vkResetQueryPoolEXT"
            | "vkSetBufferCollectionBufferConstraintsFUCHSIA"
            | "vkSetBufferCollectionImageConstraintsFUCHSIA"
            | "vkSetDebugUtilsObjectNameEXT"
            | "vkSetDebugUtilsObjectTagEXT"
            | "vkSetDeviceMemoryPriorityEXT"
            | "vkSetGpaDeviceClockModeAMD"
            | "vkSetHdrMetadataEXT"
            | "vkSetLatencyMarkerLegacyNV"
            | "vkSetLatencyMarkerNV"
            | "vkSetLatencySleepModeLegacyNV"
            | "vkSetLatencySleepModeNV"
            | "vkSetLocalDimmingAMD"
            | "vkSetPrivateDataEXT"
            | "vkSetSwapchainPresentTimingQueueSizeEXT"
            | "vkShutdownLatencyDeviceLegacyNV"
            | "vkSignalSemaphoreKHR"
            | "vkSubmitDebugUtilsMessageEXT"
            | "vkTransitionImageLayoutEXT"
            | "vkTrimCommandPoolKHR"
            | "vkUninitializePerformanceApiINTEL"
            | "vkUnmapMemory2KHR"
            | "vkUnregisterCustomBorderColorEXT"
            | "vkUpdateDescriptorSetWithTemplateKHR"
            | "vkUpdateIndirectExecutionSetPipelineEXT"
            | "vkUpdateIndirectExecutionSetShaderEXT"
            | "vkUpdateVideoSessionParametersKHR"
            | "vkWaitForPresent2KHR"
            | "vkWaitForPresentKHR"
            | "vkWaitSemaphoresKHR"
            | "vkWriteAccelerationStructuresPropertiesKHR"
            | "vkWriteMicromapsPropertiesEXT"
            | "vkWriteResourceDescriptorsEXT"
            | "vkWriteSamplerDescriptorsEXT"
    )
}

pub fn lookup_instance(instance: VkInstance, name: &str) -> Option<usize> {
    if !contains(name) {
        return None;
    }
    let address = crate::host::instance_symbol(instance, name)?;
    bind(name, address)
}

pub fn lookup_device(device: VkDevice, name: &str) -> Option<usize> {
    if !contains(name) {
        return None;
    }
    let address = crate::host::device_symbol(device, name)?;
    bind(name, address)
}

pub fn reverse_lookup(name: &str, address: usize) -> Option<usize> {
    use crate::reverse_abi::{ArgumentClass, for_target};
    let arguments: &[ArgumentClass] = match name {
        "vkAcquireDrmDisplayEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkAcquireFullScreenExclusiveModeEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkAcquireImageANDROID" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkAcquireImageOHOS" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkAcquireNextImage2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkAcquireNextImageKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkAcquirePerformanceConfigurationINTEL" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkAcquireProfilingLockKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkAcquireWinrtDisplayNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkAcquireXlibDisplayEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkAllocateCommandBuffers" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkAllocateDescriptorSets" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkAllocateMemory" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkAntiLagUpdateAMD" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkBeginCommandBuffer" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkBindAccelerationStructureMemoryNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkBindBufferMemory" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkBindBufferMemory2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkBindBufferMemory2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkBindDataGraphPipelineSessionMemoryARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkBindImageMemory" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkBindImageMemory2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkBindImageMemory2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkBindOpticalFlowSessionImageNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkBindTensorMemoryARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkBindVideoSessionMemoryKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkBuildAccelerationStructuresKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkBuildMicromapsEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkClearShaderInstrumentationMetricsARM" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkCmdBeginConditionalRendering2EXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdBeginConditionalRenderingEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdBeginCustomResolveEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdBeginDebugUtilsLabelEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdBeginGpaSampleAMD" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBeginGpaSessionAMD" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdBeginPerTileExecutionQCOM" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdBeginQuery" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBeginQueryIndexedEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBeginRenderPass" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBeginRenderPass2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBeginRenderPass2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBeginRendering" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdBeginRenderingKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdBeginShaderInstrumentationARM" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdBeginTransformFeedback2EXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBeginTransformFeedbackEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBeginVideoCodingKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdBindDescriptorBufferEmbeddedSamplers2EXT" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkCmdBindDescriptorBufferEmbeddedSamplersEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBindDescriptorBuffersEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBindDescriptorSets" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBindDescriptorSets2" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdBindDescriptorSets2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdBindIndexBuffer" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBindIndexBuffer2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBindIndexBuffer2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBindIndexBuffer3KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdBindInvocationMaskHUAWEI" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBindPipeline" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBindPipelineShaderGroupNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBindResourceHeapEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdBindSamplerHeapEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdBindShadersEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBindShadingRateImageNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBindTileMemoryQCOM" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdBindTransformFeedbackBuffers2EXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBindTransformFeedbackBuffersEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBindVertexBuffers" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBindVertexBuffers2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBindVertexBuffers2EXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBindVertexBuffers3KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBlitImage" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBlitImage2" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdBlitImage2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdBuildAccelerationStructureNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBuildAccelerationStructuresIndirectKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBuildAccelerationStructuresKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBuildClusterAccelerationStructureIndirectNV" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkCmdBuildMicromapsEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdBuildPartitionedAccelerationStructuresNV" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkCmdClearAttachments" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdClearColorImage" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdClearDepthStencilImage" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdControlVideoCodingKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdConvertCooperativeVectorMatrixNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdCopyAccelerationStructureKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCopyAccelerationStructureNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdCopyAccelerationStructureToMemoryKHR" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkCmdCopyBuffer" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdCopyBuffer2" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCopyBuffer2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCopyBufferToImage" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdCopyBufferToImage2" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCopyBufferToImage2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCopyGpaSessionResultsAMD" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCopyImage" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdCopyImage2" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCopyImage2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCopyImageToBuffer" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdCopyImageToBuffer2" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCopyImageToBuffer2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCopyImageToMemoryKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCopyMemoryIndirectKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCopyMemoryIndirectNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdCopyMemoryKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCopyMemoryToAccelerationStructureKHR" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkCmdCopyMemoryToImageIndirectKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCopyMemoryToImageIndirectNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdCopyMemoryToImageKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCopyMemoryToMicromapEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCopyMicromapEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCopyMicromapToMemoryEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCopyQueryPoolResults" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdCopyQueryPoolResultsToMemoryKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdCopyTensorARM" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCuLaunchKernelNVX" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdCudaLaunchKernelNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdDebugMarkerBeginEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdDebugMarkerEndEXT" => &[ArgumentClass::Integer],
        "vkCmdDebugMarkerInsertEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdDecodeVideoKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdDecompressMemoryEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdDecompressMemoryIndirectCountEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDecompressMemoryIndirectCountNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDecompressMemoryNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDispatch" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDispatchBase" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDispatchBaseKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDispatchDataGraphARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDispatchGraphAMDX" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDispatchGraphIndirectAMDX" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDispatchGraphIndirectCountAMDX" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDispatchIndirect" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDispatchIndirect2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdDispatchTileQCOM" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdDraw" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawClusterHUAWEI" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawClusterIndirectHUAWEI" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawIndexed" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawIndexedIndirect" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawIndexedIndirect2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdDrawIndexedIndirectCount" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawIndexedIndirectCount2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdDrawIndexedIndirectCountAMD" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawIndexedIndirectCountKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawIndirect" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawIndirect2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdDrawIndirectByteCount2EXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawIndirectByteCountEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawIndirectCount" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawIndirectCount2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdDrawIndirectCountAMD" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawIndirectCountKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawMeshTasksEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawMeshTasksIndirect2EXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdDrawMeshTasksIndirectCount2EXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdDrawMeshTasksIndirectCountEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawMeshTasksIndirectCountNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawMeshTasksIndirectEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawMeshTasksIndirectNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawMeshTasksNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawMultiEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdDrawMultiIndexedEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdEncodeVideoKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdEndConditionalRenderingEXT" => &[ArgumentClass::Integer],
        "vkCmdEndDebugUtilsLabelEXT" => &[ArgumentClass::Integer],
        "vkCmdEndGpaSampleAMD" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdEndGpaSessionAMD" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdEndPerTileExecutionQCOM" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdEndQuery" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdEndQueryIndexedEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdEndRenderPass" => &[ArgumentClass::Integer],
        "vkCmdEndRenderPass2" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdEndRenderPass2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdEndRendering" => &[ArgumentClass::Integer],
        "vkCmdEndRendering2EXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdEndRendering2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdEndRenderingKHR" => &[ArgumentClass::Integer],
        "vkCmdEndShaderInstrumentationARM" => &[ArgumentClass::Integer],
        "vkCmdEndTransformFeedback2EXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdEndTransformFeedbackEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdEndVideoCodingKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdExecuteCommands" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdExecuteGeneratedCommandsEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdExecuteGeneratedCommandsNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdFillBuffer" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdFillMemoryKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdInitializeGraphScratchMemoryAMDX" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdInsertDebugUtilsLabelEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdNextSubpass" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdNextSubpass2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdNextSubpass2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdOpticalFlowExecuteNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdPipelineBarrier" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdPipelineBarrier2" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdPipelineBarrier2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdPreprocessGeneratedCommandsEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdPreprocessGeneratedCommandsNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdPushConstants" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdPushConstants2" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdPushConstants2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdPushDataEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdPushDescriptorSet" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdPushDescriptorSet2" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdPushDescriptorSet2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdPushDescriptorSetKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdPushDescriptorSetWithTemplate" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdPushDescriptorSetWithTemplate2" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdPushDescriptorSetWithTemplate2KHR" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkCmdPushDescriptorSetWithTemplateKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdRefreshObjectsKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdResetEvent" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdResetEvent2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdResetEvent2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdResetQueryPool" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdResolveImage" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdResolveImage2" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdResolveImage2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetAlphaToCoverageEnableEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetAlphaToOneEnableEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetAttachmentFeedbackLoopEnableEXT" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkCmdSetBlendConstants" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetCheckpointNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetCoarseSampleOrderNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetColorBlendAdvancedEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetColorBlendEnableEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetColorBlendEquationEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetColorWriteEnableEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetColorWriteMaskEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetComputeOccupancyPriorityNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetConservativeRasterizationModeEXT" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkCmdSetCoverageModulationModeNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetCoverageModulationTableEnableNV" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkCmdSetCoverageModulationTableNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetCoverageReductionModeNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetCoverageToColorEnableNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetCoverageToColorLocationNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetCullMode" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetCullModeEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDepthBias" => &[
            ArgumentClass::Integer,
            ArgumentClass::Float,
            ArgumentClass::Float,
            ArgumentClass::Float,
        ],
        "vkCmdSetDepthBias2EXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDepthBiasEnable" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDepthBiasEnableEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDepthBounds" => &[
            ArgumentClass::Integer,
            ArgumentClass::Float,
            ArgumentClass::Float,
        ],
        "vkCmdSetDepthBoundsTestEnable" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDepthBoundsTestEnableEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDepthClampEnableEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDepthClampRangeEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetDepthClipEnableEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDepthClipNegativeOneToOneEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDepthCompareOp" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDepthCompareOpEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDepthTestEnable" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDepthTestEnableEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDepthWriteEnable" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDepthWriteEnableEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDescriptorBufferOffsets2EXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDescriptorBufferOffsetsEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetDeviceMask" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDeviceMaskKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDiscardRectangleEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetDiscardRectangleEnableEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDiscardRectangleModeEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetDispatchParametersARM" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetEvent" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetEvent2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetEvent2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetExclusiveScissorEnableNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetExclusiveScissorNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetExtraPrimitiveOverestimationSizeEXT" => {
            &[ArgumentClass::Integer, ArgumentClass::Float]
        }
        "vkCmdSetFragmentShadingRateEnumNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetFragmentShadingRateKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetFrontFace" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetFrontFaceEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetLineRasterizationModeEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetLineStipple" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetLineStippleEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetLineStippleEnableEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetLineStippleKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetLineWidth" => &[ArgumentClass::Integer, ArgumentClass::Float],
        "vkCmdSetLogicOpEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetLogicOpEnableEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetPatchControlPointsEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetPerformanceMarkerINTEL" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetPerformanceOverrideINTEL" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetPerformanceStreamMarkerINTEL" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetPolygonModeEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetPrimitiveRestartEnable" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetPrimitiveRestartEnableEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetPrimitiveRestartIndexEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetPrimitiveTopology" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetPrimitiveTopologyEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetProvokingVertexModeEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetRasterizationSamplesEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetRasterizationStreamEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetRasterizerDiscardEnable" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetRasterizerDiscardEnableEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetRayTracingPipelineStackSizeKHR" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkCmdSetRenderingAttachmentLocations" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetRenderingAttachmentLocationsKHR" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkCmdSetRenderingInputAttachmentIndices" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkCmdSetRenderingInputAttachmentIndicesKHR" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkCmdSetRepresentativeFragmentTestEnableNV" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkCmdSetSampleLocationsEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetSampleLocationsEnableEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetSampleMaskEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetScissor" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetScissorWithCount" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetScissorWithCountEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetShadingRateImageEnableNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetStencilCompareMask" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetStencilOp" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetStencilOpEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetStencilReference" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetStencilTestEnable" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetStencilTestEnableEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetStencilWriteMask" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetTessellationDomainOriginEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetVertexInputEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetViewport" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetViewportShadingRatePaletteNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetViewportSwizzleNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetViewportWScalingEnableNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdSetViewportWScalingNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetViewportWithCount" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSetViewportWithCountEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdSubpassShadingHUAWEI" => &[ArgumentClass::Integer],
        "vkCmdTraceRaysIndirect2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdTraceRaysIndirectKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdTraceRaysKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdTraceRaysNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdUpdateBuffer" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdUpdateMemoryKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdUpdatePipelineIndirectBufferNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdWaitEvents" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdWaitEvents2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdWaitEvents2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdWriteAccelerationStructuresPropertiesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdWriteAccelerationStructuresPropertiesNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdWriteBufferMarker2AMD" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdWriteBufferMarkerAMD" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdWriteMarkerToMemoryAMD" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCmdWriteMicromapsPropertiesEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdWriteTimestamp" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdWriteTimestamp2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCmdWriteTimestamp2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCompileDeferredNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkConvertCooperativeVectorMatrixNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCopyAccelerationStructureKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCopyAccelerationStructureToMemoryKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCopyImageToImage" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCopyImageToImageEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCopyImageToMemory" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCopyImageToMemoryEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCopyMemoryToAccelerationStructureKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCopyMemoryToImage" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCopyMemoryToImageEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkCopyMemoryToMicromapEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCopyMicromapEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCopyMicromapToMemoryEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateAccelerationStructure2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateAccelerationStructureKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateAccelerationStructureNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateAndroidSurfaceKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateBuffer" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateBufferCollectionFUCHSIA" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateBufferView" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateCommandPool" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateComputePipelines" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateCuFunctionNVX" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateCuModuleNVX" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateCudaFunctionNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateCudaModuleNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateDataGraphPipelineSessionARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateDataGraphPipelinesARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateDebugReportCallbackEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateDebugUtilsMessengerEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateDeferredOperationKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateDescriptorPool" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateDescriptorSetLayout" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateDescriptorUpdateTemplate" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateDescriptorUpdateTemplateKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateDevice" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateDirectFBSurfaceEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateDisplayModeKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateDisplayPlaneSurfaceKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateEvent" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateExecutionGraphPipelinesAMDX" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateExternalComputeQueueNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateFence" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateFramebuffer" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateGpaSessionAMD" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateGraphicsPipelines" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateHeadlessSurfaceEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateIOSSurfaceMVK" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateImage" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateImagePipeSurfaceFUCHSIA" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateImageView" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateIndirectCommandsLayoutEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateIndirectCommandsLayoutNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateIndirectExecutionSetEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateInstance" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateMacOSSurfaceMVK" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateMetalSurfaceEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateMicromapEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateOpticalFlowSessionNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreatePipelineBinariesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreatePipelineCache" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreatePipelineLayout" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreatePrivateDataSlot" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreatePrivateDataSlotEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateQueryPool" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateRayTracingPipelinesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateRayTracingPipelinesNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateRenderPass" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateRenderPass2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateRenderPass2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateSampler" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateSamplerYcbcrConversion" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateSamplerYcbcrConversionKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateScreenSurfaceQNX" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateSemaphore" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateSemaphoreSciSyncPoolNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateShaderInstrumentationARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateShaderModule" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateShadersEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateSharedSwapchainsKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateStreamDescriptorSurfaceGGP" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateSurfaceOHOS" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateSwapchainKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateTensorARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateTensorViewARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateUbmSurfaceSEC" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateValidationCacheEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateViSurfaceNN" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateVideoSessionKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateVideoSessionParametersKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateWaylandSurfaceKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateWin32SurfaceKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateXcbSurfaceKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkCreateXlibSurfaceKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDebugMarkerSetObjectNameEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkDebugMarkerSetObjectTagEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkDebugReportMessageEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDeferredOperationJoinKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkDestroyAccelerationStructureKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyAccelerationStructureNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyBuffer" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyBufferCollectionFUCHSIA" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyBufferView" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyCommandPool" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyCuFunctionNVX" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyCuModuleNVX" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyCudaFunctionNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyCudaModuleNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyDataGraphPipelineSessionARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyDebugReportCallbackEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyDebugUtilsMessengerEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyDeferredOperationKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyDescriptorPool" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyDescriptorSetLayout" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyDescriptorUpdateTemplate" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyDescriptorUpdateTemplateKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyDevice" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkDestroyEvent" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyExternalComputeQueueNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyFence" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyFramebuffer" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyGpaSessionAMD" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyImage" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyImageView" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyIndirectCommandsLayoutEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyIndirectCommandsLayoutNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyIndirectExecutionSetEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyInstance" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkDestroyMicromapEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyOpticalFlowSessionNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyPipeline" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyPipelineBinaryKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyPipelineCache" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyPipelineLayout" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyPrivateDataSlot" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyPrivateDataSlotEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyQueryPool" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyRenderPass" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroySampler" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroySamplerYcbcrConversion" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroySamplerYcbcrConversionKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroySemaphore" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroySemaphoreSciSyncPoolNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyShaderEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyShaderInstrumentationARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyShaderModule" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroySurfaceKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroySwapchainKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyTensorARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyTensorViewARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyValidationCacheEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyVideoSessionKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDestroyVideoSessionParametersKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkDeviceWaitIdle" => &[ArgumentClass::Integer],
        "vkDisplayPowerControlEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkEndCommandBuffer" => &[ArgumentClass::Integer],
        "vkEnumerateDeviceExtensionProperties" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkEnumerateDeviceLayerProperties" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkEnumerateInstanceExtensionProperties" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkEnumerateInstanceLayerProperties" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkEnumerateInstanceVersion" => &[ArgumentClass::Integer],
        "vkEnumeratePhysicalDeviceGroups" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkEnumeratePhysicalDeviceGroupsKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkEnumeratePhysicalDeviceQueueFamilyPerformanceCountersByRegionARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkEnumeratePhysicalDeviceQueueFamilyPerformanceQueryCountersKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkEnumeratePhysicalDeviceShaderInstrumentationMetricsARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkEnumeratePhysicalDevices" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkExportMetalObjectsEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkFlushMappedMemoryRanges" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkFreeCommandBuffers" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkFreeDescriptorSets" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkFreeMemory" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetAccelerationStructureBuildSizesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetAccelerationStructureDeviceAddressKHR" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkGetAccelerationStructureHandleNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetAccelerationStructureMemoryRequirementsNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetAccelerationStructureOpaqueCaptureDescriptorDataEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetAndroidHardwareBufferPropertiesANDROID" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetBufferCollectionPropertiesFUCHSIA" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetBufferDeviceAddress" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetBufferDeviceAddressEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetBufferDeviceAddressKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetBufferMemoryRequirements" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetBufferMemoryRequirements2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetBufferMemoryRequirements2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetBufferOpaqueCaptureAddress" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetBufferOpaqueCaptureAddressKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetBufferOpaqueCaptureDescriptorDataEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetCalibratedTimestampsEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetCalibratedTimestampsKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetClusterAccelerationStructureBuildSizesNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetCommandPoolMemoryConsumption" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetCudaModuleCacheNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDataGraphPipelineAvailablePropertiesARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDataGraphPipelinePropertiesARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDataGraphPipelineSessionBindPointRequirementsARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDataGraphPipelineSessionMemoryRequirementsARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeferredOperationMaxConcurrencyKHR" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkGetDeferredOperationResultKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetDescriptorEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDescriptorSetHostMappingVALVE" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDescriptorSetLayoutBindingOffsetEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDescriptorSetLayoutHostMappingInfoVALVE" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDescriptorSetLayoutSizeEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDescriptorSetLayoutSupport" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDescriptorSetLayoutSupportKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceAccelerationStructureCompatibilityKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceBufferMemoryRequirements" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceBufferMemoryRequirementsKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceCombinedImageSamplerIndexNVX" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceFaultDebugInfoKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetDeviceFaultInfoEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceFaultReportsKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceGroupPeerMemoryFeatures" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceGroupPeerMemoryFeaturesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceGroupPresentCapabilitiesKHR" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkGetDeviceGroupSurfacePresentModes2EXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceGroupSurfacePresentModesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceImageMemoryRequirements" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceImageMemoryRequirementsKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceImageSparseMemoryRequirements" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceImageSparseMemoryRequirementsKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceImageSubresourceLayout" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceImageSubresourceLayoutKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceMemoryCommitment" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceMemoryOpaqueCaptureAddress" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkGetDeviceMemoryOpaqueCaptureAddressKHR" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkGetDeviceMicromapCompatibilityEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceProcAddr" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetDeviceQueue" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceQueue2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceSubpassShadingMaxWorkgroupSizeHUAWEI" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDeviceTensorMemoryRequirementsARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDisplayModeProperties2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDisplayModePropertiesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDisplayPlaneCapabilities2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDisplayPlaneCapabilitiesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDisplayPlaneSupportedDisplaysKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDrmDisplayEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetDynamicRenderingTilePropertiesQCOM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetEncodedVideoSessionParametersKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetEventStatus" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetExecutionGraphPipelineNodeIndexAMDX" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetExecutionGraphPipelineScratchSizeAMDX" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetExternalComputeQueueDataNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetFaultData" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetFenceFdKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetFenceSciSyncFenceNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetFenceSciSyncObjNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetFenceStatus" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetFenceWin32HandleKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetFramebufferTilePropertiesQCOM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetGeneratedCommandsMemoryRequirementsEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetGeneratedCommandsMemoryRequirementsNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetGpaDeviceClockInfoAMD" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetGpaSessionResultsAMD" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetGpaSessionStatusAMD" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetImageDrmFormatModifierPropertiesEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetImageMemoryRequirements" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetImageMemoryRequirements2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetImageMemoryRequirements2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetImageOpaqueCaptureDataEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetImageOpaqueCaptureDescriptorDataEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetImageSparseMemoryRequirements" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetImageSparseMemoryRequirements2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetImageSparseMemoryRequirements2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetImageSubresourceLayout" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetImageSubresourceLayout2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetImageSubresourceLayout2EXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetImageSubresourceLayout2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetImageViewAddressNVX" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetImageViewHandle64NVX" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetImageViewHandleNVX" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetImageViewOpaqueCaptureDescriptorDataEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetInstanceProcAddr" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetLatencyTimingsLegacyNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetLatencyTimingsNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetMemoryAndroidHardwareBufferANDROID" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetMemoryFdKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetMemoryFdPropertiesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetMemoryHostPointerPropertiesEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetMemoryMetalHandleEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetMemoryMetalHandlePropertiesEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetMemoryNativeBufferOHOS" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetMemoryRemoteAddressNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetMemorySciBufNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetMemoryWin32HandleKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetMemoryWin32HandleNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetMemoryWin32HandlePropertiesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetMemoryZirconHandleFUCHSIA" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetMemoryZirconHandlePropertiesFUCHSIA" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetMicromapBuildSizesEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetNativeBufferPropertiesOHOS" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPartitionedAccelerationStructuresBuildSizesNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPastPresentationTimingEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPastPresentationTimingGOOGLE" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPerformanceParameterINTEL" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceCalibrateableTimeDomainsEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceCalibrateableTimeDomainsKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceCooperativeMatrixFlexibleDimensionsPropertiesNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceCooperativeMatrixProperties2EXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceCooperativeMatrixPropertiesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceCooperativeMatrixPropertiesNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceCooperativeVectorPropertiesNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceDescriptorSizeEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetPhysicalDeviceDirectFBPresentationSupportEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceDisplayPlaneProperties2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceDisplayPlanePropertiesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceDisplayProperties2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceDisplayPropertiesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceExternalBufferProperties" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceExternalBufferPropertiesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceExternalFenceProperties" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceExternalFencePropertiesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceExternalImageFormatPropertiesNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceExternalMemorySciBufPropertiesNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceExternalSemaphoreProperties" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceExternalSemaphorePropertiesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceExternalTensorPropertiesARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceFeatures" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetPhysicalDeviceFeatures2" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetPhysicalDeviceFeatures2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetPhysicalDeviceFormatProperties" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceFormatProperties2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceFormatProperties2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceFragmentShadingRatesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceImageFormatProperties" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceImageFormatProperties2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceImageFormatProperties2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceMemoryProperties" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetPhysicalDeviceMemoryProperties2" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetPhysicalDeviceMemoryProperties2KHR" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkGetPhysicalDeviceMultisamplePropertiesEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceOpticalFlowImageFormatsNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDevicePresentRectanglesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceProperties" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetPhysicalDeviceProperties2" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetPhysicalDeviceProperties2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetPhysicalDeviceQueueFamilyDataGraphEngineOperationPropertiesARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceQueueFamilyDataGraphOpticalFlowImageFormatsARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceQueueFamilyDataGraphProcessingEnginePropertiesARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceQueueFamilyDataGraphPropertiesARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceQueueFamilyPerformanceQueryPassesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceQueueFamilyProperties" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceQueueFamilyProperties2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceQueueFamilyProperties2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceRefreshableObjectTypesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceSciBufAttributesNV" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkGetPhysicalDeviceSciSyncAttributesNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceScreenPresentationSupportQNX" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceSparseImageFormatProperties" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceSparseImageFormatProperties2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceSparseImageFormatProperties2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceSupportedFramebufferMixedSamplesCombinationsNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceSurfaceCapabilities2EXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceSurfaceCapabilities2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceSurfaceCapabilitiesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceSurfaceFormats2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceSurfaceFormatsKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceSurfacePresentModes2EXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceSurfacePresentModesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceSurfaceSupportKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceToolProperties" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceToolPropertiesEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceUbmPresentationSupportSEC" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceVideoCapabilitiesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceVideoEncodeQualityLevelPropertiesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceVideoFormatPropertiesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceWaylandPresentationSupportKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceWin32PresentationSupportKHR" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkGetPhysicalDeviceXcbPresentationSupportKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPhysicalDeviceXlibPresentationSupportKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPipelineBinaryDataKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPipelineCacheData" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPipelineExecutableInternalRepresentationsKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPipelineExecutablePropertiesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPipelineExecutableStatisticsKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPipelineIndirectDeviceAddressNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetPipelineIndirectMemoryRequirementsNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPipelineKeyKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPipelinePropertiesEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPrivateData" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetPrivateDataEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetQueryPoolResults" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetQueueCheckpointData2NV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetQueueCheckpointDataNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetRandROutputDisplayEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetRayTracingCaptureReplayShaderGroupHandlesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetRayTracingShaderGroupHandlesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetRayTracingShaderGroupHandlesNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetRayTracingShaderGroupStackSizeKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetRefreshCycleDurationGOOGLE" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetRenderAreaGranularity" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetRenderingAreaGranularity" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetRenderingAreaGranularityKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetSamplerOpaqueCaptureDescriptorDataEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetScreenBufferPropertiesQNX" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetSemaphoreCounterValue" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetSemaphoreCounterValueKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetSemaphoreFdKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetSemaphoreSciSyncObjNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetSemaphoreWin32HandleKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetSemaphoreZirconHandleFUCHSIA" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetShaderBinaryDataEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetShaderInfoAMD" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetShaderInstrumentationValuesARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetShaderModuleCreateInfoIdentifierEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetShaderModuleIdentifierEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetSleepStatusLegacyNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetSwapchainCounterEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetSwapchainGrallocUsage2ANDROID" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetSwapchainGrallocUsageANDROID" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetSwapchainGrallocUsageOHOS" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetSwapchainImagesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetSwapchainStatusKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkGetSwapchainTimeDomainPropertiesEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetSwapchainTimingPropertiesEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetTensorMemoryRequirementsARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetTensorOpaqueCaptureDataARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetTensorOpaqueCaptureDescriptorDataARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetTensorViewOpaqueCaptureDescriptorDataARM" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetValidationCacheDataEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetVideoSessionMemoryRequirementsKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkGetWinrtDisplayNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkImportFenceFdKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkImportFenceSciSyncFenceNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkImportFenceSciSyncObjNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkImportFenceWin32HandleKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkImportSemaphoreFdKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkImportSemaphoreSciSyncObjNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkImportSemaphoreWin32HandleKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkImportSemaphoreZirconHandleFUCHSIA" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkInitializePerformanceApiINTEL" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkInvalidateMappedMemoryRanges" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkLatencySleepLegacyNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkLatencySleepNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkMapMemory" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkMapMemory2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkMapMemory2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkMergePipelineCaches" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkMergeValidationCachesEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkQueueBeginDebugUtilsLabelEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkQueueBindSparse" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkQueueEndDebugUtilsLabelEXT" => &[ArgumentClass::Integer],
        "vkQueueInsertDebugUtilsLabelEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkQueueNotifyOutOfBandLegacyNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkQueueNotifyOutOfBandNV" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkQueuePresentKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkQueueSetPerfHintQCOM" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkQueueSetPerformanceConfigurationINTEL" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkQueueSignalReleaseImageANDROID" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkQueueSignalReleaseImageOHOS" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkQueueSubmit" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkQueueSubmit2" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkQueueSubmit2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkQueueWaitIdle" => &[ArgumentClass::Integer],
        "vkRegisterCustomBorderColorEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkRegisterDeviceEventEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkRegisterDisplayEventEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkReleaseCapturedPipelineDataKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkReleaseDisplayEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkReleaseFullScreenExclusiveModeEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkReleasePerformanceConfigurationINTEL" => {
            &[ArgumentClass::Integer, ArgumentClass::Integer]
        }
        "vkReleaseProfilingLockKHR" => &[ArgumentClass::Integer],
        "vkReleaseSwapchainImagesEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkReleaseSwapchainImagesKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkResetCommandBuffer" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkResetCommandPool" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkResetDescriptorPool" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkResetEvent" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkResetFences" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkResetGpaSessionAMD" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkResetQueryPool" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkResetQueryPoolEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkSetBufferCollectionBufferConstraintsFUCHSIA" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkSetBufferCollectionImageConstraintsFUCHSIA" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkSetDebugUtilsObjectNameEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkSetDebugUtilsObjectTagEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkSetDeviceMemoryPriorityEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Float,
        ],
        "vkSetEvent" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkSetGpaDeviceClockModeAMD" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkSetHdrMetadataEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkSetLatencyMarkerLegacyNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkSetLatencyMarkerNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkSetLatencySleepModeLegacyNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkSetLatencySleepModeNV" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkSetLocalDimmingAMD" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkSetPrivateData" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkSetPrivateDataEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkSetSwapchainPresentTimingQueueSizeEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkShutdownLatencyDeviceLegacyNV" => &[ArgumentClass::Integer],
        "vkSignalSemaphore" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkSignalSemaphoreKHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkSubmitDebugUtilsMessageEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkTransitionImageLayout" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkTransitionImageLayoutEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkTrimCommandPool" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkTrimCommandPoolKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkUninitializePerformanceApiINTEL" => &[ArgumentClass::Integer],
        "vkUnmapMemory" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkUnmapMemory2" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkUnmapMemory2KHR" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkUnregisterCustomBorderColorEXT" => &[ArgumentClass::Integer, ArgumentClass::Integer],
        "vkUpdateDescriptorSetWithTemplate" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkUpdateDescriptorSetWithTemplateKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkUpdateDescriptorSets" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkUpdateIndirectExecutionSetPipelineEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkUpdateIndirectExecutionSetShaderEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkUpdateVideoSessionParametersKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkWaitForFences" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkWaitForPresent2KHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkWaitForPresentKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkWaitSemaphores" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkWaitSemaphoresKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkWriteAccelerationStructuresPropertiesKHR" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkWriteMicromapsPropertiesEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkWriteResourceDescriptorsEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        "vkWriteSamplerDescriptorsEXT" => &[
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ],
        _ => return None,
    };
    for_target(address, arguments)
}

/// Size of a registered x86-64 structure that may occur in a pNext chain.
pub fn pnext_node_size(s_type: u32) -> Option<usize> {
    match s_type {
        16 => Some(40),          // VkShaderModuleCreateInfo
        30 => Some(48),          // VkPipelineLayoutCreateInfo
        49 => Some(64),          // VkPhysicalDeviceVulkan11Features
        50 => Some(96),          // VkPhysicalDeviceVulkan11Properties
        51 => Some(208),         // VkPhysicalDeviceVulkan12Features
        52 => Some(248),         // VkPhysicalDeviceVulkan12Properties
        53 => Some(80),          // VkPhysicalDeviceVulkan13Features
        54 => Some(216),         // VkPhysicalDeviceVulkan13Properties
        55 => Some(104),         // VkPhysicalDeviceVulkan14Features
        56 => Some(136),         // VkPhysicalDeviceVulkan14Properties
        1000003000 => Some(56),  // VkDisplayPresentInfoKHR
        1000010000 => Some(56),  // VkNativeBufferANDROID
        1000010001 => Some(24),  // VkSwapchainImageCreateInfoANDROID
        1000010002 => Some(24),  // VkPhysicalDevicePresentationPropertiesANDROID
        1000011000 => Some(40),  // VkDebugReportCallbackCreateInfoEXT
        1000018000 => Some(24),  // VkPipelineRasterizationStateRasterizationOrderAMD
        1000023000 => Some(32),  // VkVideoProfileInfoKHR
        1000023012 => Some(24),  // VkQueueFamilyVideoPropertiesKHR
        1000023013 => Some(32),  // VkVideoProfileListInfoKHR
        1000023016 => Some(24),  // VkQueueFamilyQueryResultStatusPropertiesKHR
        1000024001 => Some(24),  // VkVideoDecodeCapabilitiesKHR
        1000024002 => Some(24),  // VkVideoDecodeUsageInfoKHR
        1000026000 => Some(24),  // VkDedicatedAllocationImageCreateInfoNV
        1000026001 => Some(24),  // VkDedicatedAllocationBufferCreateInfoNV
        1000026002 => Some(32),  // VkDedicatedAllocationMemoryAllocateInfoNV
        1000028000 => Some(24),  // VkPhysicalDeviceTransformFeedbackFeaturesEXT
        1000028001 => Some(64),  // VkPhysicalDeviceTransformFeedbackPropertiesEXT
        1000028002 => Some(24),  // VkPipelineRasterizationStateStreamCreateInfoEXT
        1000029004 => Some(24),  // VkCuModuleTexturingModeCreateInfoNVX
        1000038000 => Some(72),  // VkVideoEncodeH264CapabilitiesKHR
        1000038001 => Some(32),  // VkVideoEncodeH264SessionParametersCreateInfoKHR
        1000038002 => Some(48),  // VkVideoEncodeH264SessionParametersAddInfoKHR
        1000038003 => Some(48),  // VkVideoEncodeH264PictureInfoKHR
        1000038004 => Some(24),  // VkVideoEncodeH264DpbSlotInfoKHR
        1000038006 => Some(32),  // VkVideoEncodeH264GopRemainingFrameInfoKHR
        1000038007 => Some(24),  // VkVideoEncodeH264ProfileInfoKHR
        1000038008 => Some(40),  // VkVideoEncodeH264RateControlInfoKHR
        1000038009 => Some(64),  // VkVideoEncodeH264RateControlLayerInfoKHR
        1000038010 => Some(24),  // VkVideoEncodeH264SessionCreateInfoKHR
        1000038011 => Some(64),  // VkVideoEncodeH264QualityLevelPropertiesKHR
        1000038012 => Some(32),  // VkVideoEncodeH264SessionParametersGetInfoKHR
        1000038013 => Some(24),  // VkVideoEncodeH264SessionParametersFeedbackInfoKHR
        1000039000 => Some(88),  // VkVideoEncodeH265CapabilitiesKHR
        1000039001 => Some(40),  // VkVideoEncodeH265SessionParametersCreateInfoKHR
        1000039002 => Some(64),  // VkVideoEncodeH265SessionParametersAddInfoKHR
        1000039003 => Some(40),  // VkVideoEncodeH265PictureInfoKHR
        1000039004 => Some(24),  // VkVideoEncodeH265DpbSlotInfoKHR
        1000039006 => Some(32),  // VkVideoEncodeH265GopRemainingFrameInfoKHR
        1000039007 => Some(24),  // VkVideoEncodeH265ProfileInfoKHR
        1000039009 => Some(40),  // VkVideoEncodeH265RateControlInfoKHR
        1000039010 => Some(64),  // VkVideoEncodeH265RateControlLayerInfoKHR
        1000039011 => Some(24),  // VkVideoEncodeH265SessionCreateInfoKHR
        1000039012 => Some(56),  // VkVideoEncodeH265QualityLevelPropertiesKHR
        1000039013 => Some(40),  // VkVideoEncodeH265SessionParametersGetInfoKHR
        1000039014 => Some(32),  // VkVideoEncodeH265SessionParametersFeedbackInfoKHR
        1000040000 => Some(32),  // VkVideoDecodeH264CapabilitiesKHR
        1000040001 => Some(40),  // VkVideoDecodeH264PictureInfoKHR
        1000040003 => Some(24),  // VkVideoDecodeH264ProfileInfoKHR
        1000040004 => Some(32),  // VkVideoDecodeH264SessionParametersCreateInfoKHR
        1000040005 => Some(48),  // VkVideoDecodeH264SessionParametersAddInfoKHR
        1000040006 => Some(24),  // VkVideoDecodeH264DpbSlotInfoKHR
        1000041000 => Some(24),  // VkTextureLODGatherFormatPropertiesAMD
        1000044002 => Some(40),  // VkPipelineRenderingCreateInfo
        1000044003 => Some(24),  // VkPhysicalDeviceDynamicRenderingFeatures
        1000044004 => Some(56),  // VkCommandBufferInheritanceRenderingInfo
        1000044006 => Some(40),  // VkRenderingFragmentShadingRateAttachmentInfoKHR
        1000044007 => Some(32),  // VkRenderingFragmentDensityMapAttachmentInfoEXT
        1000044008 => Some(40),  // VkAttachmentSampleCountInfoAMD
        1000044009 => Some(24),  // VkMultiviewPerViewAttributesInfoNVX
        1000050000 => Some(24),  // VkPhysicalDeviceCornerSampledImageFeaturesNV
        1000053000 => Some(64),  // VkRenderPassMultiviewCreateInfo
        1000053001 => Some(32),  // VkPhysicalDeviceMultiviewFeatures
        1000053002 => Some(24),  // VkPhysicalDeviceMultiviewProperties
        1000056000 => Some(24),  // VkExternalMemoryImageCreateInfoNV
        1000056001 => Some(24),  // VkExportMemoryAllocateInfoNV
        1000057000 => Some(32),  // VkImportMemoryWin32HandleInfoNV
        1000057001 => Some(32),  // VkExportMemoryWin32HandleInfoNV
        1000058000 => Some(72),  // VkWin32KeyedMutexAcquireReleaseInfoNV
        1000059000 => Some(240), // VkPhysicalDeviceFeatures2
        1000060000 => Some(24),  // VkMemoryAllocateFlagsInfo
        1000060003 => Some(32),  // VkDeviceGroupRenderPassBeginInfo
        1000060004 => Some(24),  // VkDeviceGroupCommandBufferBeginInfo
        1000060005 => Some(64),  // VkDeviceGroupSubmitInfo
        1000060006 => Some(24),  // VkDeviceGroupBindSparseInfo
        1000060008 => Some(24),  // VkImageSwapchainCreateInfoKHR
        1000060009 => Some(32),  // VkBindImageMemorySwapchainInfoKHR
        1000060011 => Some(40),  // VkDeviceGroupPresentInfoKHR
        1000060012 => Some(24),  // VkDeviceGroupSwapchainCreateInfoKHR
        1000060013 => Some(32),  // VkBindBufferMemoryDeviceGroupInfo
        1000060014 => Some(48),  // VkBindImageMemoryDeviceGroupInfo
        1000061000 => Some(32),  // VkValidationFlagsEXT
        1000063000 => Some(24),  // VkPhysicalDeviceShaderDrawParametersFeatures
        1000066000 => Some(24),  // VkPhysicalDeviceTextureCompressionASTCHDRFeatures
        1000067000 => Some(24),  // VkImageViewASTCDecodeModeEXT
        1000067001 => Some(24),  // VkPhysicalDeviceASTCDecodeFeaturesEXT
        1000068000 => Some(32),  // VkPipelineRobustnessCreateInfo
        1000068001 => Some(24),  // VkPhysicalDevicePipelineRobustnessFeatures
        1000068002 => Some(32),  // VkPhysicalDevicePipelineRobustnessProperties
        1000070001 => Some(32),  // VkDeviceGroupDeviceCreateInfo
        1000071000 => Some(24),  // VkPhysicalDeviceExternalImageFormatInfo
        1000071001 => Some(32),  // VkExternalImageFormatProperties
        1000071004 => Some(48),  // VkPhysicalDeviceIDProperties
        1000072000 => Some(24),  // VkExternalMemoryBufferCreateInfo
        1000072001 => Some(24),  // VkExternalMemoryImageCreateInfo
        1000072002 => Some(24),  // VkExportMemoryAllocateInfo
        1000073000 => Some(40),  // VkImportMemoryWin32HandleInfoKHR
        1000073001 => Some(40),  // VkExportMemoryWin32HandleInfoKHR
        1000074000 => Some(24),  // VkImportMemoryFdInfoKHR
        1000075000 => Some(72),  // VkWin32KeyedMutexAcquireReleaseInfoKHR
        1000077000 => Some(24),  // VkExportSemaphoreCreateInfo
        1000078001 => Some(40),  // VkExportSemaphoreWin32HandleInfoKHR
        1000078002 => Some(48),  // VkD3D12FenceSubmitInfoKHR
        1000080000 => Some(24),  // VkPhysicalDevicePushDescriptorProperties
        1000081000 => Some(24),  // VkCommandBufferInheritanceConditionalRenderingInfoEXT
        1000081001 => Some(24),  // VkPhysicalDeviceConditionalRenderingFeaturesEXT
        1000082000 => Some(24),  // VkPhysicalDeviceShaderFloat16Int8Features
        1000083000 => Some(32),  // VkPhysicalDevice16BitStorageFeatures
        1000084000 => Some(32),  // VkPresentRegionsKHR
        1000087000 => Some(32),  // VkPipelineViewportWScalingStateCreateInfoNV
        1000091003 => Some(24),  // VkSwapchainCounterCreateInfoEXT
        1000092000 => Some(32),  // VkPresentTimesInfoGOOGLE
        1000094000 => Some(32),  // VkPhysicalDeviceSubgroupProperties
        1000097000 => Some(24),  // VkPhysicalDeviceMultiviewPerViewAttributesPropertiesNVX
        1000098000 => Some(32),  // VkPipelineViewportSwizzleStateCreateInfoNV
        1000099000 => Some(24),  // VkPhysicalDeviceDiscardRectanglePropertiesEXT
        1000099001 => Some(40),  // VkPipelineDiscardRectangleStateCreateInfoEXT
        1000101000 => Some(56),  // VkPhysicalDeviceConservativeRasterizationPropertiesEXT
        1000101001 => Some(32),  // VkPipelineRasterizationConservativeStateCreateInfoEXT
        1000102000 => Some(24),  // VkPhysicalDeviceDepthClipEnableFeaturesEXT
        1000102001 => Some(24),  // VkPipelineRasterizationDepthClipStateCreateInfoEXT
        1000108000 => Some(24),  // VkPhysicalDeviceImagelessFramebufferFeatures
        1000108001 => Some(32),  // VkFramebufferAttachmentsCreateInfo
        1000108003 => Some(32),  // VkRenderPassAttachmentBeginInfo
        1000110000 => Some(24),  // VkPhysicalDeviceRelaxedLineRasterizationFeaturesIMG
        1000111000 => Some(24),  // VkSharedPresentSurfaceCapabilitiesKHR
        1000113000 => Some(24),  // VkExportFenceCreateInfo
        1000114001 => Some(40),  // VkExportFenceWin32HandleInfoKHR
        1000116000 => Some(24),  // VkPhysicalDevicePerformanceQueryFeaturesKHR
        1000116001 => Some(24),  // VkPhysicalDevicePerformanceQueryPropertiesKHR
        1000116002 => Some(32),  // VkQueryPoolPerformanceCreateInfoKHR
        1000116003 => Some(24),  // VkPerformanceQuerySubmitInfoKHR
        1000116007 => Some(24),  // VkPerformanceQueryReservationInfoKHR
        1000117000 => Some(24),  // VkPhysicalDevicePointClippingProperties
        1000117001 => Some(32),  // VkRenderPassInputAttachmentAspectCreateInfo
        1000117002 => Some(24),  // VkImageViewUsageCreateInfo
        1000117003 => Some(24),  // VkPipelineTessellationDomainOriginStateCreateInfo
        1000120000 => Some(24),  // VkPhysicalDeviceVariablePointersFeatures
        1000127000 => Some(24),  // VkMemoryDedicatedRequirements
        1000127001 => Some(32),  // VkMemoryDedicatedAllocateInfo
        1000128000 => Some(40),  // VkDebugUtilsObjectNameInfoEXT
        1000128004 => Some(48),  // VkDebugUtilsMessengerCreateInfoEXT
        1000129000 => Some(24),  // VkAndroidHardwareBufferUsageANDROID
        1000129002 => Some(72),  // VkAndroidHardwareBufferFormatPropertiesANDROID
        1000129003 => Some(24),  // VkImportAndroidHardwareBufferInfoANDROID
        1000129005 => Some(24),  // VkExternalFormatANDROID
        1000129006 => Some(72),  // VkAndroidHardwareBufferFormatProperties2ANDROID
        1000130000 => Some(24),  // VkPhysicalDeviceSamplerFilterMinmaxProperties
        1000130001 => Some(24),  // VkSamplerReductionModeCreateInfo
        1000133000 => Some(32),  // VkPhysicalDeviceGpaFeaturesAMD
        1000133001 => Some(48),  // VkPhysicalDeviceGpaPropertiesAMD
        1000133005 => Some(24),  // VkPhysicalDeviceGpaProperties2AMD
        1000134000 => Some(24),  // VkPhysicalDeviceShaderEnqueueFeaturesAMDX
        1000134001 => Some(56),  // VkPhysicalDeviceShaderEnqueuePropertiesAMDX
        1000134004 => Some(32),  // VkPipelineShaderStageNodeCreateInfoAMDX
        1000135006 => Some(32),  // VkShaderDescriptorSetAndBindingMappingInfoEXT
        1000135007 => Some(24),  // VkOpaqueCaptureDataCreateInfoEXT
        1000135008 => Some(152), // VkPhysicalDeviceDescriptorHeapPropertiesEXT
        1000135009 => Some(24),  // VkPhysicalDeviceDescriptorHeapFeaturesEXT
        1000135010 => Some(32),  // VkCommandBufferInheritanceDescriptorHeapInfoEXT
        1000135011 => Some(24),  // VkSamplerCustomBorderColorIndexCreateInfoEXT
        1000135012 => Some(24),  // VkIndirectCommandsLayoutPushDataTokenNV
        1000135013 => Some(24),  // VkSubsampledImageFormatPropertiesEXT
        1000135014 => Some(40),  // VkPhysicalDeviceDescriptorHeapTensorPropertiesARM
        1000138000 => Some(24),  // VkPhysicalDeviceInlineUniformBlockFeatures
        1000138001 => Some(40),  // VkPhysicalDeviceInlineUniformBlockProperties
        1000138002 => Some(32),  // VkWriteDescriptorSetInlineUniformBlock
        1000138003 => Some(24),  // VkDescriptorPoolInlineUniformBlockCreateInfo
        1000141000 => Some(32),  // VkPhysicalDeviceShaderBfloat16FeaturesKHR
        1000143000 => Some(40),  // VkSampleLocationsInfoEXT
        1000143001 => Some(48),  // VkRenderPassSampleLocationsBeginInfoEXT
        1000143002 => Some(64),  // VkPipelineSampleLocationsStateCreateInfoEXT
        1000143003 => Some(48),  // VkPhysicalDeviceSampleLocationsPropertiesEXT
        1000145000 => Some(24),  // VkProtectedSubmitInfo
        1000145001 => Some(24),  // VkPhysicalDeviceProtectedMemoryFeatures
        1000145002 => Some(24),  // VkPhysicalDeviceProtectedMemoryProperties
        1000147000 => Some(32),  // VkImageFormatListCreateInfo
        1000148000 => Some(24),  // VkPhysicalDeviceBlendOperationAdvancedFeaturesEXT
        1000148001 => Some(40),  // VkPhysicalDeviceBlendOperationAdvancedPropertiesEXT
        1000148002 => Some(32),  // VkPipelineColorBlendAdvancedStateCreateInfoEXT
        1000149000 => Some(32),  // VkPipelineCoverageToColorStateCreateInfoNV
        1000150007 => Some(32),  // VkWriteDescriptorSetAccelerationStructureKHR
        1000150013 => Some(40),  // VkPhysicalDeviceAccelerationStructureFeaturesKHR
        1000150014 => Some(64),  // VkPhysicalDeviceAccelerationStructurePropertiesKHR
        1000152000 => Some(40),  // VkPipelineCoverageModulationStateCreateInfoNV
        1000154000 => Some(24),  // VkPhysicalDeviceShaderSMBuiltinsFeaturesNV
        1000154001 => Some(24),  // VkPhysicalDeviceShaderSMBuiltinsPropertiesNV
        1000156001 => Some(24),  // VkSamplerYcbcrConversionInfo
        1000156002 => Some(24),  // VkBindImagePlaneMemoryInfo
        1000156003 => Some(24),  // VkImagePlaneMemoryRequirementsInfo
        1000156004 => Some(24),  // VkPhysicalDeviceSamplerYcbcrConversionFeatures
        1000156005 => Some(24),  // VkSamplerYcbcrConversionImageFormatProperties
        1000158000 => Some(32),  // VkDrmFormatModifierPropertiesListEXT
        1000158002 => Some(40),  // VkPhysicalDeviceImageDrmFormatModifierInfoEXT
        1000158003 => Some(32),  // VkImageDrmFormatModifierListCreateInfoEXT
        1000158004 => Some(40),  // VkImageDrmFormatModifierExplicitCreateInfoEXT
        1000158006 => Some(32),  // VkDrmFormatModifierPropertiesList2EXT
        1000160001 => Some(24),  // VkShaderModuleValidationCacheCreateInfoEXT
        1000161000 => Some(32),  // VkDescriptorSetLayoutBindingFlagsCreateInfo
        1000161001 => Some(96),  // VkPhysicalDeviceDescriptorIndexingFeatures
        1000161002 => Some(112), // VkPhysicalDeviceDescriptorIndexingProperties
        1000161003 => Some(32),  // VkDescriptorSetVariableDescriptorCountAllocateInfo
        1000161004 => Some(24),  // VkDescriptorSetVariableDescriptorCountLayoutSupport
        1000163000 => Some(80),  // VkPhysicalDevicePortabilitySubsetFeaturesKHR
        1000163001 => Some(24),  // VkPhysicalDevicePortabilitySubsetPropertiesKHR
        1000164000 => Some(32),  // VkPipelineViewportShadingRateImageStateCreateInfoNV
        1000164001 => Some(24),  // VkPhysicalDeviceShadingRateImageFeaturesNV
        1000164002 => Some(32),  // VkPhysicalDeviceShadingRateImagePropertiesNV
        1000164005 => Some(32),  // VkPipelineViewportCoarseSampleOrderStateCreateInfoNV
        1000165007 => Some(32),  // VkWriteDescriptorSetAccelerationStructureNV
        1000165009 => Some(64),  // VkPhysicalDeviceRayTracingPropertiesNV
        1000166000 => Some(24),  // VkPhysicalDeviceRepresentativeFragmentTestFeaturesNV
        1000166001 => Some(24),  // VkPipelineRepresentativeFragmentTestStateCreateInfoNV
        1000168000 => Some(32),  // VkPhysicalDeviceMaintenance3Properties
        1000170000 => Some(24),  // VkPhysicalDeviceImageViewImageFormatInfoEXT
        1000170001 => Some(24),  // VkFilterCubicImageViewImageFormatPropertiesEXT
        1000172000 => Some(24),  // VkPhysicalDeviceCooperativeMatrixConversionFeaturesQCOM
        1000173000 => Some(24),  // VkPhysicalDeviceElapsedTimerQueryFeaturesQCOM
        1000174000 => Some(24),  // VkDeviceQueueGlobalPriorityCreateInfo
        1000175000 => Some(24),  // VkPhysicalDeviceShaderSubgroupExtendedTypesFeatures
        1000177000 => Some(32),  // VkPhysicalDevice8BitStorageFeatures
        1000178000 => Some(32),  // VkImportMemoryHostPointerInfoEXT
        1000178002 => Some(24),  // VkPhysicalDeviceExternalMemoryHostPropertiesEXT
        1000180000 => Some(24),  // VkPhysicalDeviceShaderAtomicInt64Features
        1000181000 => Some(24),  // VkPhysicalDeviceShaderClockFeaturesKHR
        1000183000 => Some(24),  // VkPipelineCompilerControlCreateInfoAMD
        1000185000 => Some(72),  // VkPhysicalDeviceShaderCorePropertiesAMD
        1000187000 => Some(24),  // VkVideoDecodeH265CapabilitiesKHR
        1000187001 => Some(40),  // VkVideoDecodeH265SessionParametersCreateInfoKHR
        1000187002 => Some(64),  // VkVideoDecodeH265SessionParametersAddInfoKHR
        1000187003 => Some(24),  // VkVideoDecodeH265ProfileInfoKHR
        1000187004 => Some(40),  // VkVideoDecodeH265PictureInfoKHR
        1000187005 => Some(24),  // VkVideoDecodeH265DpbSlotInfoKHR
        1000189000 => Some(24),  // VkDeviceMemoryOverallocationCreateInfoAMD
        1000190000 => Some(24),  // VkPhysicalDeviceVertexAttributeDivisorPropertiesEXT
        1000190001 => Some(32),  // VkPipelineVertexInputDivisorStateCreateInfo
        1000190002 => Some(24),  // VkPhysicalDeviceVertexAttributeDivisorFeatures
        1000191000 => Some(24),  // VkPresentFrameTokenGGP
        1000192000 => Some(40),  // VkPipelineCreationFeedbackCreateInfo
        1000196000 => Some(48),  // VkPhysicalDeviceDriverProperties
        1000197000 => Some(88),  // VkPhysicalDeviceFloatControlsProperties
        1000199000 => Some(32),  // VkPhysicalDeviceDepthStencilResolveProperties
        1000199001 => Some(32),  // VkSubpassDescriptionDepthStencilResolve
        1000201000 => Some(24),  // VkPhysicalDeviceComputeShaderDerivativesFeaturesKHR
        1000202000 => Some(24),  // VkPhysicalDeviceMeshShaderFeaturesNV
        1000202001 => Some(80),  // VkPhysicalDeviceMeshShaderPropertiesNV
        1000203000 => Some(24),  // VkPhysicalDeviceFragmentShaderBarycentricFeaturesKHR
        1000204000 => Some(24),  // VkPhysicalDeviceShaderImageFootprintFeaturesNV
        1000205000 => Some(32),  // VkPipelineViewportExclusiveScissorStateCreateInfoNV
        1000205002 => Some(24),  // VkPhysicalDeviceExclusiveScissorFeaturesNV
        1000206001 => Some(24),  // VkQueueFamilyCheckpointPropertiesNV
        1000207000 => Some(24),  // VkPhysicalDeviceTimelineSemaphoreFeatures
        1000207001 => Some(24),  // VkPhysicalDeviceTimelineSemaphoreProperties
        1000207002 => Some(32),  // VkSemaphoreTypeCreateInfo
        1000207003 => Some(48),  // VkTimelineSemaphoreSubmitInfo
        1000208000 => Some(32),  // VkPhysicalDevicePresentTimingFeaturesEXT
        1000208003 => Some(32),  // VkPresentTimingsInfoEXT
        1000208008 => Some(32),  // VkPresentTimingSurfaceCapabilitiesEXT
        1000208009 => Some(40),  // VkSwapchainCalibratedTimestampInfoEXT
        1000209000 => Some(24),  // VkPhysicalDeviceShaderIntegerFunctions2FeaturesINTEL
        1000210000 => Some(24),  // VkQueryPoolPerformanceQueryCreateInfoINTEL
        1000211000 => Some(32),  // VkPhysicalDeviceVulkanMemoryModelFeatures
        1000212000 => Some(32),  // VkPhysicalDevicePCIBusInfoPropertiesEXT
        1000213000 => Some(24),  // VkDisplayNativeHdrSurfaceCapabilitiesAMD
        1000213001 => Some(24),  // VkSwapchainDisplayNativeHdrCreateInfoAMD
        1000215000 => Some(24),  // VkPhysicalDeviceShaderTerminateInvocationFeatures
        1000218000 => Some(32),  // VkPhysicalDeviceFragmentDensityMapFeaturesEXT
        1000218001 => Some(40),  // VkPhysicalDeviceFragmentDensityMapPropertiesEXT
        1000218002 => Some(24),  // VkRenderPassFragmentDensityMapCreateInfoEXT
        1000221000 => Some(24),  // VkPhysicalDeviceScalarBlockLayoutFeatures
        1000225000 => Some(32),  // VkPhysicalDeviceSubgroupSizeControlProperties
        1000225001 => Some(24),  // VkPipelineShaderStageRequiredSubgroupSizeCreateInfo
        1000225002 => Some(24),  // VkPhysicalDeviceSubgroupSizeControlFeatures
        1000226000 => Some(32),  // VkFragmentShadingRateAttachmentInfoKHR
        1000226001 => Some(32),  // VkPipelineFragmentShadingRateStateCreateInfoKHR
        1000226002 => Some(96),  // VkPhysicalDeviceFragmentShadingRatePropertiesKHR
        1000226003 => Some(32),  // VkPhysicalDeviceFragmentShadingRateFeaturesKHR
        1000227000 => Some(24),  // VkPhysicalDeviceShaderCoreProperties2AMD
        1000229000 => Some(24),  // VkPhysicalDeviceCoherentMemoryFeaturesAMD
        1000231000 => Some(24),  // VkPhysicalDeviceShaderConstantDataFeaturesKHR
        1000232000 => Some(24),  // VkPhysicalDeviceDynamicRenderingLocalReadFeatures
        1000232001 => Some(32),  // VkRenderingAttachmentLocationInfo
        1000232002 => Some(48),  // VkRenderingInputAttachmentIndexInfo
        1000233000 => Some(24),  // VkPhysicalDeviceShaderAbortFeaturesKHR
        1000233001 => Some(32),  // VkDeviceFaultShaderAbortMessageInfoKHR
        1000233002 => Some(24),  // VkPhysicalDeviceShaderAbortPropertiesKHR
        1000234000 => Some(24),  // VkPhysicalDeviceShaderImageAtomicInt64FeaturesEXT
        1000235000 => Some(24),  // VkPhysicalDeviceShaderQuadControlFeaturesKHR
        1000237000 => Some(32),  // VkPhysicalDeviceMemoryBudgetPropertiesEXT
        1000238000 => Some(24),  // VkPhysicalDeviceMemoryPriorityFeaturesEXT
        1000238001 => Some(24),  // VkMemoryPriorityAllocateInfoEXT
        1000239000 => Some(24),  // VkSurfaceProtectedCapabilitiesKHR
        1000240000 => Some(24),  // VkPhysicalDeviceDedicatedAllocationImageAliasingFeaturesNV
        1000241000 => Some(24),  // VkPhysicalDeviceSeparateDepthStencilLayoutsFeatures
        1000241001 => Some(24),  // VkAttachmentReferenceStencilLayout
        1000241002 => Some(24),  // VkAttachmentDescriptionStencilLayout
        1000244000 => Some(32),  // VkPhysicalDeviceBufferDeviceAddressFeaturesEXT
        1000244002 => Some(24),  // VkBufferDeviceAddressCreateInfoEXT
        1000246000 => Some(24),  // VkImageStencilUsageCreateInfo
        1000247000 => Some(48),  // VkValidationFeaturesEXT
        1000248000 => Some(24),  // VkPhysicalDevicePresentWaitFeaturesKHR
        1000249000 => Some(24),  // VkPhysicalDeviceCooperativeMatrixFeaturesNV
        1000249002 => Some(24),  // VkPhysicalDeviceCooperativeMatrixPropertiesNV
        1000250000 => Some(24),  // VkPhysicalDeviceCoverageReductionModeFeaturesNV
        1000250001 => Some(24),  // VkPipelineCoverageReductionStateCreateInfoNV
        1000251000 => Some(32),  // VkPhysicalDeviceFragmentShaderInterlockFeaturesEXT
        1000252000 => Some(24),  // VkPhysicalDeviceYcbcrImageArraysFeaturesEXT
        1000253000 => Some(24),  // VkPhysicalDeviceUniformBufferStandardLayoutFeatures
        1000254000 => Some(24),  // VkPhysicalDeviceProvokingVertexFeaturesEXT
        1000254001 => Some(24),  // VkPipelineRasterizationProvokingVertexStateCreateInfoEXT
        1000254002 => Some(24),  // VkPhysicalDeviceProvokingVertexPropertiesEXT
        1000255000 => Some(24),  // VkSurfaceFullScreenExclusiveInfoEXT
        1000255001 => Some(24),  // VkSurfaceFullScreenExclusiveWin32InfoEXT
        1000255002 => Some(24),  // VkSurfaceCapabilitiesFullScreenExclusiveEXT
        1000257000 => Some(32),  // VkPhysicalDeviceBufferDeviceAddressFeatures
        1000257002 => Some(24),  // VkBufferOpaqueCaptureAddressCreateInfo
        1000257003 => Some(24),  // VkMemoryOpaqueCaptureAddressAllocateInfo
        1000259000 => Some(40),  // VkPhysicalDeviceLineRasterizationFeatures
        1000259001 => Some(32),  // VkPipelineRasterizationLineStateCreateInfo
        1000259002 => Some(24),  // VkPhysicalDeviceLineRasterizationProperties
        1000260000 => Some(64),  // VkPhysicalDeviceShaderAtomicFloatFeaturesEXT
        1000261000 => Some(24),  // VkPhysicalDeviceHostQueryResetFeatures
        1000265000 => Some(24),  // VkPhysicalDeviceIndexTypeUint8Features
        1000267000 => Some(24),  // VkPhysicalDeviceExtendedDynamicStateFeaturesEXT
        1000269000 => Some(24),  // VkPhysicalDevicePipelineExecutablePropertiesFeaturesKHR
        1000270000 => Some(24),  // VkPhysicalDeviceHostImageCopyFeatures
        1000270001 => Some(64),  // VkPhysicalDeviceHostImageCopyProperties
        1000270008 => Some(24),  // VkSubresourceHostMemcpySize
        1000270009 => Some(24),  // VkHostImageCopyDevicePerformanceQuery
        1000272000 => Some(32),  // VkPhysicalDeviceMapMemoryPlacedFeaturesEXT
        1000272001 => Some(24),  // VkPhysicalDeviceMapMemoryPlacedPropertiesEXT
        1000272002 => Some(24),  // VkMemoryMapPlacedInfoEXT
        1000273000 => Some(64),  // VkPhysicalDeviceShaderAtomicFloat2FeaturesEXT
        1000274000 => Some(24),  // VkSurfacePresentModeKHR
        1000274001 => Some(48),  // VkSurfacePresentScalingCapabilitiesKHR
        1000274002 => Some(32),  // VkSurfacePresentModeCompatibilityKHR
        1000275000 => Some(24),  // VkPhysicalDeviceSwapchainMaintenance1FeaturesKHR
        1000275001 => Some(32),  // VkSwapchainPresentFenceInfoKHR
        1000275002 => Some(32),  // VkSwapchainPresentModesCreateInfoKHR
        1000275003 => Some(32),  // VkSwapchainPresentModeInfoKHR
        1000275004 => Some(32),  // VkSwapchainPresentScalingCreateInfoKHR
        1000276000 => Some(24),  // VkPhysicalDeviceShaderDemoteToHelperInvocationFeatures
        1000277000 => Some(56),  // VkPhysicalDeviceDeviceGeneratedCommandsPropertiesNV
        1000277002 => Some(48),  // VkGraphicsPipelineShaderGroupsCreateInfoNV
        1000277007 => Some(24),  // VkPhysicalDeviceDeviceGeneratedCommandsFeaturesNV
        1000278000 => Some(24),  // VkPhysicalDeviceInheritedViewportScissorFeaturesNV
        1000278001 => Some(32),  // VkCommandBufferInheritanceViewportScissorInfoNV
        1000280000 => Some(24),  // VkPhysicalDeviceShaderIntegerDotProductFeatures
        1000280001 => Some(136), // VkPhysicalDeviceShaderIntegerDotProductProperties
        1000281000 => Some(24),  // VkPhysicalDeviceTexelBufferAlignmentFeaturesEXT
        1000281001 => Some(48),  // VkPhysicalDeviceTexelBufferAlignmentProperties
        1000282000 => Some(40),  // VkCommandBufferInheritanceRenderPassTransformInfoQCOM
        1000282001 => Some(24),  // VkRenderPassTransformBeginInfoQCOM
        1000283000 => Some(32),  // VkPhysicalDeviceDepthBiasControlFeaturesEXT
        1000283002 => Some(24),  // VkDepthBiasRepresentationInfoEXT
        1000284000 => Some(24),  // VkPhysicalDeviceDeviceMemoryReportFeaturesEXT
        1000284001 => Some(40),  // VkDeviceDeviceMemoryReportCreateInfoEXT
        1000286000 => Some(32),  // VkPhysicalDeviceRobustness2FeaturesKHR
        1000286001 => Some(32),  // VkPhysicalDeviceRobustness2PropertiesKHR
        1000287000 => Some(32),  // VkSamplerCustomBorderColorCreateInfoEXT
        1000287001 => Some(24),  // VkPhysicalDeviceCustomBorderColorPropertiesEXT
        1000287002 => Some(24),  // VkPhysicalDeviceCustomBorderColorFeaturesEXT
        1000288000 => Some(24),  // VkPhysicalDeviceTextureCompressionASTC3DFeaturesEXT
        1000290000 => Some(32),  // VkPipelineLibraryCreateInfoKHR
        1000292000 => Some(24),  // VkPhysicalDevicePresentBarrierFeaturesNV
        1000292001 => Some(24),  // VkSurfaceCapabilitiesPresentBarrierNV
        1000292002 => Some(24),  // VkSwapchainPresentBarrierCreateInfoNV
        1000294000 => Some(32),  // VkPresentIdKHR
        1000294001 => Some(24),  // VkPhysicalDevicePresentIdFeaturesKHR
        1000295000 => Some(24),  // VkPhysicalDevicePrivateDataFeatures
        1000295001 => Some(24),  // VkDevicePrivateDataCreateInfo
        1000297000 => Some(24),  // VkPhysicalDevicePipelineCreationCacheControlFeatures
        1000298000 => Some(24),  // VkPhysicalDeviceVulkanSC10Features
        1000298001 => Some(96),  // VkPhysicalDeviceVulkanSC10Properties
        1000298002 => Some(200), // VkDeviceObjectReservationCreateInfo
        1000298003 => Some(32),  // VkCommandPoolMemoryReservationCreateInfo
        1000298008 => Some(40),  // VkFaultCallbackInfo
        1000298010 => Some(40),  // VkPipelineOfflineCreateInfo
        1000299001 => Some(48),  // VkVideoEncodeRateControlInfoKHR
        1000299003 => Some(56),  // VkVideoEncodeCapabilitiesKHR
        1000299004 => Some(32),  // VkVideoEncodeUsageInfoKHR
        1000299005 => Some(24),  // VkQueryPoolVideoEncodeFeedbackCreateInfoKHR
        1000299008 => Some(24),  // VkVideoEncodeQualityLevelInfoKHR
        1000300000 => Some(24),  // VkPhysicalDeviceDiagnosticsConfigFeaturesNV
        1000300001 => Some(24),  // VkDeviceDiagnosticsConfigCreateInfoNV
        1000302001 => Some(24),  // VkPhysicalDeviceQueuePerfHintFeaturesQCOM
        1000302002 => Some(24),  // VkPhysicalDeviceQueuePerfHintPropertiesQCOM
        1000303000 => Some(32),  // VkPhysicalDeviceImageProcessing3FeaturesQCOM
        1000304000 => Some(24),  // VkPhysicalDeviceShaderMultipleWaitQueuesFeaturesQCOM
        1000304001 => Some(24),  // VkPhysicalDeviceShaderMultipleWaitQueuesPropertiesQCOM
        1000305000 => Some(24),  // VkPhysicalDeviceShaderSplitBarrierFeaturesEXT
        1000305001 => Some(24),  // VkPhysicalDeviceShaderSplitBarrierPropertiesEXT
        1000307003 => Some(24),  // VkPhysicalDeviceCudaKernelLaunchFeaturesNV
        1000307004 => Some(24),  // VkPhysicalDeviceCudaKernelLaunchPropertiesNV
        1000309000 => Some(72),  // VkPhysicalDeviceTileShadingFeaturesQCOM
        1000309001 => Some(40),  // VkPhysicalDeviceTileShadingPropertiesQCOM
        1000309002 => Some(32),  // VkRenderPassTileShadingCreateInfoQCOM
        1000310000 => Some(24),  // VkQueryLowLatencySupportNV
        1000311000 => Some(24),  // VkExportMetalObjectCreateInfoEXT
        1000311002 => Some(24),  // VkExportMetalDeviceInfoEXT
        1000311003 => Some(32),  // VkExportMetalCommandQueueInfoEXT
        1000311004 => Some(32),  // VkExportMetalBufferInfoEXT
        1000311005 => Some(24),  // VkImportMetalBufferInfoEXT
        1000311006 => Some(56),  // VkExportMetalTextureInfoEXT
        1000311007 => Some(32),  // VkImportMetalTextureInfoEXT
        1000311008 => Some(32),  // VkExportMetalIOSurfaceInfoEXT
        1000311009 => Some(24),  // VkImportMetalIOSurfaceInfoEXT
        1000311010 => Some(40),  // VkExportMetalSharedEventInfoEXT
        1000311011 => Some(24),  // VkImportMetalSharedEventInfoEXT
        1000314000 => Some(48),  // VkMemoryBarrier2
        1000314007 => Some(24),  // VkPhysicalDeviceSynchronization2Features
        1000314008 => Some(24),  // VkQueueFamilyCheckpointProperties2NV
        1000316000 => Some(256), // VkPhysicalDeviceDescriptorBufferPropertiesEXT
        1000316001 => Some(24),  // VkPhysicalDeviceDescriptorBufferDensityMapPropertiesEXT
        1000316002 => Some(32),  // VkPhysicalDeviceDescriptorBufferFeaturesEXT
        1000316010 => Some(24),  // VkOpaqueCaptureDescriptorDataCreateInfoEXT
        1000316012 => Some(24),  // VkDescriptorBufferBindingPushDescriptorBufferHandleEXT
        1000318004 => Some(32),  // VkMemoryRangeBarriersInfoKHR
        1000318006 => Some(24),  // VkPhysicalDeviceDeviceAddressCommandsFeaturesKHR
        1000320000 => Some(24),  // VkPhysicalDeviceGraphicsPipelineLibraryFeaturesEXT
        1000320001 => Some(24),  // VkPhysicalDeviceGraphicsPipelineLibraryPropertiesEXT
        1000320002 => Some(24),  // VkGraphicsPipelineLibraryCreateInfoEXT
        1000321000 => Some(24),  // VkPhysicalDeviceShaderEarlyAndLateFragmentTestsFeaturesAMD
        1000322000 => Some(24),  // VkPhysicalDeviceFragmentShaderBarycentricPropertiesKHR
        1000323000 => Some(24),  // VkPhysicalDeviceShaderSubgroupUniformControlFlowFeaturesKHR
        1000325000 => Some(24),  // VkPhysicalDeviceZeroInitializeWorkgroupMemoryFeatures
        1000326000 => Some(24),  // VkPhysicalDeviceFragmentShadingRateEnumsPropertiesNV
        1000326001 => Some(32),  // VkPhysicalDeviceFragmentShadingRateEnumsFeaturesNV
        1000326002 => Some(32),  // VkPipelineFragmentShadingRateEnumStateCreateInfoNV
        1000327000 => Some(24),  // VkAccelerationStructureGeometryMotionTrianglesDataNV
        1000327001 => Some(24),  // VkPhysicalDeviceRayTracingMotionBlurFeaturesNV
        1000327002 => Some(24),  // VkAccelerationStructureMotionInfoNV
        1000328000 => Some(40),  // VkPhysicalDeviceMeshShaderFeaturesEXT
        1000328001 => Some(160), // VkPhysicalDeviceMeshShaderPropertiesEXT
        1000330000 => Some(24),  // VkPhysicalDeviceYcbcr2Plane444FormatsFeaturesEXT
        1000332000 => Some(24),  // VkPhysicalDeviceFragmentDensityMap2FeaturesEXT
        1000332001 => Some(32),  // VkPhysicalDeviceFragmentDensityMap2PropertiesEXT
        1000333000 => Some(24),  // VkCopyCommandTransformInfoQCOM
        1000335000 => Some(24),  // VkPhysicalDeviceImageRobustnessFeatures
        1000336000 => Some(32),  // VkPhysicalDeviceWorkgroupMemoryExplicitLayoutFeaturesKHR
        1000338000 => Some(24),  // VkPhysicalDeviceImageCompressionControlFeaturesEXT
        1000338001 => Some(32),  // VkImageCompressionControlEXT
        1000338004 => Some(24),  // VkImageCompressionPropertiesEXT
        1000339000 => Some(24),  // VkPhysicalDeviceAttachmentFeedbackLoopLayoutFeaturesEXT
        1000340000 => Some(24),  // VkPhysicalDevice4444FormatsFeaturesEXT
        1000341000 => Some(24),  // VkPhysicalDeviceFaultFeaturesEXT
        1000342000 => Some(32),  // VkPhysicalDeviceRasterizationOrderAttachmentAccessFeaturesEXT
        1000344000 => Some(24),  // VkPhysicalDeviceRGBA10X6FormatsFeaturesEXT
        1000347000 => Some(40),  // VkPhysicalDeviceRayTracingPipelineFeaturesKHR
        1000347001 => Some(48),  // VkPhysicalDeviceRayTracingPipelinePropertiesKHR
        1000348013 => Some(24),  // VkPhysicalDeviceRayQueryFeaturesKHR
        1000351000 => Some(24),  // VkPhysicalDeviceMutableDescriptorTypeFeaturesEXT
        1000351002 => Some(32),  // VkMutableDescriptorTypeCreateInfoEXT
        1000352000 => Some(24),  // VkPhysicalDeviceVertexInputDynamicStateFeaturesEXT
        1000353000 => Some(56),  // VkPhysicalDeviceDrmPropertiesEXT
        1000354000 => Some(24),  // VkPhysicalDeviceAddressBindingReportFeaturesEXT
        1000354001 => Some(48),  // VkDeviceAddressBindingCallbackDataEXT
        1000355000 => Some(24),  // VkPhysicalDeviceDepthClipControlFeaturesEXT
        1000355001 => Some(24),  // VkPipelineViewportDepthClipControlCreateInfoEXT
        1000356000 => Some(24),  // VkPhysicalDevicePrimitiveTopologyListRestartFeaturesEXT
        1000360000 => Some(40),  // VkFormatProperties3
        1000361000 => Some(24),  // VkPhysicalDevicePresentModeFifoLatestReadyFeaturesKHR
        1000364000 => Some(24),  // VkImportMemoryZirconHandleInfoFUCHSIA
        1000366001 => Some(32),  // VkImportMemoryBufferCollectionFUCHSIA
        1000366002 => Some(32),  // VkBufferCollectionImageCreateInfoFUCHSIA
        1000366005 => Some(32),  // VkBufferCollectionBufferCreateInfoFUCHSIA
        1000369000 => Some(32),  // VkSubpassShadingPipelineCreateInfoHUAWEI
        1000369001 => Some(24),  // VkPhysicalDeviceSubpassShadingFeaturesHUAWEI
        1000369002 => Some(24),  // VkPhysicalDeviceSubpassShadingPropertiesHUAWEI
        1000370000 => Some(24),  // VkPhysicalDeviceInvocationMaskFeaturesHUAWEI
        1000371001 => Some(24),  // VkPhysicalDeviceExternalMemoryRDMAFeaturesNV
        1000372001 => Some(24),  // VkPhysicalDevicePipelinePropertiesFeaturesEXT
        1000373001 => Some(24),  // VkExportFenceSciSyncInfoNV
        1000373005 => Some(24),  // VkExportSemaphoreSciSyncInfoNV
        1000373007 => Some(32),  // VkPhysicalDeviceExternalSciSyncFeaturesNV
        1000374000 => Some(32),  // VkImportMemorySciBufInfoNV
        1000374001 => Some(24),  // VkExportMemorySciBufInfoNV
        1000374004 => Some(24),  // VkPhysicalDeviceExternalMemorySciBufFeaturesNV
        1000375000 => Some(24),  // VkPhysicalDeviceFrameBoundaryFeaturesEXT
        1000375001 => Some(88),  // VkFrameBoundaryEXT
        1000376000 => Some(24),  // VkPhysicalDeviceMultisampledRenderToSingleSampledFeaturesEXT
        1000376001 => Some(24),  // VkSubpassResolvePerformanceQueryEXT
        1000376002 => Some(24),  // VkMultisampledRenderToSingleSampledInfoEXT
        1000377000 => Some(32),  // VkPhysicalDeviceExtendedDynamicState2FeaturesEXT
        1000381000 => Some(24),  // VkPhysicalDeviceColorWriteEnableFeaturesEXT
        1000381001 => Some(32),  // VkPipelineColorWriteCreateInfoEXT
        1000382000 => Some(32),  // VkPhysicalDevicePrimitivesGeneratedQueryFeaturesEXT
        1000386000 => Some(24),  // VkPhysicalDeviceRayTracingMaintenance1FeaturesKHR
        1000387000 => Some(24),  // VkPhysicalDeviceShaderUntypedPointersFeaturesKHR
        1000388000 => Some(24),  // VkPhysicalDeviceGlobalPriorityQueryFeatures
        1000388001 => Some(32),  // VkQueueFamilyGlobalPriorityProperties
        1000390000 => Some(24),  // VkPhysicalDeviceVideoEncodeRgbConversionFeaturesVALVE
        1000390001 => Some(32),  // VkVideoEncodeRgbConversionCapabilitiesVALVE
        1000390002 => Some(24),  // VkVideoEncodeProfileRgbConversionInfoVALVE
        1000390003 => Some(32),  // VkVideoEncodeSessionRgbConversionCreateInfoVALVE
        1000391000 => Some(24),  // VkPhysicalDeviceImageViewMinLodFeaturesEXT
        1000391001 => Some(24),  // VkImageViewMinLodCreateInfoEXT
        1000392000 => Some(24),  // VkPhysicalDeviceMultiDrawFeaturesEXT
        1000392001 => Some(24),  // VkPhysicalDeviceMultiDrawPropertiesEXT
        1000393000 => Some(24),  // VkPhysicalDeviceImage2DViewOf3DFeaturesEXT
        1000395000 => Some(32),  // VkPhysicalDeviceShaderTileImageFeaturesEXT
        1000395001 => Some(32),  // VkPhysicalDeviceShaderTileImagePropertiesEXT
        1000396005 => Some(32),  // VkPhysicalDeviceOpacityMicromapFeaturesEXT
        1000396006 => Some(24),  // VkPhysicalDeviceOpacityMicromapPropertiesEXT
        1000396009 => Some(72),  // VkAccelerationStructureTrianglesOpacityMicromapEXT
        1000397000 => Some(24),  // VkPhysicalDeviceDisplacementMicromapFeaturesNV
        1000397001 => Some(24),  // VkPhysicalDeviceDisplacementMicromapPropertiesNV
        1000397002 => Some(128), // VkAccelerationStructureTrianglesDisplacementMicromapNV
        1000404000 => Some(24),  // VkPhysicalDeviceClusterCullingShaderFeaturesHUAWEI
        1000404001 => Some(48),  // VkPhysicalDeviceClusterCullingShaderPropertiesHUAWEI
        1000404002 => Some(24),  // VkPhysicalDeviceClusterCullingShaderVrsFeaturesHUAWEI
        1000411000 => Some(24),  // VkPhysicalDeviceBorderColorSwizzleFeaturesEXT
        1000411001 => Some(40),  // VkSamplerBorderColorComponentMappingCreateInfoEXT
        1000412000 => Some(24),  // VkPhysicalDevicePageableDeviceLocalMemoryFeaturesEXT
        1000413000 => Some(24),  // VkPhysicalDeviceMaintenance4Features
        1000413001 => Some(24),  // VkPhysicalDeviceMaintenance4Properties
        1000415000 => Some(32),  // VkPhysicalDeviceShaderCorePropertiesARM
        1000416000 => Some(24),  // VkPhysicalDeviceShaderSubgroupRotateFeatures
        1000417000 => Some(24),  // VkDeviceQueueShaderCoreControlCreateInfoARM
        1000417001 => Some(24),  // VkPhysicalDeviceSchedulingControlsFeaturesARM
        1000417002 => Some(24),  // VkPhysicalDeviceSchedulingControlsPropertiesARM
        1000417004 => Some(32), // VkPhysicalDeviceSchedulingControlsDispatchParametersPropertiesARM
        1000418000 => Some(24), // VkPhysicalDeviceImageSlicedViewOf3DFeaturesEXT
        1000418001 => Some(24), // VkImageViewSlicedCreateInfoEXT
        1000420000 => Some(24), // VkPhysicalDeviceDescriptorSetHostMappingFeaturesVALVE
        1000421000 => Some(24), // VkPhysicalDeviceDepthClampZeroOneFeaturesKHR
        1000422000 => Some(24), // VkPhysicalDeviceNonSeamlessCubeMapFeaturesEXT
        1000424000 => Some(24), // VkPhysicalDeviceRenderPassStripedFeaturesARM
        1000424001 => Some(32), // VkPhysicalDeviceRenderPassStripedPropertiesARM
        1000424002 => Some(32), // VkRenderPassStripeBeginInfoARM
        1000424004 => Some(32), // VkRenderPassStripeSubmitInfoARM
        1000425000 => Some(24), // VkPhysicalDeviceFragmentDensityMapOffsetFeaturesEXT
        1000425001 => Some(24), // VkPhysicalDeviceFragmentDensityMapOffsetPropertiesEXT
        1000425002 => Some(32), // VkRenderPassFragmentDensityMapOffsetEndInfoEXT
        1000426000 => Some(24), // VkPhysicalDeviceCopyMemoryIndirectFeaturesNV
        1000426001 => Some(24), // VkPhysicalDeviceCopyMemoryIndirectPropertiesKHR
        1000427000 => Some(24), // VkPhysicalDeviceMemoryDecompressionFeaturesEXT
        1000427001 => Some(32), // VkPhysicalDeviceMemoryDecompressionPropertiesEXT
        1000428000 => Some(32), // VkPhysicalDeviceDeviceGeneratedCommandsComputeFeaturesNV
        1000428001 => Some(40), // VkComputePipelineIndirectBufferInfoNV
        1000429008 => Some(24), // VkPhysicalDeviceRayTracingLinearSweptSpheresFeaturesNV
        1000429009 => Some(96), // VkAccelerationStructureGeometryLinearSweptSpheresDataNV
        1000429010 => Some(88), // VkAccelerationStructureGeometrySpheresDataNV
        1000430000 => Some(24), // VkPhysicalDeviceLinearColorAttachmentFeaturesNV
        1000434000 => Some(24), // VkPhysicalDeviceShaderMaximalReconvergenceFeaturesKHR
        1000435000 => Some(40), // VkApplicationParametersEXT
        1000437000 => Some(24), // VkPhysicalDeviceImageCompressionControlSwapchainFeaturesEXT
        1000440000 => Some(32), // VkPhysicalDeviceImageProcessingFeaturesQCOM
        1000440001 => Some(48), // VkPhysicalDeviceImageProcessingPropertiesQCOM
        1000440002 => Some(40), // VkImageViewSampleWeightCreateInfoQCOM
        1000451000 => Some(32), // VkPhysicalDeviceNestedCommandBufferFeaturesEXT
        1000451001 => Some(24), // VkPhysicalDeviceNestedCommandBufferPropertiesEXT
        1000452000 => Some(24), // VkNativeBufferUsageOHOS
        1000452002 => Some(72), // VkNativeBufferFormatPropertiesOHOS
        1000452003 => Some(24), // VkImportNativeBufferInfoOHOS
        1000452005 => Some(24), // VkExternalFormatOHOS
        1000453000 => Some(24), // VkExternalMemoryAcquireUnmodifiedEXT
        1000453001 => Some(24), // VkNativeBufferOHOS
        1000453002 => Some(24), // VkSwapchainImageCreateInfoOHOS
        1000453003 => Some(24), // VkPhysicalDevicePresentationPropertiesOHOS
        1000455000 => Some(144), // VkPhysicalDeviceExtendedDynamicState3FeaturesEXT
        1000455001 => Some(24), // VkPhysicalDeviceExtendedDynamicState3PropertiesEXT
        1000458000 => Some(24), // VkPhysicalDeviceSubpassMergeFeedbackFeaturesEXT
        1000458001 => Some(24), // VkRenderPassCreationControlEXT
        1000458002 => Some(24), // VkRenderPassCreationFeedbackCreateInfoEXT
        1000458003 => Some(24), // VkRenderPassSubpassFeedbackCreateInfoEXT
        1000459001 => Some(32), // VkDirectDriverLoadingListLUNARG
        1000460003 => Some(32), // VkWriteDescriptorSetTensorARM
        1000460004 => Some(88), // VkPhysicalDeviceTensorPropertiesARM
        1000460005 => Some(32), // VkTensorFormatPropertiesARM
        1000460006 => Some(56), // VkTensorDescriptionARM
        1000460008 => Some(64), // VkTensorMemoryBarrierARM
        1000460009 => Some(40), // VkPhysicalDeviceTensorFeaturesARM
        1000460013 => Some(32), // VkTensorDependencyInfoARM
        1000460014 => Some(24), // VkMemoryDedicatedAllocateInfoTensorARM
        1000460017 => Some(24), // VkExternalMemoryTensorCreateInfoARM
        1000460018 => Some(24), // VkPhysicalDeviceDescriptorBufferTensorFeaturesARM
        1000460019 => Some(40), // VkPhysicalDeviceDescriptorBufferTensorPropertiesARM
        1000460020 => Some(24), // VkDescriptorGetTensorInfoARM
        1000460023 => Some(32), // VkFrameBoundaryTensorsARM
        1000462000 => Some(24), // VkPhysicalDeviceShaderModuleIdentifierFeaturesEXT
        1000462001 => Some(24), // VkPhysicalDeviceShaderModuleIdentifierPropertiesEXT
        1000462002 => Some(32), // VkPipelineShaderStageModuleIdentifierCreateInfoEXT
        1000464000 => Some(24), // VkPhysicalDeviceOpticalFlowFeaturesNV
        1000464001 => Some(64), // VkPhysicalDeviceOpticalFlowPropertiesNV
        1000464002 => Some(24), // VkOpticalFlowImageFormatInfoNV
        1000464010 => Some(32), // VkOpticalFlowSessionCreatePrivateDataInfoNV
        1000465000 => Some(24), // VkPhysicalDeviceLegacyDitheringFeaturesEXT
        1000466000 => Some(24), // VkPhysicalDevicePipelineProtectedAccessFeatures
        1000468000 => Some(24), // VkPhysicalDeviceExternalFormatResolveFeaturesANDROID
        1000468001 => Some(32), // VkPhysicalDeviceExternalFormatResolvePropertiesANDROID
        1000468002 => Some(24), // VkAndroidHardwareBufferFormatResolvePropertiesANDROID
        1000470000 => Some(24), // VkPhysicalDeviceMaintenance5Features
        1000470001 => Some(40), // VkPhysicalDeviceMaintenance5Properties
        1000470005 => Some(24), // VkPipelineCreateFlags2CreateInfo
        1000470006 => Some(24), // VkBufferUsageFlags2CreateInfo
        1000476000 => Some(24), // VkPhysicalDeviceAntiLagFeaturesAMD
        1000478000 => Some(24), // VkPhysicalDeviceDenseGeometryFormatFeaturesAMDX
        1000478001 => Some(56), // VkAccelerationStructureDenseGeometryFormatTrianglesDataAMDX
        1000479000 => Some(24), // VkSurfaceCapabilitiesPresentId2KHR
        1000479001 => Some(32), // VkPresentId2KHR
        1000479002 => Some(24), // VkPhysicalDevicePresentId2FeaturesKHR
        1000480000 => Some(24), // VkSurfaceCapabilitiesPresentWait2KHR
        1000480001 => Some(24), // VkPhysicalDevicePresentWait2FeaturesKHR
        1000481000 => Some(24), // VkPhysicalDeviceRayTracingPositionFetchFeaturesKHR
        1000482000 => Some(24), // VkPhysicalDeviceShaderObjectFeaturesEXT
        1000482001 => Some(32), // VkPhysicalDeviceShaderObjectPropertiesEXT
        1000483000 => Some(24), // VkPhysicalDevicePipelineBinaryFeaturesKHR
        1000483002 => Some(32), // VkPipelineBinaryInfoKHR
        1000483004 => Some(40), // VkPhysicalDevicePipelineBinaryPropertiesKHR
        1000483008 => Some(24), // VkDevicePipelineBinaryInternalCacheControlKHR
        1000484000 => Some(24), // VkPhysicalDeviceTilePropertiesFeaturesQCOM
        1000485000 => Some(24), // VkPhysicalDeviceAmigoProfilingFeaturesSEC
        1000485001 => Some(32), // VkAmigoProfilingSubmitInfoSEC
        1000488000 => Some(24), // VkPhysicalDeviceMultiviewPerViewViewportsFeaturesQCOM
        1000489001 => Some(32), // VkSemaphoreSciSyncCreateInfoNV
        1000489002 => Some(32), // VkPhysicalDeviceExternalSciSync2FeaturesNV
        1000489003 => Some(24), // VkDeviceSemaphoreSciSyncPoolReservationCreateInfoNV
        1000490000 => Some(24), // VkPhysicalDeviceRayTracingInvocationReorderFeaturesNV
        1000490001 => Some(24), // VkPhysicalDeviceRayTracingInvocationReorderPropertiesNV
        1000491000 => Some(24), // VkPhysicalDeviceCooperativeVectorFeaturesNV
        1000491001 => Some(32), // VkPhysicalDeviceCooperativeVectorPropertiesNV
        1000492000 => Some(24), // VkPhysicalDeviceExtendedSparseAddressSpaceFeaturesNV
        1000492001 => Some(32), // VkPhysicalDeviceExtendedSparseAddressSpacePropertiesNV
        1000495000 => Some(24), // VkPhysicalDeviceLegacyVertexAttributesFeaturesEXT
        1000495001 => Some(24), // VkPhysicalDeviceLegacyVertexAttributesPropertiesEXT
        1000496000 => Some(32), // VkLayerSettingsCreateInfoEXT
        1000497000 => Some(24), // VkPhysicalDeviceShaderCoreBuiltinsFeaturesARM
        1000497001 => Some(32), // VkPhysicalDeviceShaderCoreBuiltinsPropertiesARM
        1000498000 => Some(24), // VkPhysicalDevicePipelineLibraryGroupHandlesFeaturesEXT
        1000499000 => Some(24), // VkPhysicalDeviceDynamicRenderingUnusedAttachmentsFeaturesEXT
        1000504000 => Some(24), // VkPhysicalDeviceInternallySynchronizedQueuesFeaturesKHR
        1000505005 => Some(24), // VkLatencySubmissionPresentIdNV
        1000505007 => Some(24), // VkSwapchainLatencyCreateInfoNV
        1000505008 => Some(32), // VkLatencySurfaceCapabilitiesNV
        1000506000 => Some(24), // VkPhysicalDeviceCooperativeMatrixFeaturesKHR
        1000506002 => Some(24), // VkPhysicalDeviceCooperativeMatrixPropertiesKHR
        1000507006 => Some(40), // VkPhysicalDeviceDataGraphFeaturesARM
        1000507007 => Some(56), // VkDataGraphPipelineShaderModuleCreateInfoARM
        1000507010 => Some(24), // VkDataGraphPipelineCompilerControlCreateInfoARM
        1000507013 => Some(32), // VkDataGraphPipelineIdentifierCreateInfoARM
        1000507015 => Some(32), // VkDataGraphPipelineConstantTensorSemiStructuredSparsityInfoARM
        1000507016 => Some(32), // VkDataGraphProcessingEngineCreateInfoARM
        1000510000 => Some(24), // VkPhysicalDeviceMultiviewPerViewRenderAreasFeaturesQCOM
        1000510001 => Some(32), // VkMultiviewPerViewRenderAreasRenderPassBeginInfoQCOM
        1000511000 => Some(24), // VkPhysicalDeviceComputeShaderDerivativesPropertiesKHR
        1000512000 => Some(24), // VkVideoDecodeAV1CapabilitiesKHR
        1000512001 => Some(56), // VkVideoDecodeAV1PictureInfoKHR
        1000512003 => Some(24), // VkVideoDecodeAV1ProfileInfoKHR
        1000512004 => Some(24), // VkVideoDecodeAV1SessionParametersCreateInfoKHR
        1000512005 => Some(24), // VkVideoDecodeAV1DpbSlotInfoKHR
        1000513000 => Some(128), // VkVideoEncodeAV1CapabilitiesKHR
        1000513001 => Some(48), // VkVideoEncodeAV1SessionParametersCreateInfoKHR
        1000513002 => Some(56), // VkVideoEncodeAV1PictureInfoKHR
        1000513003 => Some(24), // VkVideoEncodeAV1DpbSlotInfoKHR
        1000513004 => Some(24), // VkPhysicalDeviceVideoEncodeAV1FeaturesKHR
        1000513005 => Some(24), // VkVideoEncodeAV1ProfileInfoKHR
        1000513006 => Some(40), // VkVideoEncodeAV1RateControlInfoKHR
        1000513007 => Some(64), // VkVideoEncodeAV1RateControlLayerInfoKHR
        1000513008 => Some(88), // VkVideoEncodeAV1QualityLevelPropertiesKHR
        1000513009 => Some(24), // VkVideoEncodeAV1SessionCreateInfoKHR
        1000513010 => Some(32), // VkVideoEncodeAV1GopRemainingFrameInfoKHR
        1000514000 => Some(24), // VkPhysicalDeviceVideoDecodeVP9FeaturesKHR
        1000514001 => Some(24), // VkVideoDecodeVP9CapabilitiesKHR
        1000514002 => Some(48), // VkVideoDecodeVP9PictureInfoKHR
        1000514003 => Some(24), // VkVideoDecodeVP9ProfileInfoKHR
        1000515000 => Some(24), // VkPhysicalDeviceVideoMaintenance1FeaturesKHR
        1000515001 => Some(32), // VkVideoInlineQueryInfoKHR
        1000516000 => Some(24), // VkPhysicalDevicePerStageDescriptorSetFeaturesNV
        1000518000 => Some(24), // VkPhysicalDeviceImageProcessing2FeaturesQCOM
        1000518001 => Some(24), // VkPhysicalDeviceImageProcessing2PropertiesQCOM
        1000518002 => Some(32), // VkSamplerBlockMatchWindowCreateInfoQCOM
        1000519000 => Some(24), // VkSamplerCubicWeightsCreateInfoQCOM
        1000519001 => Some(24), // VkPhysicalDeviceCubicWeightsFeaturesQCOM
        1000519002 => Some(24), // VkBlitImageCubicWeightsInfoQCOM
        1000520000 => Some(24), // VkPhysicalDeviceYcbcrDegammaFeaturesQCOM
        1000520001 => Some(24), // VkSamplerYcbcrConversionYcbcrDegammaCreateInfoQCOM
        1000521000 => Some(24), // VkPhysicalDeviceCubicClampFeaturesQCOM
        1000524000 => Some(24), // VkPhysicalDeviceAttachmentFeedbackLoopDynamicStateFeaturesEXT
        1000525000 => Some(24), // VkPhysicalDeviceVertexAttributeDivisorProperties
        1000527000 => Some(24), // VkPhysicalDeviceUnifiedImageLayoutsFeaturesKHR
        1000527001 => Some(24), // VkAttachmentFeedbackLoopInfoEXT
        1000528000 => Some(24), // VkPhysicalDeviceShaderFloatControls2Features
        1000529001 => Some(80), // VkScreenBufferFormatPropertiesQNX
        1000529002 => Some(24), // VkImportScreenBufferInfoQNX
        1000529003 => Some(24), // VkExternalFormatQNX
        1000529004 => Some(24), // VkPhysicalDeviceExternalMemoryScreenBufferFeaturesQNX
        1000530000 => Some(24), // VkPhysicalDeviceLayeredDriverPropertiesMSFT
        1000544000 => Some(24), // VkPhysicalDeviceShaderExpectAssumeFeatures
        1000545000 => Some(24), // VkPhysicalDeviceMaintenance6Features
        1000545001 => Some(32), // VkPhysicalDeviceMaintenance6Properties
        1000545002 => Some(24), // VkBindMemoryStatus
        1000546000 => Some(24), // VkPhysicalDeviceDescriptorPoolOverallocationFeaturesNV
        1000547000 => Some(24), // VkPhysicalDeviceTileMemoryHeapFeaturesQCOM
        1000547001 => Some(24), // VkPhysicalDeviceTileMemoryHeapPropertiesQCOM
        1000547002 => Some(32), // VkTileMemoryRequirementsQCOM
        1000547003 => Some(24), // VkTileMemoryBindInfoQCOM
        1000547004 => Some(24), // VkTileMemorySizeInfoQCOM
        1000549000 => Some(24), // VkPhysicalDeviceCopyMemoryIndirectFeaturesKHR
        1000551000 => Some(24), // VkDisplaySurfaceStereoCreateInfoNV
        1000551001 => Some(24), // VkDisplayModeStereoPropertiesNV
        1000552000 => Some(40), // VkVideoEncodeIntraRefreshCapabilitiesKHR
        1000552001 => Some(24), // VkVideoEncodeSessionIntraRefreshCreateInfoKHR
        1000552002 => Some(24), // VkVideoEncodeIntraRefreshInfoKHR
        1000552003 => Some(24), // VkVideoReferenceIntraRefreshInfoKHR
        1000552004 => Some(24), // VkPhysicalDeviceVideoEncodeIntraRefreshFeaturesKHR
        1000553000 => Some(24), // VkVideoEncodeQuantizationMapCapabilitiesKHR
        1000553001 => Some(24), // VkVideoFormatQuantizationMapPropertiesKHR
        1000553002 => Some(32), // VkVideoEncodeQuantizationMapInfoKHR
        1000553003 => Some(24), // VkVideoEncodeH264QuantizationMapCapabilitiesKHR
        1000553004 => Some(24), // VkVideoEncodeH265QuantizationMapCapabilitiesKHR
        1000553005 => Some(24), // VkVideoEncodeQuantizationMapSessionParametersCreateInfoKHR
        1000553006 => Some(24), // VkVideoFormatH265QuantizationMapPropertiesKHR
        1000553007 => Some(24), // VkVideoEncodeAV1QuantizationMapCapabilitiesKHR
        1000553008 => Some(24), // VkVideoFormatAV1QuantizationMapPropertiesKHR
        1000553009 => Some(24), // VkPhysicalDeviceVideoEncodeQuantizationMapFeaturesKHR
        1000555000 => Some(24), // VkPhysicalDeviceRawAccessChainsFeaturesNV
        1000556000 => Some(24), // VkExternalComputeQueueDeviceCreateInfoNV
        1000556003 => Some(24), // VkPhysicalDeviceExternalComputeQueuePropertiesNV
        1000558000 => Some(24), // VkPhysicalDeviceShaderRelaxedExtendedInstructionFeaturesKHR
        1000559000 => Some(24), // VkPhysicalDeviceCommandBufferInheritanceFeaturesNV
        1000562000 => Some(24), // VkPhysicalDeviceMaintenance7FeaturesKHR
        1000562001 => Some(48), // VkPhysicalDeviceMaintenance7PropertiesKHR
        1000562002 => Some(32), // VkPhysicalDeviceLayeredApiPropertiesListKHR
        1000562004 => Some(600), // VkPhysicalDeviceLayeredApiVulkanPropertiesKHR
        1000563000 => Some(24), // VkPhysicalDeviceShaderAtomicFloat16VectorFeaturesNV
        1000564000 => Some(24), // VkPhysicalDeviceShaderReplicatedCompositesFeaturesEXT
        1000565000 => Some(56), // VkTensorExplicitTilingFormatPropertiesARM
        1000565001 => Some(24), // VkTensorRollingBackingCreateInfoARM
        1000567000 => Some(24), // VkPhysicalDeviceShaderFloat8FeaturesEXT
        1000568000 => Some(24), // VkPhysicalDeviceRayTracingValidationFeaturesNV
        1000569000 => Some(24), // VkPhysicalDeviceClusterAccelerationStructureFeaturesNV
        1000569001 => Some(48), // VkPhysicalDeviceClusterAccelerationStructurePropertiesNV
        1000569007 => Some(24), // VkRayTracingPipelineClusterAccelerationStructureCreateInfoNV
        1000570000 => Some(24), // VkPhysicalDevicePartitionedAccelerationStructureFeaturesNV
        1000570001 => Some(24), // VkPhysicalDevicePartitionedAccelerationStructurePropertiesNV
        1000570002 => Some(32), // VkWriteDescriptorSetPartitionedAccelerationStructureNV
        1000570005 => Some(24), // VkPartitionedAccelerationStructureFlagsNV
        1000572000 => Some(24), // VkPhysicalDeviceDeviceGeneratedCommandsFeaturesEXT
        1000572001 => Some(64), // VkPhysicalDeviceDeviceGeneratedCommandsPropertiesEXT
        1000572013 => Some(24), // VkGeneratedCommandsPipelineInfoEXT
        1000572014 => Some(32), // VkGeneratedCommandsShaderInfoEXT
        1000573000 => Some(32), // VkPhysicalDeviceFaultFeaturesKHR
        1000573001 => Some(24), // VkPhysicalDeviceFaultPropertiesKHR
        1000574000 => Some(24), // VkPhysicalDeviceMaintenance8FeaturesKHR
        1000574002 => Some(32), // VkMemoryBarrierAccessFlags3KHR
        1000575000 => Some(24), // VkPhysicalDeviceImageAlignmentControlFeaturesMESA
        1000575001 => Some(24), // VkPhysicalDeviceImageAlignmentControlPropertiesMESA
        1000575002 => Some(24), // VkImageAlignmentControlCreateInfoMESA
        1000579000 => Some(32), // VkPhysicalDeviceShaderFmaFeaturesKHR
        1000580000 => Some(24), // VkPushConstantBankInfoNV
        1000580001 => Some(24), // VkPhysicalDevicePushConstantBankFeaturesNV
        1000580002 => Some(32), // VkPhysicalDevicePushConstantBankPropertiesNV
        1000581000 => Some(24), // VkPhysicalDeviceRayTracingInvocationReorderFeaturesEXT
        1000581001 => Some(24), // VkPhysicalDeviceRayTracingInvocationReorderPropertiesEXT
        1000582000 => Some(24), // VkPhysicalDeviceDepthClampControlFeaturesEXT
        1000582001 => Some(32), // VkPipelineViewportDepthClampControlCreateInfoEXT
        1000584000 => Some(24), // VkPhysicalDeviceMaintenance9FeaturesKHR
        1000584001 => Some(24), // VkPhysicalDeviceMaintenance9PropertiesKHR
        1000584002 => Some(24), // VkQueueFamilyOwnershipTransferPropertiesKHR
        1000586000 => Some(24), // VkPhysicalDeviceVideoMaintenance2FeaturesKHR
        1000586001 => Some(32), // VkVideoDecodeH264InlineSessionParametersInfoKHR
        1000586002 => Some(40), // VkVideoDecodeH265InlineSessionParametersInfoKHR
        1000586003 => Some(24), // VkVideoDecodeAV1InlineSessionParametersInfoKHR
        1000590000 => Some(24), // VkPhysicalDeviceHdrVividFeaturesHUAWEI
        1000590001 => Some(32), // VkHdrVividDynamicMetadataHUAWEI
        1000593000 => Some(48), // VkPhysicalDeviceCooperativeMatrix2FeaturesNV
        1000593002 => Some(32), // VkPhysicalDeviceCooperativeMatrix2PropertiesNV
        1000596000 => Some(24), // VkPhysicalDevicePipelineOpacityMicromapFeaturesARM
        1000598000 => Some(24), // VkPhysicalDeviceVideoEncodeFeedback2FeaturesKHR
        1000598001 => Some(24), // VkVideoEncodeFeedback2CapabilitiesKHR
        1000598002 => Some(24), // VkQueryPoolVideoEncodePerPartitionFeedbackCreateInfoKHR
        1000602000 => Some(32), // VkImportMemoryMetalHandleInfoEXT
        1000605000 => Some(24), // VkPhysicalDevicePerformanceCountersByRegionFeaturesARM
        1000605001 => Some(40), // VkPhysicalDevicePerformanceCountersByRegionPropertiesARM
        1000605004 => Some(48), // VkRenderPassPerformanceCountersByRegionBeginInfoARM
        1000607000 => Some(24), // VkPhysicalDeviceShaderInstrumentationFeaturesARM
        1000607001 => Some(24), // VkPhysicalDeviceShaderInstrumentationPropertiesARM
        1000608000 => Some(24), // VkPhysicalDeviceVertexAttributeRobustnessFeaturesEXT
        1000609000 => Some(24), // VkPhysicalDeviceFormatPackFeaturesARM
        1000611000 => Some(24), // VkPhysicalDeviceFragmentDensityMapLayeredFeaturesVALVE
        1000611001 => Some(24), // VkPhysicalDeviceFragmentDensityMapLayeredPropertiesVALVE
        1000611002 => Some(24), // VkPipelineFragmentDensityMapLayeredCreateInfoVALVE
        1000613000 => Some(24), // VkSetPresentConfigNV
        1000613001 => Some(24), // VkPhysicalDevicePresentMeteringFeaturesNV
        1000616000 => Some(24), // VkPhysicalDeviceMultisampledRenderToSwapchainFeaturesEXT
        1000616001 => Some(24), // VkSwapchainFlagsSurfaceCapabilitiesEXT
        1000620000 => Some(24), // VkPhysicalDeviceZeroInitializeDeviceMemoryFeaturesEXT
        1000623000 => Some(24), // VkPhysicalDeviceOpacityMicromapFeaturesKHR
        1000623001 => Some(40), // VkPhysicalDeviceOpacityMicromapPropertiesKHR
        1000623002 => Some(64), // VkAccelerationStructureGeometryMicromapDataKHR
        1000623003 => Some(56), // VkAccelerationStructureTrianglesOpacityMicromapKHR
        1000627000 => Some(24), // VkPhysicalDeviceShader64BitIndexingFeaturesEXT
        1000628000 => Some(24), // VkPhysicalDeviceCustomResolveFeaturesEXT
        1000628002 => Some(40), // VkCustomResolveCreateInfoEXT
        1000629000 => Some(24), // VkPhysicalDeviceDataGraphModelFeaturesQCOM
        1000629001 => Some(24), // VkDataGraphPipelineBuiltinModelCreateInfoQCOM
        1000630000 => Some(24), // VkPhysicalDeviceMaintenance10FeaturesKHR
        1000630001 => Some(32), // VkPhysicalDeviceMaintenance10PropertiesKHR
        1000630002 => Some(24), // VkRenderingAttachmentFlagsInfoKHR
        1000630004 => Some(32), // VkResolveImageModeInfoKHR
        1000631000 => Some(24), // VkPhysicalDeviceDataGraphOpticalFlowFeaturesARM
        1000631002 => Some(56), // VkDataGraphPipelineOpticalFlowCreateInfoARM
        1000631003 => Some(24), // VkDataGraphOpticalFlowImageFormatInfoARM
        1000631005 => Some(24), // VkDataGraphPipelineOpticalFlowDispatchInfoARM
        1000631006 => Some(24), // VkDataGraphPipelineResourceInfoImageLayoutARM
        1000631007 => Some(32), // VkDataGraphPipelineSingleNodeCreateInfoARM
        1000635000 => Some(24), // VkPhysicalDeviceShaderLongVectorFeaturesEXT
        1000635001 => Some(24), // VkPhysicalDeviceShaderLongVectorPropertiesEXT
        1000637000 => Some(24), // VkPhysicalDevicePipelineCacheIncrementalModeFeaturesSEC
        1000642000 => Some(24), // VkPhysicalDeviceShaderUniformBufferUnsizedArrayFeaturesEXT
        1000645001 => Some(24), // VkPhysicalDeviceComputeOccupancyPriorityFeaturesNV
        1000657000 => Some(24), // VkPhysicalDeviceMaintenance11FeaturesKHR
        1000657001 => Some(32), // VkQueueFamilyOptimalImageTransferGranularityPropertiesKHR
        1000659000 => Some(40), // VkPhysicalDeviceCooperativeMatrixMaintenance1FeaturesEXT
        1000662000 => Some(24), // VkPhysicalDeviceShaderSubgroupPartitionedFeaturesEXT
        1000668000 => Some(40), // VkFormatProperties4KHR
        1000668001 => Some(24), // VkImageCreateFlags2CreateInfoKHR
        1000668002 => Some(24), // VkImageUsageFlags2CreateInfoKHR
        1000668003 => Some(24), // VkImageViewUsage2CreateInfoKHR
        1000668004 => Some(24), // VkPhysicalDeviceExtendedFlagsFeaturesKHR
        1000668005 => Some(24), // VkImageStencilUsage2CreateInfoKHR
        1000668006 => Some(24), // VkSharedPresentSurfaceCapabilities2KHR
        1000672000 => Some(32), // VkPhysicalDeviceShaderOCPMicroscalingTypesFeaturesEXT
        1000673000 => Some(32), // VkPhysicalDeviceShaderMixedFloatDotProductFeaturesVALVE
        1000674000 => Some(24), // VkPhysicalDeviceThrottleHintFeaturesSEC
        1000674001 => Some(24), // VkThrottleHintSubmitInfoSEC
        1000676000 => Some(24), // VkDataGraphPipelineNeuralStatisticsCreateInfoARM
        1000676001 => Some(24), // VkDataGraphPipelineSessionNeuralStatisticsCreateInfoARM
        1000676002 => Some(24), // VkPhysicalDeviceDataGraphNeuralAcceleratorStatisticsFeaturesARM
        1000678000 => Some(24), // VkPhysicalDevicePrimitiveRestartIndexFeaturesEXT
        1000687000 => Some(24), // VkPhysicalDeviceImageTilingControlFeaturesEXT
        1000687001 => Some(24), // VkImageTilingControlCreateInfoEXT
        1000689000 => Some(24), // VkPhysicalDeviceCooperativeMatrixDecodeVectorFeaturesNV
        1000707000 => Some(24), // VkPhysicalDevicePrivateDataBaseHandleFeaturesNV
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    #[test]
    fn generated_table_is_substantial_and_unique() {
        assert!(super::NAMES.len() > 100);
        assert_eq!(
            super::NAMES.iter().copied().collect::<BTreeSet<_>>().len(),
            super::NAMES.len(),
        );
    }

    #[test]
    fn callback_chain_layouts_match_the_vulkan_headers() {
        assert_eq!(super::pnext_node_size(1_000_128_004), Some(48));
        assert_eq!(super::pnext_node_size(1_000_284_001), Some(40));
        assert_eq!(super::pnext_node_size(1_000_298_008), Some(40));
        assert_eq!(super::pnext_node_size(1_000_459_001), Some(32));
    }
}
