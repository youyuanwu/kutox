/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_DRIVER_CHECK_MARKER_H
#define KUTOX_DRIVER_CHECK_MARKER_H
#ifndef KUTOX_DRIVER_COMPILE_ONLY
#error "Driver-check declarations are not a runtime ABI"
#endif
static const char kutox_compile_only[] __attribute__((used, section(".kutox.compile_only"))) =
    "e1000 declarations only; no runtime providers";
#endif
