/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_DRIVER_CHECK_PCI_H
#define KUTOX_DRIVER_CHECK_PCI_H
#include "../device_types.h"
struct dev_pm_ops { int (*suspend)(struct device *); int (*resume)(struct device *); };
#define DEFINE_SIMPLE_DEV_PM_OPS(n,s,r) const struct dev_pm_ops n __attribute__((unused)) = {s,r}
#define pm_sleep_ptr(p) ((const struct dev_pm_ops *)NULL)
#define PCI_ANY_ID (~0U)
#define PCI_VENDOR_ID_INTEL 0x8086
#define PCI_VENDOR_ID_VMWARE 0x15ad
#define PCI_DEVICE(v,d) .vendor=v,.device=d,.subvendor=PCI_ANY_ID,.subdevice=PCI_ANY_ID
#define PCI_STD_NUM_BARS 6
#define IORESOURCE_IO 0x100
#define IORESOURCE_MEM 0x200
#define PCI_D0 0
#define PCI_D3hot 3
#define PCI_D3cold 4
typedef int pci_power_t;
int pci_register_driver(struct pci_driver *);
void pci_unregister_driver(struct pci_driver *);
int pci_enable_device(struct pci_dev *);
int pci_enable_device_mem(struct pci_dev *);
void pci_disable_device(struct pci_dev *);
int pci_select_bars(struct pci_dev *, unsigned long);
int pci_request_selected_regions(struct pci_dev *, int, const char *);
void pci_release_selected_regions(struct pci_dev *, int);
void pci_set_master(struct pci_dev *);
void pci_clear_master(struct pci_dev *);
int pci_save_state(struct pci_dev *);
void pci_restore_state(struct pci_dev *);
void pci_set_drvdata(struct pci_dev *, void *);
void *pci_get_drvdata(const struct pci_dev *);
void *pci_ioremap_bar(struct pci_dev *, int);
unsigned long pci_resource_len(struct pci_dev *, int);
unsigned long pci_resource_flags(struct pci_dev *, int);
resource_size_t pci_resource_start(struct pci_dev *, int);
int pci_read_config_word(struct pci_dev *, int, u16 *);
int pci_write_config_word(struct pci_dev *, int, u16);
int pci_read_config_dword(struct pci_dev *, int, u32 *);
int pci_write_config_dword(struct pci_dev *, int, u32);
int pci_set_mwi(struct pci_dev *);
void pci_clear_mwi(struct pci_dev *);
int pcix_get_mmrbc(struct pci_dev *);
int pcix_set_mmrbc(struct pci_dev *, int);
const char *pci_name(const struct pci_dev *);
bool pci_dev_run_wake(struct pci_dev *);
void pci_enable_wake(struct pci_dev *, pci_power_t, bool);
void pci_wake_from_d3(struct pci_dev *, bool);
int pci_prepare_to_sleep(struct pci_dev *);
int pci_set_power_state(struct pci_dev *, pci_power_t);
void pci_disable_link_state(struct pci_dev *, int);
bool pci_channel_offline(struct pci_dev *);
void device_set_wakeup_enable(struct device *, bool);
bool device_can_wakeup(struct device *);
struct pci_dev *to_pci_dev(struct device *);
enum dma_data_direction { DMA_BIDIRECTIONAL, DMA_TO_DEVICE, DMA_FROM_DEVICE, DMA_NONE };
int dma_set_mask_and_coherent(struct device *, u64);
void *dma_alloc_coherent(struct device *, size_t, dma_addr_t *, gfp_t);
void dma_free_coherent(struct device *, size_t, void *, dma_addr_t);
dma_addr_t dma_map_single(struct device *, void *, size_t, enum dma_data_direction);
bool dma_mapping_error(struct device *, dma_addr_t);
void dma_unmap_single(struct device *, dma_addr_t, size_t, enum dma_data_direction);
void dma_sync_single_for_cpu(struct device *, dma_addr_t, size_t, enum dma_data_direction);
void dma_sync_single_for_device(struct device *, dma_addr_t, size_t, enum dma_data_direction);
struct page;
dma_addr_t dma_map_page(struct device *, struct page *, size_t, size_t, enum dma_data_direction);
void dma_unmap_page(struct device *, dma_addr_t, size_t, enum dma_data_direction);
#endif
