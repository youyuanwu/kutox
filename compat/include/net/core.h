/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_NET_CORE_H
#define KUTOX_NET_CORE_H
#include "common/core.h"
#include <uapi/linux/socket.h>
#include <uapi/linux/if_ether.h>
#include <uapi/linux/mii.h>
#include <uapi/linux/sockios.h>
struct sockaddr { unsigned short sa_family; char sa_data[14]; };
#include <uapi/linux/if.h>
#include <uapi/linux/in.h>
#include <uapi/linux/ip.h>
#include <uapi/linux/ipv6.h>
#include <uapi/linux/tcp.h>
#include <uapi/linux/udp.h>
#define ETH_FCS_LEN 4
#define NET_IP_ALIGN 2
#define CAP_NET_ADMIN 12
struct net_device;
void netdev_printk(const struct net_device *, const char *, ...);
void netif_log(u32, const char *, const struct net_device *, const char *, ...);
#define netif_err(a,l,d,...) netif_log((a)->msg_enable,#l,d,__VA_ARGS__)
#define netif_info(a,l,d,...) netif_log((a)->msg_enable,#l,d,__VA_ARGS__)
#define netif_warn(a,l,d,...) netif_log((a)->msg_enable,#l,d,__VA_ARGS__)
#define netif_dbg(a,l,d,...) netif_log((a)->msg_enable,#l,d,__VA_ARGS__)
#define netdev_dbg(d,...) netdev_printk(d,__VA_ARGS__)
#define netdev_info(d,...) netdev_printk(d,__VA_ARGS__)
#define netdev_err(d,...) netdev_printk(d,__VA_ARGS__)
#define netdev_warn(d,...) netdev_printk(d,__VA_ARGS__)
bool net_ratelimit(void);
void netdev_warn_once(const struct net_device *, const char *, ...);
#define netdev_WARN_ONCE(d,...) netdev_warn_once(d,__VA_ARGS__)
#endif
