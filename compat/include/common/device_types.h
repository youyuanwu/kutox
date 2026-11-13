/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_DEVICE_TYPES_H
#define KUTOX_DEVICE_TYPES_H
struct device_driver;
struct bus_type;
struct attribute_group;
struct device_node;
struct cpumask;
struct module;
struct kobj_uevent_env;
struct dev_pm_ops;
struct device {
    struct device *parent;
    struct device_driver *driver;
    const struct bus_type *bus;
    const struct attribute_group **groups;
    void (*release)(struct device *);
    void *driver_data;
    struct device_node *of_node;
};
struct device_driver { const char *name; const struct bus_type *bus; struct module *owner; };
struct bus_type {
    const char *name;
    const struct attribute_group **dev_groups;
    int (*match)(struct device *, const struct device_driver *);
    int (*uevent)(const struct device *, struct kobj_uevent_env *);
    int (*probe)(struct device *);
    void (*remove)(struct device *);
    const struct cpumask *(*irq_get_affinity)(struct device *, unsigned int);
    void (*shutdown)(struct device *);
    int (*num_vf)(struct device *);
};
struct pci_dev {
    struct device dev;
    u16 vendor, device, subsystem_vendor, subsystem_device;
    u8 revision;
    unsigned int irq;
    bool is_virtfn;
    struct pci_dev *physfn;
};
struct pci_device_id { u32 vendor, device, subvendor, subdevice, class, class_mask; unsigned long driver_data; };
typedef enum { pci_channel_io_normal = 1, pci_channel_io_frozen, pci_channel_io_perm_failure } pci_channel_state_t;
typedef enum { PCI_ERS_RESULT_NONE = 1, PCI_ERS_RESULT_CAN_RECOVER, PCI_ERS_RESULT_NEED_RESET, PCI_ERS_RESULT_DISCONNECT, PCI_ERS_RESULT_RECOVERED } pci_ers_result_t;
struct pci_error_handlers {
    pci_ers_result_t (*error_detected)(struct pci_dev *, pci_channel_state_t);
    pci_ers_result_t (*slot_reset)(struct pci_dev *);
    void (*resume)(struct pci_dev *);
    void (*reset_prepare)(struct pci_dev *);
    void (*reset_done)(struct pci_dev *);
};
struct pci_driver {
    const char *name;
    const struct pci_device_id *id_table;
    int (*probe)(struct pci_dev *, const struct pci_device_id *);
    void (*remove)(struct pci_dev *);
    struct { const struct dev_pm_ops *pm; } driver;
    void (*shutdown)(struct pci_dev *);
    const struct pci_error_handlers *err_handler;
    int (*sriov_configure)(struct pci_dev *, int);
};
#endif
