/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_BLOCK_SERVICES_H
#define KUTOX_BLOCK_SERVICES_H
#include "block/driver.h"

#define KUTOX_BLOCK_BYTES 4096
int kutox_block_prepare(struct request *, sector_t, size_t, u64 *);
int kutox_block_submit(u64);
int kutox_block_result(u64, void *, size_t);
int kutox_block_cancel(u64);
int kutox_block_release(u64);
int kutox_block_pump(void);
bool kutox_block_quiescent(void);
#if KUTOX_GUEST_TESTS
int kutox_block_context(unsigned int);
void kutox_block_fail(const char *) __attribute__((noreturn));
#endif
#endif
