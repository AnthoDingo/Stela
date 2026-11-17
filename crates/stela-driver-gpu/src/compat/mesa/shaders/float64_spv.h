/* SPDX-License-Identifier: GPL-2.0-or-later */
/* What glsl2spirv.py makes of src/compiler/glsl/float64.glsl: the library ANV
 * links into shaders on devices without 64-bit floats (anv_util.c). Graphics
 * version 9 has them, and the image's glslangValidator compiles nothing
 * (`xtask/src/gpu/nvk.rs`), so this is the empty module it gives, from every
 * build host. */
#ifndef STELA_MESA_FLOAT64_SPV_H
#define STELA_MESA_FLOAT64_SPV_H
#include <stdint.h>
static const uint32_t float64_spv_source[] = {0x07230203, 0x00010500, 0, 1, 0};
#endif
