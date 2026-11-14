/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_BRIDGE_H
#define KUTOX_BRIDGE_H

#include <stdint.h>
#include <stddef.h>

enum kutox_stage {
    KUTOX_VALIDATE = 0,
    KUTOX_MAC_TYPE = 1,
    KUTOX_EEPROM_PARAMS = 2,
    KUTOX_READ_EEPROM = 3,
    KUTOX_COMPLETE = 4,
};

struct kutox_result {
    int32_t code;
    uint32_t stage;
    uint32_t denied_grants;
    uint32_t eeprom_reads;
    uint8_t mac[6];
};

_Static_assert(sizeof(struct kutox_result) == 24, "Rust/C result size mismatch");
_Static_assert(offsetof(struct kutox_result, mac) == 16, "Rust/C MAC offset mismatch");

#endif
