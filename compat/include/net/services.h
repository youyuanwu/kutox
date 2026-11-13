/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_DRIVER_SERVICES_H
#define KUTOX_DRIVER_SERVICES_H
#include "net/driver.h"
#if KUTOX_GUEST_TESTS
extern void kutox_platform_fail(const char *) __attribute__((noreturn));
#endif
#endif
