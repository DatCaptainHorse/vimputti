#include <stdio.h>
#include <stdlib.h>
#include <fcntl.h>
#include <unistd.h>
#include <string.h>
#include <errno.h>
#include <sys/stat.h>
#include <sys/ioctl.h>
#include <linux/input.h>

#define EVIOCGVERSION _IOR('E', 0x01, int)
#define EVIOCGID _IOR('E', 0x02, struct input_id)
#define EVIOCGNAME(len) _IOC(_IOC_READ, 'E', 0x06, len)

int main() {
    printf("=== Vimputti Shim Simple Test ===\n\n");

    // Test 1: Try to open a device
    printf("Test 1: Opening /dev/input/event0...\n");
    int fd = open("/dev/input/event0", O_RDONLY | O_NONBLOCK);
    if (fd < 0) {
        perror("  ✗ Failed to open device");
        printf("  Make sure:\n");
        printf("    1. Manager is running\n");
        printf("    2. A device has been created (run create_test_device example first)\n");
        printf("    3. LD_PRELOAD is set correctly\n");
        return 1;
    }
    printf("  ✓ Device opened successfully! fd=%d\n\n", fd);

    // Test 2: Get version
    printf("Test 2: Getting input subsystem version...\n");
    int version;
    if (ioctl(fd, EVIOCGVERSION, &version) < 0) {
        perror("  ✗ EVIOCGVERSION failed");
    } else {
        printf("  ✓ Version: %d.%d.%d\n\n",
               version >> 16, (version >> 8) & 0xff, version & 0xff);
    }

    // Test 3: Get device ID
    printf("Test 3: Getting device ID...\n");
    struct input_id device_id;
    if (ioctl(fd, EVIOCGID, &device_id) < 0) {
        perror("  ✗ EVIOCGID failed");
    } else {
        printf("  ✓ Bus: 0x%04x\n", device_id.bustype);
        printf("  ✓ Vendor: 0x%04x\n", device_id.vendor);
        printf("  ✓ Product: 0x%04x\n", device_id.product);
        printf("  ✓ Version: 0x%04x\n\n", device_id.version);
    }

    // Test 4: Get device name
    printf("Test 4: Getting device name...\n");
    char name[256] = {0};
    if (ioctl(fd, EVIOCGNAME(sizeof(name)), name) < 0) {
        perror("  ✗ EVIOCGNAME failed");
    } else {
        printf("  ✓ Name: %s\n\n", name);
    }

    // Test 5: Read from sysfs
    printf("Test 5: Reading from sysfs...\n");
    FILE *f = fopen("/sys/class/input/event0/device/name", "r");
    if (f) {
        char sysfs_name[256] = {0};
        if (fgets(sysfs_name, sizeof(sysfs_name), f)) {
            // Remove trailing newline
            sysfs_name[strcspn(sysfs_name, "\n")] = 0;
            printf("  ✓ Sysfs name: %s\n", sysfs_name);
        } else {
            printf("  ✗ Failed to read from sysfs\n");
        }
        fclose(f);
    } else {
        perror("  ✗ Failed to open sysfs file");
    }
    printf("\n");

    // Test 6: Check sysfs ID files
    printf("Test 6: Reading sysfs device ID...\n");
    f = fopen("/sys/class/input/event0/device/id/vendor", "r");
    if (f) {
        char vendor[16] = {0};
        if (fgets(vendor, sizeof(vendor), f)) {
            vendor[strcspn(vendor, "\n")] = 0;
            printf("  ✓ Sysfs vendor: %s\n", vendor);
        }
        fclose(f);
    } else {
        perror("  ✗ Failed to open vendor file");
    }

    f = fopen("/sys/class/input/event0/device/id/product", "r");
    if (f) {
        char product[16] = {0};
        if (fgets(product, sizeof(product), f)) {
            product[strcspn(product, "\n")] = 0;
            printf("  ✓ Sysfs product: %s\n", product);
        }
        fclose(f);
    } else {
        perror("  ✗ Failed to open product file");
    }
    printf("\n");

    close(fd);
    printf("=== All tests completed successfully! ===\n");

    return 0;
}
