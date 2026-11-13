/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_DRIVER_CHECK_MARKER_H
#define KUTOX_DRIVER_CHECK_MARKER_H
#ifndef KUTOX_DRIVER_COMPILE_ONLY
#error "Audit objects require the compile-only build flag"
#endif
static const char kutox_compile_only[] __attribute__((used, section(".kutox.compile_only"))) =
    "Linux driver audit only; not a runtime provider";
#endif
