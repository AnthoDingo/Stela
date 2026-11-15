/* SPDX-License-Identifier: GPL-2.0-or-later */
#ifndef _ASM_STELA_SMP_H
#define _ASM_STELA_SMP_H
/* x86's cross-processor services. Linux code here runs on one processor and
 * cannot reach the others, so this traps (kernel/unsupported.c). */
void wbinvd_on_all_cpus(void);
#endif
