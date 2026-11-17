/* SPDX-License-Identifier: GPL-2.0-or-later */
/* ANV's internal kernels. Mesa compiles them from OpenCL C with LLVM at build
 * time (src/intel/shaders/, one SPIR-V module per hardware version) and
 * vtn_bindgen2 turns each into a function that builds it in NIR, declared in
 * intel_gfx<ver>_shaders_binding.h; genX_internal_kernels.c calls them. The
 * boot image builds neither, so `anvcl.c` defines these functions, with the
 * parameters genX_internal_kernels.c passes, each a nir_def. */
#ifndef STELA_MESA_ANVCL_H
#define STELA_MESA_ANVCL_H
#include "nir.h"

#define ANVCL_P1 nir_def *a1
#define ANVCL_P2 ANVCL_P1, nir_def *a2
#define ANVCL_P3 ANVCL_P2, nir_def *a3
#define ANVCL_P4 ANVCL_P3, nir_def *a4
#define ANVCL_P5 ANVCL_P4, nir_def *a5
#define ANVCL_P6 ANVCL_P5, nir_def *a6
#define ANVCL_P7 ANVCL_P6, nir_def *a7
#define ANVCL_P8 ANVCL_P7, nir_def *a8
#define ANVCL_P9 ANVCL_P8, nir_def *a9
#define ANVCL_P10 ANVCL_P9, nir_def *a10
#define ANVCL_P11 ANVCL_P10, nir_def *a11
#define ANVCL_P12 ANVCL_P11, nir_def *a12
#define ANVCL_P13 ANVCL_P12, nir_def *a13
#define ANVCL_P14 ANVCL_P13, nir_def *a14
#define ANVCL_P15 ANVCL_P14, nir_def *a15
#define ANVCL_P16 ANVCL_P15, nir_def *a16
#define ANVCL_P17 ANVCL_P16, nir_def *a17
#define ANVCL_P18 ANVCL_P17, nir_def *a18
#define ANVCL_P19 ANVCL_P18, nir_def *a19

/* Every version's kernels, by name and parameter count; ray tracing's from
 * version 12.5 on, where genX_internal_kernels.c calls it. */
#define ANVCL_KERNELS(X, gfx)                                                   \
	X(gfx, write_draw, 16)                                                  \
	X(gfx, query_copy, 10)                                                  \
	X(gfx, memcpy, 4)                                                       \
	X(gfx, preprocess_gfx_generate, 17)                                     \
	X(gfx, preprocess_cs_generate, 19)                                      \
	X(gfx, postprocess_cs_generate, 12)                                     \
	X(gfx, dgc_dump, 3)
#define ANVCL_RT_KERNELS(X, gfx) X(gfx, preprocess_rt_generate, 19)

/* The versions genX_internal_kernels.c is built for (libanv_per_hw_ver). */
#define ANVCL_VERSIONS(X)                                                       \
	ANVCL_KERNELS(X, gfx9)                                                  \
	ANVCL_KERNELS(X, gfx11)                                                 \
	ANVCL_KERNELS(X, gfx12)                                                 \
	ANVCL_KERNELS(X, gfx125)                                                \
	ANVCL_RT_KERNELS(X, gfx125)                                             \
	ANVCL_KERNELS(X, gfx20)                                                 \
	ANVCL_RT_KERNELS(X, gfx20)                                              \
	ANVCL_KERNELS(X, gfx30)                                                 \
	ANVCL_RT_KERNELS(X, gfx30)                                              \
	ANVCL_KERNELS(X, gfx35)                                                 \
	ANVCL_RT_KERNELS(X, gfx35)

#define ANVCL_DECLARE(gfx, name, count) void gfx##_libanv_##name(nir_builder *b, ANVCL_P##count);
ANVCL_VERSIONS(ANVCL_DECLARE)
#undef ANVCL_DECLARE
#endif
