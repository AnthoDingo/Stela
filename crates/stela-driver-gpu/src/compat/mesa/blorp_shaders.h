/* SPDX-License-Identifier: GPL-2.0-or-later */
/* BLORP's OpenCL helpers (src/intel/blorp/blorp_shaders.cl), which Mesa
 * compiles with LLVM at build time and vtn_bindgen2 turns into functions that
 * build them in NIR, declared here as it declares them; `blorpcl.c` builds
 * them instead, so the boot image needs neither. */
#ifndef STELA_MESA_BLORP_SHADERS_H
#define STELA_MESA_BLORP_SHADERS_H
#include "nir.h"

nir_def *blorp_check_in_bounds(nir_builder *b, nir_def *bounds_rect, nir_def *pos);
void blorp_copy_memory_indirect_shader(nir_builder *b, nir_def *indirect_buf,
				       nir_def *copy_count, nir_def *stride_bytes,
				       nir_def *global_id);
void blorp_copy_memory_to_image_indirect_shader(nir_builder *b, nir_def *indirect_buf,
						nir_def *stride_bytes, nir_def *copy_idx,
						nir_def *max_layer, nir_def *dest_coord_offsets,
						nir_def *global_id, nir_def *dimensions,
						nir_def *forced_layer_or_z, nir_def *format_Bpb,
						nir_def *format_block_size,
						nir_def *is_block_compressed);
#endif
