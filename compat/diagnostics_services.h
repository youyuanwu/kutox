/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_DIAGNOSTICS_SERVICES_H
#define KUTOX_DIAGNOSTICS_SERVICES_H
int kutox_vprint(const char *, va_list);
int printk(const char *, ...);
void netdev_printk(const struct net_device *, const char *, ...);
void dev_printk(const struct device *, const char *, ...);
void netdev_warn_once(const struct net_device *, const char *, ...);
bool warn_on(bool, const char *);
void bug_on(bool);
_Noreturn void bug(void);
void prefetch(const void *);
int fls(unsigned int);
void cpu_to_le16s(u16 *);
void le16_to_cpus(__le16 *);
bool net_ratelimit(void);
u32 netif_msg_init(int, int);
bool netif_msg_hw(u32);
bool netif_msg_rx_status(u32);
bool netif_msg_tx_done(u32);
void netif_log(u32, const char *, const struct net_device *, const char *, ...);
void print_hex_dump(const char *, const char *, int, int, int, const void *, size_t, bool);
#endif
