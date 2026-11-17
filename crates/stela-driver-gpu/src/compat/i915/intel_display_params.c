// SPDX-License-Identifier: MIT
/* i915's display parameters, display/intel_display_params.c unchanged, with
 * the values Stela gives them in place of a kernel command line, which a
 * port's registrations do not carry for this unit. */
#include "../../../../../third_party/linux/drivers/gpu/drm/i915/display/intel_display_params.c"

/* Panel self refresh and framebuffer compression off: Stela's console writes
 * the front buffer it scans out without telling DRM, and with either on the
 * display keeps showing the frame it last fetched (the Latitude 7490's eDP
 * froze on the first status line). Page flips, as the guest's frames are
 * presented, are tracked with both on.
 * ponytail: both off for the whole boot; a DIRTYFB from the console would
 * keep their power saving. */
void stela_i915_display_params(void)
{
	intel_display_modparams.enable_psr = 0;
	intel_display_modparams.enable_fbc = 0;
}
