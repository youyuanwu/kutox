/* SPDX-License-Identifier: GPL-2.0-only */
#include "net/services.h"
unsigned int kutox_tcp_header_bytes(const struct tcphdr *header)
{
    return header->doff * 4U;
}
