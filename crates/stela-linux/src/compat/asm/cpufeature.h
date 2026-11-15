/* SPDX-License-Identifier: GPL-2.0-or-later */
#ifndef _ASM_STELA_CPUFEATURE_H
#define _ASM_STELA_CPUFEATURE_H
/* The x86 feature tests Linux code here makes, each answered for this
 * processor. A feature not named below does not compile: Stela keeps no
 * feature array to answer it from. */
#include <linux/types.h>

#define static_cpu_has(feature) __stela_cpu_has_##feature()
#define boot_cpu_has(feature) static_cpu_has(feature)

/* CPUID leaf 1's feature words (Intel SDM vol. 2A, "CPUID"). */
static inline u32 __stela_cpuid_1(bool edx)
{
	u32 eax = 1, ebx, ecx = 0, d;

	asm volatile("cpuid" : "+a"(eax), "=b"(ebx), "+c"(ecx), "=d"(d));
	return edx ? d : ecx;
}

/* CPUID.01H:EDX[19]. */
static inline bool __stela_cpu_has_X86_FEATURE_CLFLUSH(void)
{
	return __stela_cpuid_1(true) & (1u << 19);
}
/* CPUID.01H:ECX[31], set by a hypervisor for its guests. */
static inline bool __stela_cpu_has_X86_FEATURE_HYPERVISOR(void)
{
	return __stela_cpuid_1(false) & (1u << 31);
}
/* SSE4.1 needs the vector registers, which Linux code here may not use: it
 * is built without them (-mgeneral-regs-only) and kernel_fpu_begin() is not
 * supplied. */
static inline bool __stela_cpu_has_X86_FEATURE_XMM4_1(void)
{
	return false;
}
#endif
