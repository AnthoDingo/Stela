/* SPDX-License-Identifier: GPL-2.0-or-later */
/* The NIR options a device's shaders are built with, for the AIR compiler,
 * which reaches ANV by no route Vulkan offers. brw keeps a set per stage,
 * each a copy of one but for fields its own passes read (brw_compiler.c),
 * and the Vulkan runtime gives NIR handed to it its stage's own set
 * (vk_pipeline.c), so the AIR compiler builds with the compute stage's. */
#include "anv_private.h"

const struct nir_shader_compiler_options *stela_anv_nir_options(VkPhysicalDevice physical)
{
	VK_FROM_HANDLE(anv_physical_device, pdev, physical);
	return &pdev->compiler->nir_options[MESA_SHADER_COMPUTE];
}
