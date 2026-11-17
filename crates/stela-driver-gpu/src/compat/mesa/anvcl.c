// SPDX-License-Identifier: GPL-2.0-or-later
/* ANV's internal kernels (`anvcl.h`), none built yet: each refuses when ANV
 * first asks for it, which is when a command buffer needs it (indirect draws
 * generated on the GPU, query copies and memory copies by shader), never at
 * device or instance creation. Stela claims only graphics version 9, so the
 * other versions' are never reached; version 9's three, write_draw,
 * query_copy and memcpy, are to be built in NIR as `nvkcl.c` builds NVK's.
 * The device-generated-commands kernels need version 12.5. */
#include "anvcl.h"
#include <stela/host.h>

#define ANVCL_REFUSE(gfx, name, count)                                          \
	void gfx##_libanv_##name(nir_builder *b, ANVCL_P##count)                \
	{                                                                       \
		(void)b;                                                        \
		stela_libc_fatal("ANV's internal kernel " #gfx "_libanv_" #name \
				 " is not built for the boot image");           \
	}
ANVCL_VERSIONS(ANVCL_REFUSE)
