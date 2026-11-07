/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_DRIVER_CHECK_EXECUTION_H
#define KUTOX_DRIVER_CHECK_EXECUTION_H
#include "../execution_types.h"
struct list_head { struct list_head *next, *prev; };
struct timer_list { unsigned long state; };
struct module { unsigned long state; };
extern struct module kutox_module;
#define THIS_MODULE (&kutox_module)
#define kutox_check_string(x) #x
#define __MODULE_STRING(x) kutox_check_string(x)
struct kutox_module_entry { int (*init)(void); void (*exit)(void); };
#define module_init(f) int (*const kutox_module_init)(void) __attribute__((used,section(".kutox.module_init"))) = f
#define module_exit(f) void (*const kutox_module_exit)(void) __attribute__((used,section(".kutox.module_exit"))) = f
struct kutox_parameter { const char *name; const char *type; void *value; size_t count; unsigned int *used; unsigned int mode; };
#define module_param(n,t,m) const struct kutox_parameter kutox_param_##n __attribute__((used,section(".kutox.parameters"))) = { #n,#t,&n,1,NULL,m }
#define module_param_array_named(n,v,t,c,m) const struct kutox_parameter kutox_param_##n __attribute__((used,section(".kutox.parameters"))) = { #n,#t,v,ARRAY_SIZE(v),c,m }
#define MODULE_PARM_DESC(n,d) static const char kutox_desc_##n[] __attribute__((used,section(".kutox.parameter_descriptions"))) = d
#define MODULE_DESCRIPTION(d) static const char kutox_description[] __attribute__((used)) = d
#define MODULE_LICENSE(d) static const char kutox_license[] __attribute__((used)) = d
#define MODULE_DEVICE_TABLE(t,n) static const void *const kutox_table_##n __attribute__((used,section(".kutox.device_tables"))) = n
#define DEFINE_MUTEX(n) struct mutex n = {0}
#define DEFINE_SPINLOCK(n) spinlock_t n = {0}
#include "../execution_services.h"
void mutex_lock(struct mutex *);
void mutex_unlock(struct mutex *);
void mutex_init(struct mutex *);
void spin_lock_init(spinlock_t *);
void spin_lock(spinlock_t *);
void spin_unlock(spinlock_t *);
unsigned long kutox_check_spin_lock_irqsave(spinlock_t *);
void kutox_check_spin_unlock_irqrestore(spinlock_t *, unsigned long);
#define spin_lock_irqsave(l,f) ((f) = kutox_check_spin_lock_irqsave(l))
#define spin_unlock_irqrestore(l,f) kutox_check_spin_unlock_irqrestore(l,f)
void local_bh_disable(void);
void local_bh_enable(void);
void smp_mb(void);
void smp_wmb(void);
void smp_rmb(void);
void smp_mb__before_atomic(void);
void smp_mb__after_atomic(void);
void dma_wmb(void);
void dma_rmb(void);
void barrier(void);
void cpu_relax(void);
int atomic_read(const atomic_t *);
void atomic_set(atomic_t *, int);
bool test_bit(unsigned int, const unsigned long *);
bool test_and_set_bit(unsigned int, unsigned long *);
void set_bit(unsigned int, unsigned long *);
void clear_bit(unsigned int, unsigned long *);
unsigned long msecs_to_jiffies(unsigned int);
bool time_after(unsigned long, unsigned long);
bool time_before(unsigned long, unsigned long);
bool time_after_eq(unsigned long, unsigned long);
void udelay(unsigned long);
void mdelay(unsigned long);
void msleep(unsigned int);
unsigned long msleep_interruptible(unsigned int);
void usleep_range(unsigned long, unsigned long);
int cond_resched(void);
void init_work(struct work_struct *, void (*)(struct work_struct *));
void init_delayed_work(struct delayed_work *, void (*)(struct work_struct *));
bool schedule_work(struct work_struct *);
bool schedule_delayed_work(struct delayed_work *, unsigned long);
bool cancel_work_sync(struct work_struct *);
bool cancel_delayed_work_sync(struct delayed_work *);
void rtnl_lock(void);
void rtnl_unlock(void);
bool rtnl_is_locked(void);
int request_irq(unsigned int, irq_handler_t, unsigned long, const char *, void *);
void free_irq(unsigned int, void *);
void synchronize_irq(unsigned int);
bool napi_schedule_prep(struct napi_struct *);
void __napi_schedule(struct napi_struct *);
bool napi_complete_done(struct napi_struct *, int);
void napi_enable(struct napi_struct *);
void napi_disable(struct napi_struct *);
void netif_napi_del(struct napi_struct *);
#endif
