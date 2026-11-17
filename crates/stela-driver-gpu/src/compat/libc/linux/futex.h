/* SPDX-License-Identifier: GPL-2.0-or-later */
/* Linux's futex operations, of which `syscall(SYS_futex, ...)` supplies
 * FUTEX_WAKE and FUTEX_WAIT_BITSET on the monotonic clock with every bit
 * matched (`pthread.c`): what Mesa's util/futex.c calls. */
#ifndef STELA_LIBC_LINUX_FUTEX_H
#define STELA_LIBC_LINUX_FUTEX_H
#define FUTEX_WAIT 0
#define FUTEX_WAKE 1
#define FUTEX_WAIT_BITSET 9
#define FUTEX_PRIVATE_FLAG 128
#define FUTEX_CLOCK_REALTIME 256
#define FUTEX_BITSET_MATCH_ANY 0xffffffff
#endif
