/* SPDX-License-Identifier: GPL-2.0-only */
#include "driver_services.h"
static struct pci_dev endpoint;
static struct pci_driver *driver;
static void *driver_data;
static char location[16];
static bool wakeup;
static bool bound;
static bool probed;
extern void kutox_driver_trace(unsigned int,uintptr_t,uintptr_t,int);
void kutox_c_pci_clear_data(void *data) { if (driver_data == data) driver_data = NULL; }

uintptr_t kutox_c_driver_device(void) { return (uintptr_t)&endpoint.dev; }
struct pci_dev *kutox_c_driver_endpoint(unsigned int irq)
{
    if (bound) kutox_platform_fail("pci_endpoint_live");
    memset(&endpoint, 0, sizeof(endpoint));
    int vendor = kutox_driver_pci(10, 0, 0);
    int device = kutox_driver_pci(10, 2, 0);
    driver_check(vendor, "pci_vendor"); driver_check(device, "pci_device");
    if (vendor != 0x8086 || device != 0x100e) kutox_platform_fail("pci_endpoint_profile");
    endpoint.vendor = (u16)vendor; endpoint.device = (u16)device; endpoint.irq = irq;
    int revision = kutox_driver_pci(10, 8, 0);
    driver_check(revision, "pci_revision"); endpoint.revision = (u8)revision;
    int subsystem_vendor = kutox_driver_pci(10, 0x2c, 0);
    int subsystem_device = kutox_driver_pci(10, 0x2e, 0);
    driver_check(subsystem_vendor, "pci_subvendor"); driver_check(subsystem_device, "pci_subdevice");
    endpoint.subsystem_vendor = (u16)subsystem_vendor; endpoint.subsystem_device = (u16)subsystem_device;
    u64 address = kutox_driver_pci_value(3);
    if (address >> 24 || ((address >> 8) & 255) >= 32 || (address & 255) >= 8)
        kutox_platform_fail("pci_location_profile");
    strcpy(location, "0000:00:00.0");
    location[5] = "0123456789abcdef"[(address >> 20) & 15];
    location[6] = "0123456789abcdef"[(address >> 16) & 15];
    location[8] = "0123456789abcdef"[(address >> 12) & 15];
    location[9] = "0123456789abcdef"[(address >> 8) & 15];
    location[11] = "0123456789abcdef"[address & 7];
    bound = true;
    return &endpoint;
}
static void check_pci(const struct pci_dev *device)
{
    if (!bound || device != &endpoint || (uintptr_t)&endpoint.dev != kutox_driver_device())
        kutox_platform_fail("pci_device_identity");
}
int pci_register_driver(struct pci_driver *candidate)
{
    if (!candidate || !candidate->id_table || !candidate->probe || !candidate->remove) return -EINVAL;
    if (driver) return -EBUSY;
    if (!bound) return -ENODEV;
    driver = candidate;
    for (size_t index = 0; index < 64; index++) {
        const struct pci_device_id *id = &candidate->id_table[index];
        if (!id->vendor && !id->device) return 0;
        if (id->vendor != endpoint.vendor || id->device != endpoint.device ||
            (id->subvendor != PCI_ANY_ID && id->subvendor != endpoint.subsystem_vendor) ||
            (id->subdevice != PCI_ANY_ID && id->subdevice != endpoint.subsystem_device) ||
            ((0x020000 ^ id->class) & id->class_mask)) continue;
        kutox_driver_trace(3,(uintptr_t)candidate->probe,(uintptr_t)&endpoint,0);
        int result = candidate->probe(&endpoint, id);
        kutox_driver_trace(4,(uintptr_t)candidate->probe,(uintptr_t)&endpoint,result);
        if (result < 0) { driver = NULL; kutox_driver_diag("pci_probe", result); }
        else probed = true;
        return result;
    }
    kutox_platform_fail("pci_id_table_bound");
}
void pci_unregister_driver(struct pci_driver *candidate)
{
    if (driver != candidate) kutox_platform_fail("pci_unregister_identity");
    if (probed) {
        kutox_driver_trace(5,(uintptr_t)candidate->remove,(uintptr_t)&endpoint,0);
        candidate->remove(&endpoint);
    }
    probed = false;
    driver = NULL;
}
void kutox_c_driver_endpoint_release(void)
{
    if (driver || driver_data) kutox_platform_fail("pci_endpoint_resources");
    bound = false;
}
int pci_enable_device(struct pci_dev *device) { check_pci(device); return kutox_driver_pci(0,0,0); }
int pci_enable_device_mem(struct pci_dev *device) { return pci_enable_device(device); }
void pci_disable_device(struct pci_dev *device) { check_pci(device); driver_check(kutox_driver_pci(1,0,0), "pci_disable"); }
int pci_select_bars(struct pci_dev *device, unsigned long flags) { check_pci(device); return (flags & IORESOURCE_MEM ? 1 : 0) | (flags & IORESOURCE_IO ? 2 : 0); }
int pci_request_selected_regions(struct pci_dev *device, int bars, const char *name)
{
    check_pci(device); if (!name) return -EINVAL;
    return kutox_driver_pci(2, (u64)bars, 0);
}
void pci_release_selected_regions(struct pci_dev *device, int bars) { check_pci(device); driver_check(kutox_driver_pci(3,(u64)bars,0), "pci_release"); }
void pci_set_master(struct pci_dev *device) { check_pci(device); driver_check(kutox_driver_pci(4,0,0), "pci_set_master"); }
void pci_clear_master(struct pci_dev *device) { check_pci(device); driver_check(kutox_driver_pci(9,0,0), "pci_clear_master"); }
int pci_save_state(struct pci_dev *device) { check_pci(device); return kutox_driver_pci(5,0,0); }
void pci_set_drvdata(struct pci_dev *device, void *data) { check_pci(device); driver_data = data; }
void *pci_get_drvdata(const struct pci_dev *device) { check_pci(device); return driver_data; }
const char *pci_name(const struct pci_dev *device) { check_pci(device); return location; }
unsigned long pci_resource_len(struct pci_dev *device, int bar) { check_pci(device); return bar == 0 ? kutox_driver_pci_value(1) : bar == 1 ? 64 : 0; }
unsigned long pci_resource_flags(struct pci_dev *device, int bar) { check_pci(device); return bar == 0 ? IORESOURCE_MEM : bar == 1 ? IORESOURCE_IO : 0; }
resource_size_t pci_resource_start(struct pci_dev *device, int bar) { check_pci(device); return bar == 0 ? kutox_driver_pci_value(0) : bar == 1 ? kutox_driver_pci_value(4) : 0; }
void *pci_ioremap_bar(struct pci_dev *device, int bar)
{
    check_pci(device);
    if (bar != 0) { kutox_driver_diag("pci_bar_profile", -EOPNOTSUPP); return NULL; }
    int result = kutox_driver_pci(6,0,0);
    if (result < 0) return NULL;
    return (void *)(uintptr_t)kutox_driver_pci_value(2);
}
void *ioremap(resource_size_t address, unsigned long size)
{
    if (address != kutox_driver_pci_value(0) || size != kutox_driver_pci_value(1)) {
        kutox_driver_diag("ioremap_profile", -EOPNOTSUPP); return NULL;
    }
    return pci_ioremap_bar(&endpoint, 0);
}
void iounmap(volatile void *address)
{
    if (!address) return;
    if ((uintptr_t)address != kutox_driver_pci_value(2)) kutox_platform_fail("iounmap_identity");
    driver_check(kutox_driver_pci(7,0,0), "iounmap");
}
int pci_read_config_word(struct pci_dev *device, int offset, u16 *value)
{
    check_pci(device); if (!value || offset < 0 || offset > 254 || offset & 1) return -EINVAL;
    int result = kutox_driver_pci(10, (u64)offset, 0);
    if (result < 0) return result;
    *value = (u16)result; return 0;
}
int pci_write_config_word(struct pci_dev *device, int offset, u16 value) { check_pci(device); return kutox_driver_pci(11,(u64)offset,value); }
int pci_set_mwi(struct pci_dev *device)
{
    check_pci(device); u16 command; int result = pci_read_config_word(device,4,&command);
    if (result < 0) return result;
    return pci_write_config_word(device,4,command | PCI_COMMAND_INVALIDATE);
}
void pci_clear_mwi(struct pci_dev *device)
{
    check_pci(device); u16 command;
    driver_check(pci_read_config_word(device,4,&command), "pci_read_mwi");
    driver_check(pci_write_config_word(device,4,command & ~PCI_COMMAND_INVALIDATE), "pci_clear_mwi");
}
bool pci_channel_offline(struct pci_dev *device)
{
    check_pci(device);
    int vendor = kutox_driver_pci(10,0,0);
    driver_check(vendor, "pci_channel_status");
    return vendor == 0xffff;
}
void device_set_wakeup_enable(struct device *device, bool enable)
{
    if (device != &endpoint.dev) kutox_platform_fail("device_wakeup_identity");
    wakeup = enable;
}
bool device_can_wakeup(struct device *device)
{
    if (device != &endpoint.dev) kutox_platform_fail("device_wakeup_identity");
    return wakeup;
}
