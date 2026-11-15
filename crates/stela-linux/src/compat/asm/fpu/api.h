/* SPDX-License-Identifier: GPL-2.0-or-later */
#ifndef _ASM_STELA_FPU_API_H
#define _ASM_STELA_FPU_API_H
/* Kernel use of the vector registers, from x86's asm/fpu/api.h. Stela saves
 * no vector state for Linux code, so these trap (kernel/unsupported.c) and
 * cpufeature.h reports no feature that needs them. */
/* What x86's version reaches through its own includes and its users rely
 * on: the feature tests and static keys (i915_memcpy.c). */
#include <linux/jump_label.h>
#include <asm/cpufeature.h>

void kernel_fpu_begin(void);
void kernel_fpu_end(void);
#endif
