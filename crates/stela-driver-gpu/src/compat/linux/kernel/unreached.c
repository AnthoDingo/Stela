// SPDX-License-Identifier: GPL-2.0-or-later
/* Services a DRM driver imports that nothing it runs under Stela reaches,
 * each a trap. Every one is named, with the reason, in its port's OPEN list
 * (build/i915.rs), which the build checks against what the archive imports.
 * Each takes its type from Linux's own declaration, so a trap of any
 * signature needs no prototype spelled again. */
#include <drm/display/drm_dsc_helper.h>
#include <drm/drm_buddy.h>
#include <drm/drm_mipi_dsi.h>
#include <linux/anon_inodes.h>
#include <linux/auxiliary_bus.h>
#include <linux/device.h>
#include <linux/device/bus.h>
#include <linux/fs.h>
#include <linux/gpu_buddy.h>
#include <linux/i2c.h>
#include <linux/ioport.h>
#include <linux/irq.h>
#include <linux/irqdesc.h>
#include <linux/kstrtox.h>
#include <linux/math.h>
#include <linux/mm.h>
#include <linux/pci.h>
#include <linux/platform_device.h>
#include <linux/relay.h>
#include <linux/seq_file.h>
#include <linux/smp.h>
#include <linux/string.h>
#include <linux/vmalloc.h>
#include <linux/writeback.h>
#include <unsupported.h>

void stela_linux_unreached(void);
void stela_linux_unreached(void)
{
	__builtin_trap();
}

/* MIPI DSI panels and their I2C side channel: display version 11 on, and
 * VLV/CHV and BXT/GLK. */
STELA_UNSUPPORTED_ALIAS(mipi_dsi_attach)
STELA_UNSUPPORTED_ALIAS(mipi_dsi_compression_mode)
STELA_UNSUPPORTED_ALIAS(mipi_dsi_create_packet)
STELA_UNSUPPORTED_ALIAS(mipi_dsi_dcs_nop)
STELA_UNSUPPORTED_ALIAS(mipi_dsi_dcs_read)
STELA_UNSUPPORTED_ALIAS(mipi_dsi_dcs_write)
STELA_UNSUPPORTED_ALIAS(mipi_dsi_dcs_write_buffer)
STELA_UNSUPPORTED_ALIAS(mipi_dsi_generic_write)
STELA_UNSUPPORTED_ALIAS(mipi_dsi_packet_format_is_long)
STELA_UNSUPPORTED_ALIAS(mipi_dsi_picture_parameter_set)
STELA_UNSUPPORTED_ALIAS(mipi_dsi_set_maximum_return_packet_size)
STELA_UNSUPPORTED_ALIAS(i2c_get_adapter)
STELA_UNSUPPORTED_ALIAS(i2c_put_adapter)
/* Display stream compression: display version 10 on. */
STELA_UNSUPPORTED_ALIAS(drm_dsc_compute_rc_parameters)
STELA_UNSUPPORTED_ALIAS(drm_dsc_dp_pps_header_init)
STELA_UNSUPPORTED_ALIAS(drm_dsc_dp_rc_buffer_size)
STELA_UNSUPPORTED_ALIAS(drm_dsc_dump_config)
STELA_UNSUPPORTED_ALIAS(drm_dsc_pps_payload_pack)
STELA_UNSUPPORTED_ALIAS(drm_dsc_set_rc_buf_thresh)
STELA_UNSUPPORTED_ALIAS(drm_dsc_setup_rc_params)
/* Local memory of discrete GPUs. */
STELA_UNSUPPORTED_ALIAS(gpu_buddy_alloc_blocks)
STELA_UNSUPPORTED_ALIAS(gpu_buddy_fini)
STELA_UNSUPPORTED_ALIAS(gpu_buddy_free_list)
STELA_UNSUPPORTED_ALIAS(gpu_buddy_init)
STELA_UNSUPPORTED_ALIAS(drm_buddy_block_print)
STELA_UNSUPPORTED_ALIAS(drm_buddy_print)
STELA_UNSUPPORTED_ALIAS(pci_assign_unassigned_bus_resources)
STELA_UNSUPPORTED_ALIAS(pci_bus_resource_n)
STELA_UNSUPPORTED_ALIAS(pci_rebar_bytes_to_size)
STELA_UNSUPPORTED_ALIAS(pci_rebar_size_supported)
STELA_UNSUPPORTED_ALIAS(pci_resize_resource)
/* The GuC's log relay, the HuC's bus notifier and firmware objects made from
 * data: GuC and HuC are off below version 12. */
STELA_UNSUPPORTED_ALIAS(relay_buf_full)
STELA_UNSUPPORTED_ALIAS(relay_close)
STELA_UNSUPPORTED_ALIAS(relay_flush)
STELA_UNSUPPORTED_ALIAS(relay_open)
STELA_UNSUPPORTED_ALIAS(relay_switch_subbuf)
STELA_UNSUPPORTED_ALIAS(bus_register_notifier)
STELA_UNSUPPORTED_ALIAS(bus_unregister_notifier)
STELA_UNSUPPORTED_ALIAS(kernel_write)
/* LPE audio, the GSC, PXP and the IOSF sideband: other platforms. */
STELA_UNSUPPORTED_ALIAS(__irq_alloc_descs)
STELA_UNSUPPORTED_ALIAS(irq_free_descs)
STELA_UNSUPPORTED_ALIAS(irq_set_chip_and_handler_name)
STELA_UNSUPPORTED_ALIAS(irq_set_chip_data)
STELA_UNSUPPORTED_ALIAS(generic_handle_irq)
STELA_UNSUPPORTED_ALIAS(generic_handle_irq_safe)
STELA_UNSUPPORTED_ALIAS(handle_simple_irq)
STELA_UNSUPPORTED_ALIAS(platform_device_register_full)
STELA_UNSUPPORTED_ALIAS(pci_dev_present)
STELA_UNSUPPORTED_ALIAS(auxiliary_device_init)
STELA_UNSUPPORTED_ALIAS(__auxiliary_device_add)
STELA_UNSUPPORTED_ALIAS(on_each_cpu_cond_mask)
STELA_UNSUPPORTED_ALIAS(device_link_del)
/* debugfs and parsing user strings: Stela has no debugfs and passes no
 * module parameter. */
STELA_UNSUPPORTED_ALIAS(single_open_size)
STELA_UNSUPPORTED_ALIAS(kstrtoint_from_user)
STELA_UNSUPPORTED_ALIAS(strim)
/* Mappings ANV does not make: legacy GEM_MMAP, OA streams, dma-buf mmap. */
STELA_UNSUPPORTED_ALIAS(vm_mmap)
STELA_UNSUPPORTED_ALIAS(find_vma)
STELA_UNSUPPORTED_ALIAS(anon_inode_getfd)
STELA_UNSUPPORTED_ALIAS(compat_vma_mmap)
/* Runtime suspend (no CONFIG_PM), the L3 parity files (IVB/HSW). */
STELA_UNSUPPORTED_ALIAS(pci_d3cold_disable)
STELA_UNSUPPORTED_ALIAS(pci_d3cold_enable)
STELA_UNSUPPORTED_ALIAS(device_create_bin_file)
STELA_UNSUPPORTED_ALIAS(device_remove_bin_file)
/* CPU maps of objects without struct pages, which on version 9 back only
 * what the GPU alone reads and writes. */
STELA_UNSUPPORTED_ALIAS(vmap_pfn)
/* Writing shmem objects out, which i915 does only from kswapd and on
 * hibernation. */
STELA_UNSUPPORTED_ALIAS(writeback_iter)
STELA_UNSUPPORTED_ALIAS(folio_redirty_for_writepage)
/* MCHBAR allocation, which firmware makes unnecessary by enabling it. */
STELA_UNSUPPORTED_ALIAS(pci_bus_alloc_resource)
STELA_UNSUPPORTED_ALIAS(pcibios_align_resource)
STELA_UNSUPPORTED_ALIAS(release_resource)
/* i915's CPU fault fills page tables itself, which Linux code under Stela
 * does not own. Reached by ANV's mappings: to be supplied with them. */
STELA_UNSUPPORTED_ALIAS(apply_to_page_range)
STELA_UNSUPPORTED_ALIAS(zap_special_vma_range)
