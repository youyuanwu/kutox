/* SPDX-License-Identifier: GPL-2.0-only */
/* Kept in C so the compiler interprets the original Linux TCP bitfield.
 * Rust receives a byte count without duplicating the bitfield layout. */
#include "net/services.h"
unsigned int kutox_tcp_header_bytes(const struct tcphdr *header)
{
    return header->doff * 4U;
}
