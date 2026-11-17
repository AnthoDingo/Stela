// SPDX-License-Identifier: GPL-2.0-or-later
/* DRM's cache maintenance, drivers/gpu/drm/drm_cache.c unchanged, built with
 * its x86 code. Stela is not CONFIG_X86, so the file's other branch would
 * warn and flush nothing; but it runs on x86 processors, and a display engine
 * that does not snoop the CPU's caches, as Intel's, scans out only what
 * clflush wrote back (i915 flushes a framebuffer as it pins it for display).
 * Its headers are read first, without CONFIG_X86, so only the file's own
 * code takes the x86 branch: clflush when the processor has it, never the
 * SSE4.1 copy, which cpufeature.h does not report. */
#include <linux/cc_platform.h>
#include <linux/export.h>
#include <linux/highmem.h>
#include <linux/ioport.h>
#include <linux/iosys-map.h>
#include <xen/xen.h>
#include <drm/drm_cache.h>
#include <asm/cpufeature.h>
#include <asm/fpu/api.h>
#include <asm/processor.h>

/* x86's asm/special_insns.h: writes back and invalidates the cache line at
 * address. CLFLUSHOPT is the same write-back without CLFLUSH's ordering, which
 * drm_cache.c fences itself; CLFLUSH is what every processor with the
 * CLFLUSH feature has (Intel SDM vol. 2A, "CLFLUSH" and "CLFLUSHOPT"). */
static inline void clflushopt(volatile void *address)
{
	asm volatile("clflush %0" : "+m"(*(volatile char *)address));
}

#define CONFIG_X86 1
/* Linux's own code, held to Linux's warnings rather than Stela's. */
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wconversion"
#pragma clang diagnostic ignored "-Wsign-conversion"
#pragma clang diagnostic ignored "-Wsign-compare"
#pragma clang diagnostic ignored "-Wunused-parameter"
#include "../../../../../../third_party/linux/drivers/gpu/drm/drm_cache.c"
#pragma clang diagnostic pop
