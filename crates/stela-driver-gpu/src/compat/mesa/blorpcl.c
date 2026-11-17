// SPDX-License-Identifier: GPL-2.0-or-later
/* BLORP's OpenCL helpers (`blorp_shaders.h`), built in NIR as
 * src/intel/blorp/blorp_shaders.cl writes them. The copy from memory to an
 * image named by an indirect buffer (vkCmdCopyMemoryToImageIndirectKHR)
 * refuses until it is written. */
#include "blorp_shaders.h"
#include "nir_builder.h"
#include <stela/host.h>

/* Whether `pos` lies in [x0, x1) by [y0, y1), the four words of
 * `bounds_rect` in that order: blits and clears that kill outside it. */
nir_def *blorp_check_in_bounds(nir_builder *b, nir_def *bounds_rect, nir_def *pos)
{
	nir_def *x = nir_channel(b, pos, 0);
	nir_def *y = nir_channel(b, pos, 1);
	nir_def *in_x = nir_iand(b, nir_uge(b, x, nir_channel(b, bounds_rect, 0)),
				 nir_ult(b, x, nir_channel(b, bounds_rect, 1)));
	nir_def *in_y = nir_iand(b, nir_uge(b, y, nir_channel(b, bounds_rect, 2)),
				 nir_ult(b, y, nir_channel(b, bounds_rect, 3)));
	return nir_iand(b, in_x, in_y);
}

/* vkCmdCopyMemoryIndirectKHR: for each of `copy_count` commands,
 * `stride_bytes` apart, this invocation copies the 32-bit word `global_id` of
 * the copy when the copy has it. A command is only 4-byte aligned, so its
 * 64-bit fields are read as two words. */
void blorp_copy_memory_indirect_shader(nir_builder *b, nir_def *indirect_buf,
				       nir_def *copy_count, nir_def *stride_bytes,
				       nir_def *global_id)
{
	/* VkCopyMemoryIndirectCommandKHR. */
	enum { SOURCE = 0, DESTINATION = 8, SIZE = 16 };
	nir_variable *copy = nir_local_variable_create(b->impl, glsl_uint_type(), "copy");
	nir_store_var(b, copy, nir_imm_int(b, 0), 1);
	nir_push_loop(b);
	{
		nir_def *c = nir_load_var(b, copy);
		nir_break_if(b, nir_uge(b, c, copy_count));
		/* The kernel's word index c * (stride / 4), truncated to 32 bits. */
		nir_def *word = nir_u2u32(b, nir_imul(b, nir_u2u64(b, c),
						      nir_udiv_imm(b, stride_bytes, 4)));
		nir_def *command = nir_iadd(b, indirect_buf, nir_u2u64(b, nir_imul_imm(b, word, 4)));
		nir_def *source = nir_pack_64_2x32(
			b, nir_load_global(b, 2, 32, nir_iadd_imm(b, command, SOURCE)));
		nir_def *destination = nir_pack_64_2x32(
			b, nir_load_global(b, 2, 32, nir_iadd_imm(b, command, DESTINATION)));
		/* The size in words, truncated to the kernel's 32-bit count. */
		nir_def *size = nir_pack_64_2x32(
			b, nir_load_global(b, 2, 32, nir_iadd_imm(b, command, SIZE)));
		nir_def *words = nir_u2u32(b, nir_ushr_imm(b, size, 2));
		nir_push_if(b, nir_ult(b, global_id, words));
		nir_def *offset = nir_u2u64(b, nir_imul_imm(b, global_id, 4));
		nir_store_global(b, nir_load_global(b, 1, 32, nir_iadd(b, source, offset)),
				 nir_iadd(b, destination, offset));
		nir_pop_if(b, NULL);
		nir_store_var(b, copy, nir_iadd_imm(b, c, 1), 1);
	}
	nir_pop_loop(b, NULL);
}

void blorp_copy_memory_to_image_indirect_shader(nir_builder *b, nir_def *indirect_buf,
						nir_def *stride_bytes, nir_def *copy_idx,
						nir_def *max_layer, nir_def *dest_coord_offsets,
						nir_def *global_id, nir_def *dimensions,
						nir_def *forced_layer_or_z, nir_def *format_Bpb,
						nir_def *format_block_size,
						nir_def *is_block_compressed)
{
	(void)b;
	(void)indirect_buf;
	(void)stride_bytes;
	(void)copy_idx;
	(void)max_layer;
	(void)dest_coord_offsets;
	(void)global_id;
	(void)dimensions;
	(void)forced_layer_or_z;
	(void)format_Bpb;
	(void)format_block_size;
	(void)is_block_compressed;
	stela_libc_fatal("BLORP's indirect copy from memory to an image is not built for the "
			 "boot image");
}
