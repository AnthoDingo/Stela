// SPDX-License-Identifier: GPL-2.0-or-later
/* Kernel services Stela does not provide yet. */
#include <linux/folio_batch.h>
#include <linux/fs.h>
#include <linux/io.h>
#include <linux/mm.h>
#include <linux/platform_device.h>
#include <linux/rcupdate.h>
#include <linux/seq_file.h>
#include <linux/swap.h>
#include <asm/fpu/api.h>
#include <asm/smp.h>
#include <stela/linux.h>
#include <unsupported.h>

/* Writing shmem pages out to swap, which Stela does not have. */
struct swap_iocb;
STELA_UNSUPPORTED(int, shmem_writeout, struct folio *, struct swap_iocb **, struct list_head *)
STELA_UNSUPPORTED(bool, folio_clear_dirty_for_io, struct folio *)
/* Transparent huge mappings of shmem-backed objects. */
STELA_UNSUPPORTED(unsigned long, mm_get_unmapped_area, struct file *, unsigned long,
		  unsigned long, unsigned long, unsigned long)
/* debugfs's seq_file operations, which no file opens: Stela has no debugfs. */
STELA_UNSUPPORTED(int, single_open, struct file *, int (*)(struct seq_file *, void *), void *)
STELA_UNSUPPORTED(int, single_release, struct inode *, struct file *)
STELA_UNSUPPORTED(ssize_t, seq_read, struct file *, char __user *, size_t, loff_t *)
STELA_UNSUPPORTED(loff_t, seq_lseek, struct file *, loff_t, int)
/* Platform devices, of which only HDMI audio codecs reach DRM. */
STELA_UNSUPPORTED(void, platform_device_unregister, struct platform_device *)
/* Vector registers, whose state Stela does not save for Linux code; no
 * feature test reports a feature that needs them (asm/cpufeature.h). */
STELA_UNSUPPORTED(void, kernel_fpu_begin, void)
STELA_UNSUPPORTED(void, kernel_fpu_end, void)
/* A cache write-back on every processor, which Linux code here cannot reach.
 * i915 asks for it only on resume and for dma-bufs on hardware without a
 * last-level cache shared with the GPU; Stela does not suspend, and version 9
 * shares its LLC. */
STELA_UNSUPPORTED(void, wbinvd_on_all_cpus, void)
