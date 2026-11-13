/* SPDX-License-Identifier: GPL-2.0-only */
#if defined(KUTOX_STORAGE_ABI_PROFILE)
#include "block/driver.h"
#elif defined(KUTOX_DRIVER_ABI_PROFILE)
#include "net/driver.h"
#else
#include "common/kutox_compat.h"
#endif
const unsigned long kutox_common_layout[] __attribute__((used,section(".kutox.common_layouts"))) = {
    sizeof(struct device), _Alignof(struct device), offsetof(struct device,parent),
    offsetof(struct device,driver), offsetof(struct device,bus), offsetof(struct device,groups),
    offsetof(struct device,release), offsetof(struct device,driver_data), offsetof(struct device,of_node),
    sizeof(struct pci_dev), offsetof(struct pci_dev,dev), offsetof(struct pci_dev,vendor),
    offsetof(struct pci_dev,device), offsetof(struct pci_dev,subsystem_vendor),
    offsetof(struct pci_dev,subsystem_device), offsetof(struct pci_dev,revision), offsetof(struct pci_dev,irq),
    offsetof(struct pci_dev,is_virtfn), offsetof(struct pci_dev,physfn),
    sizeof(struct pci_driver), offsetof(struct pci_driver,probe), offsetof(struct pci_driver,remove),
    offsetof(struct pci_driver,driver), offsetof(struct pci_driver,err_handler), offsetof(struct pci_driver,sriov_configure),
    sizeof(struct device_driver), sizeof(struct bus_type), offsetof(struct bus_type,probe), offsetof(struct bus_type,remove),
    sizeof(struct mutex), sizeof(struct completion), sizeof(spinlock_t), sizeof(struct work_struct),
    sizeof(struct list_head), GFP_KERNEL, GFP_ATOMIC, __GFP_ZERO,
};
