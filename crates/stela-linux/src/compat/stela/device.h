/* SPDX-License-Identifier: GPL-2.0-or-later */
#ifndef STELA_DEVICE_H
#define STELA_DEVICE_H
#include <linux/types.h>

struct device;

/* Undoes the managed actions on dev, last first: a driver's teardown. */
void stela_linux_devres_release(struct device *dev);

/* Lend Linux code [start, start + size) of physical memory its owner keeps,
 * for one driver at a time to request (__devm_request_region): Linux code
 * here has no resource tree of its own. On the entered owner. */
int stela_linux_lend_region(u64 start, u64 size);
#endif
