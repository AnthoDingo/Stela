/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _ASM_STELA_IO_H
#define _ASM_STELA_IO_H
/* asm-generic's MMIO accessors: volatile accesses with x86's fences. Device
 * memory is mapped by Stela's device owners, so the kernel-API's ioremap()
 * maps nothing. */
#include <linux/types.h>

void __iomem *ioremap(phys_addr_t offset, size_t size);
#define ioremap ioremap
void __iomem *ioremap_wc(phys_addr_t offset, size_t size);
#define ioremap_wc ioremap_wc
void iounmap(volatile void __iomem *addr);
#define iounmap iounmap

/* x86's byte port I/O, which asm-generic leaves to the architecture. i915
 * disables the legacy VGA plane through it from G4X on
 * (WaEnableVGAAccessThroughIOPort, display/intel_vga.c). */
#define _inb _inb
static inline u8 _inb(unsigned long port)
{
	u8 value;

	asm volatile("inb %w1, %b0" : "=a"(value) : "Nd"((u16)port));
	return value;
}
#define _outb _outb
static inline void _outb(u8 value, unsigned long port)
{
	asm volatile("outb %b0, %w1" : : "a"(value), "Nd"((u16)port));
}

#include <asm-generic/io.h>
#endif
