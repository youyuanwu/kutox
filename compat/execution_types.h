/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_EXECUTION_TYPES_H
#define KUTOX_EXECUTION_TYPES_H
/* Live objects are address-bound, non-copyable and must outlive joined users. */
struct list_head { struct list_head *next, *prev; };
struct mutex { u8 boot_held; u64 generation; };
struct completion { u8 reserved; u64 generation; };
typedef struct { u8 held; u64 owner; } spinlock_t;
typedef struct { int counter; } atomic_t;
struct work_struct { void (*function)(struct work_struct *); u64 generation; };
struct delayed_work { struct work_struct work; };
struct napi_struct {
    int (*poll)(struct napi_struct *, int);
    u64 generation;
    struct net_device *device;
    unsigned int irq;
};
#endif
