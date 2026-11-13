/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_EXECUTION_SERVICES_H
#define KUTOX_EXECUTION_SERVICES_H
#define KUTOX_NAPI_BUDGET 64
unsigned long kutox_jiffies(void);
#define jiffies kutox_jiffies()
void spin_lock(spinlock_t *);
void spin_unlock(spinlock_t *);
unsigned long kutox_check_spin_lock_irqsave(spinlock_t *);
void kutox_check_spin_unlock_irqrestore(spinlock_t *, unsigned long);
void local_bh_disable(void);
void local_bh_enable(void);
void smp_mb(void);
void smp_wmb(void);
void smp_rmb(void);
void smp_mb__before_atomic(void);
void smp_mb__after_atomic(void);
void barrier(void);
void cpu_relax(void);
int atomic_read(const atomic_t *);
void atomic_set(atomic_t *, int);
bool test_bit(unsigned int, const unsigned long *);
bool test_and_set_bit(unsigned int, unsigned long *);
void set_bit(unsigned int, unsigned long *);
void clear_bit(unsigned int, unsigned long *);
unsigned long find_first_bit(const unsigned long *, unsigned long);
unsigned long find_next_bit(const unsigned long *, unsigned long, unsigned long);
void kutox_check_store_release(void *, unsigned long, size_t);
#define smp_store_release(p,v) kutox_check_store_release(p,v,sizeof(*(p)))
u64 kutox_check_load_acquire(const void *, size_t);
u64 kutox_check_read_once(const void *, size_t);
#define smp_load_acquire(p) ((typeof(*(p)))kutox_check_load_acquire(p,sizeof(*(p))))
#define READ_ONCE(v) ((typeof(v))kutox_check_read_once(&(v),sizeof(v)))
void kutox_check_write_once(void *, u64, size_t);
#define WRITE_ONCE(p,v) kutox_check_write_once(&(p),v,sizeof(p))
bool time_after(unsigned long, unsigned long);
bool time_before(unsigned long, unsigned long);
bool time_after_eq(unsigned long, unsigned long);
void mdelay(unsigned long);
unsigned long msleep_interruptible(unsigned int);
void usleep_range(unsigned long, unsigned long);
void init_work(struct work_struct *, void (*)(struct work_struct *));
void init_delayed_work(struct delayed_work *, void (*)(struct work_struct *));
#define INIT_WORK(w,f) init_work(w,f)
#define INIT_DELAYED_WORK(w,f) init_delayed_work(w,f)
bool schedule_work(struct work_struct *);
bool schedule_delayed_work(struct delayed_work *, unsigned long);
bool cancel_work_sync(struct work_struct *);
bool cancel_delayed_work_sync(struct delayed_work *);
void kutox_destroy_work(struct work_struct *);
void rtnl_lock(void);
void rtnl_unlock(void);
bool rtnl_is_locked(void);
typedef enum { IRQ_NONE, IRQ_HANDLED } irqreturn_t;
typedef irqreturn_t (*irq_handler_t)(int, void *);
#define IRQF_SHARED 0x80
#define IRQF_PROBE_SHARED 0x100
int request_irq(unsigned int, irq_handler_t, unsigned long, const char *, void *);
void free_irq(unsigned int, void *);
void synchronize_irq(unsigned int);
void netif_napi_add(struct net_device *, struct napi_struct *, int (*)(struct napi_struct *, int));
void netif_napi_set_irq(struct napi_struct *, unsigned int);
void netif_queue_set_napi(struct net_device *, unsigned int, unsigned int, struct napi_struct *);
bool napi_schedule_prep(struct napi_struct *);
void __napi_schedule(struct napi_struct *);
bool napi_complete_done(struct napi_struct *, int);
void napi_enable(struct napi_struct *);
void napi_disable(struct napi_struct *);
void netif_napi_del(struct napi_struct *);
#endif
