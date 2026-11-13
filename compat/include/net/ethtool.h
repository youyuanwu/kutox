/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_NET_ETHTOOL_H
#define KUTOX_NET_ETHTOOL_H
#include <uapi/linux/ethtool.h>
struct netlink_ext_ack;
struct kernel_ethtool_ringparam { u32 rx_buf_len; };
struct kernel_ethtool_coalesce { u8 use_cqe_mode_tx, use_cqe_mode_rx; };
struct kernel_ethtool_ts_info;
struct ethtool_link_ksettings {
    struct ethtool_link_settings base;
    struct { unsigned long supported[4], advertising[4], lp_advertising[4]; } link_modes;
};
enum ethtool_phys_id_state { ETHTOOL_ID_INACTIVE, ETHTOOL_ID_ACTIVE, ETHTOOL_ID_ON, ETHTOOL_ID_OFF };
#define ETHTOOL_COALESCE_RX_USECS BIT(0)
struct ethtool_ops {
    u32 supported_coalesce_params;
    void (*get_drvinfo)(struct net_device *, struct ethtool_drvinfo *);
    int (*get_regs_len)(struct net_device *);
    void (*get_regs)(struct net_device *, struct ethtool_regs *, void *);
    void (*get_wol)(struct net_device *, struct ethtool_wolinfo *);
    int (*set_wol)(struct net_device *, struct ethtool_wolinfo *);
    u32 (*get_msglevel)(struct net_device *);
    void (*set_msglevel)(struct net_device *, u32);
    int (*nway_reset)(struct net_device *);
    u32 (*get_link)(struct net_device *);
    int (*get_eeprom_len)(struct net_device *);
    int (*get_eeprom)(struct net_device *, struct ethtool_eeprom *, u8 *);
    int (*set_eeprom)(struct net_device *, struct ethtool_eeprom *, u8 *);
    void (*get_ringparam)(struct net_device *, struct ethtool_ringparam *, struct kernel_ethtool_ringparam *, struct netlink_ext_ack *);
    int (*set_ringparam)(struct net_device *, struct ethtool_ringparam *, struct kernel_ethtool_ringparam *, struct netlink_ext_ack *);
    void (*get_pauseparam)(struct net_device *, struct ethtool_pauseparam *);
    int (*set_pauseparam)(struct net_device *, struct ethtool_pauseparam *);
    void (*self_test)(struct net_device *, struct ethtool_test *, u64 *);
    void (*get_strings)(struct net_device *, u32, u8 *);
    int (*set_phys_id)(struct net_device *, enum ethtool_phys_id_state);
    void (*get_ethtool_stats)(struct net_device *, struct ethtool_stats *, u64 *);
    int (*get_sset_count)(struct net_device *, int);
    int (*get_coalesce)(struct net_device *, struct ethtool_coalesce *, struct kernel_ethtool_coalesce *, struct netlink_ext_ack *);
    int (*set_coalesce)(struct net_device *, struct ethtool_coalesce *, struct kernel_ethtool_coalesce *, struct netlink_ext_ack *);
    int (*get_ts_info)(struct net_device *, struct kernel_ethtool_ts_info *);
    int (*get_link_ksettings)(struct net_device *, struct ethtool_link_ksettings *);
    int (*set_link_ksettings)(struct net_device *, const struct ethtool_link_ksettings *);
};
void ethtool_convert_legacy_u32_to_link_mode(unsigned long *, u32);
bool ethtool_convert_link_mode_to_legacy_u32(u32 *, const unsigned long *);
int ethtool_op_get_ts_info(struct net_device *, struct kernel_ethtool_ts_info *);
void ethtool_sprintf(u8 **, const char *, ...);
#endif
