use crate::protocol::*;
use anyhow::Result;
use std::path::Path;

/// Enhanced sysfs file generator
pub struct SysfsGenerator;
impl SysfsGenerator {
    /// Create complete sysfs structure for a device
    pub fn create_device_files(
        id: DeviceId,
        config: &DeviceConfig,
        base_path: &Path,
    ) -> Result<()> {
        let event_node = format!("event{}", id);
        let input_node = format!("input{}", id);
        Self::create_devices_virtual(&input_node, &event_node, config, base_path)?;
        Self::create_class_input_symlink(&event_node, &input_node, base_path)?;
        Self::create_udev_data_file(id, config, base_path)?;
        Ok(())
    }

    fn create_class_input_symlink(
        event_node: &str,
        input_node: &str,
        base_path: &Path,
    ) -> Result<()> {
        let class_input_dir = base_path.join("sysfs/class/input");
        std::fs::create_dir_all(&class_input_dir)?;

        let symlink_path = class_input_dir.join(event_node);
        let target = format!("../../devices/virtual/input/{}/{}", input_node, event_node);

        // Remove if exists
        let _ = std::fs::remove_file(&symlink_path);
        let _ = std::fs::remove_dir_all(&symlink_path);

        // Create symlink
        std::os::unix::fs::symlink(&target, &symlink_path)?;

        tracing::debug!("Created symlink: {} -> {}", symlink_path.display(), target);

        Ok(())
    }

    /// Create /sys/devices/virtual/input/inputX structure
    fn create_devices_virtual(
        input_node: &str,
        event_node: &str,
        config: &DeviceConfig,
        base_path: &Path,
    ) -> Result<()> {
        let input_base = base_path
            .join("sysfs/devices/virtual/input")
            .join(input_node);

        let event_path = input_base.join(event_node);

        // Create ALL directory structure
        std::fs::create_dir_all(&input_base)?;
        std::fs::create_dir_all(&event_path)?;
        std::fs::create_dir_all(input_base.join("id"))?;
        std::fs::create_dir_all(input_base.join("capabilities"))?;

        // Add unique name identifier
        let unique_name = format!("{} ({})", config.name, event_node);

        // Write input device properties
        std::fs::write(input_base.join("name"), format!("{}\n", unique_name))?;
        std::fs::write(
            input_base.join("phys"),
            format!("vimputti-{}\n", event_node),
        )?;
        std::fs::write(input_base.join("uniq"), format!("{}\n", event_node))?;

        // Write IDs
        std::fs::write(
            input_base.join("id/bustype"),
            format!("{:04x}\n", config.bustype as u16),
        )?;
        std::fs::write(
            input_base.join("id/vendor"),
            format!("{:04x}\n", config.vendor_id),
        )?;
        std::fs::write(
            input_base.join("id/product"),
            format!("{:04x}\n", config.product_id),
        )?;
        std::fs::write(
            input_base.join("id/version"),
            format!("{:04x}\n", config.version),
        )?;

        // Write capabilities
        Self::write_capabilities(&input_base, config)?;

        // Write modalias
        std::fs::write(
            input_base.join("modalias"),
            format!(
                "input:b{:04X}v{:04X}p{:04X}e{:04X}\n",
                config.bustype as u16, config.vendor_id, config.product_id, config.version
            ),
        )?;

        // Write uevent
        let uevent_content = format!(
            "PRODUCT={:x}/{:x}/{:x}/{:x}\n\
             NAME=\"{}\"\n\
             PHYS=\"vimputti-{}\"\n\
             UNIQ=\"{}\"\n\
             EV={}\n\
             KEY={}\n\
             ABS={}\n",
            config.bustype as u16,
            config.vendor_id,
            config.product_id,
            config.version,
            unique_name,
            event_node,
            event_node,
            Self::calculate_ev_bits(config),
            Self::calculate_key_bits(config),
            Self::calculate_abs_bits(config),
        );
        std::fs::write(input_base.join("uevent"), uevent_content)?;

        // Event node properties
        std::fs::write(
            event_path.join("dev"),
            format!("13:{}\n", event_node.trim_start_matches("event")),
        )?;

        // Create subsystem symlink
        let subsystem_link = event_path.join("subsystem");
        let _ = std::fs::remove_file(&subsystem_link);
        std::os::unix::fs::symlink("../../../../class/input", &subsystem_link)?;

        // Create device symlink: eventX/device -> ..
        let device_link = event_path.join("device");
        let _ = std::fs::remove_file(&device_link);
        let _ = std::fs::remove_dir_all(&device_link); // Remove if it's a directory
        std::os::unix::fs::symlink("..", &device_link)?;

        // Write event uevent
        let event_uevent = format!(
            "MAJOR=13\n\
             MINOR={}\n\
             DEVNAME=input/{}\n",
            event_node.trim_start_matches("event"),
            event_node
        );
        std::fs::write(event_path.join("uevent"), event_uevent)?;

        Ok(())
    }
    pub fn create_udev_data_file(
        id: DeviceId,
        config: &DeviceConfig,
        base_path: &Path,
    ) -> Result<()> {
        let minor = 64 + id; // event0 = minor 64, event1 = 65, etc.
        let data_file = format!("c13:{}", minor); // char device major 13

        let udev_data_dir = base_path.join("udev_data");
        std::fs::create_dir_all(&udev_data_dir)?;

        // Format: E:KEY=VALUE lines
        let mut content = String::new();
        content.push_str("E:ID_INPUT=1\n");
        content.push_str("E:ID_INPUT_JOYSTICK=1\n");
        content.push_str(&format!("E:ID_VENDOR_ID={:04x}\n", config.vendor_id));
        content.push_str(&format!("E:ID_MODEL_ID={:04x}\n", config.product_id));

        let bus_name = match config.bustype {
            BusType::Usb => "usb",
            BusType::Bluetooth => "bluetooth",
            BusType::Virtual => "virtual",
        };
        content.push_str(&format!("E:ID_BUS={}\n", bus_name));

        // Vendor info
        let vendor_name = match config.vendor_id {
            0x045e => "Microsoft",
            0x054c => "Sony",
            0x057e => "Nintendo",
            _ => "Unknown",
        };
        content.push_str(&format!("E:ID_VENDOR_ENC={}\n", vendor_name));
        content.push_str(&format!("E:ID_VENDOR_FROM_DATABASE={}\n", vendor_name));

        // Model info
        content.push_str(&format!(
            "E:ID_MODEL_ENC={}\n",
            config.name.replace(' ', "\\x20")
        ));
        content.push_str(&format!("E:ID_MODEL_FROM_DATABASE={}\n", config.name));

        // Path info
        content.push_str(&format!("E:ID_PATH=platform-vimputti-event{}\n", id));
        content.push_str(&format!("E:ID_PATH_TAG=platform-vimputti-event{}\n", id));
        content.push_str(&format!("E:ID_SERIAL=vimputti_event{}\n", id));

        // Tags
        content.push_str("E:TAGS=:uaccess:\n");
        content.push_str("G:uaccess\n"); // ACL tag

        std::fs::write(udev_data_dir.join(&data_file), content)?;

        Ok(())
    }

    /// Write capability bitmasks
    fn write_capabilities(base_path: &Path, config: &DeviceConfig) -> Result<()> {
        let caps_dir = base_path.join("capabilities");

        // Ensure capabilities directory exists
        std::fs::create_dir_all(&caps_dir)?;

        // EV capabilities
        std::fs::write(
            caps_dir.join("ev"),
            format!("{}\n", Self::calculate_ev_bits(config)),
        )?;

        // Key capabilities (buttons)
        std::fs::write(
            caps_dir.join("key"),
            format!("{}\n", Self::calculate_key_bits(config)),
        )?;

        // Absolute axis capabilities
        std::fs::write(
            caps_dir.join("abs"),
            format!("{}\n", Self::calculate_abs_bits(config)),
        )?;

        // Relative axis capabilities (none for controllers)
        std::fs::write(caps_dir.join("rel"), "0\n")?;

        // MSC capabilities
        std::fs::write(caps_dir.join("msc"), "0\n")?;

        // LED capabilities
        std::fs::write(caps_dir.join("led"), "0\n")?;

        // Sound capabilities
        std::fs::write(caps_dir.join("snd"), "0\n")?;

        // Force feedback capabilities (none for now)
        std::fs::write(caps_dir.join("ff"), "0\n")?;

        // Switch capabilities
        std::fs::write(caps_dir.join("sw"), "0\n")?;

        Ok(())
    }

    /// Calculate EV bitmask (supported event types)
    fn calculate_ev_bits(config: &DeviceConfig) -> String {
        let mut bits = 1u64; // EV_SYN is always supported

        if !config.buttons.is_empty() {
            bits |= 1 << EV_KEY; // Button events
        }

        if !config.axes.is_empty() {
            bits |= 1 << EV_ABS; // Absolute axis events
        }

        format!("{:x}", bits)
    }

    /// Calculate KEY bitmask (supported buttons)
    fn calculate_key_bits(config: &DeviceConfig) -> String {
        if config.buttons.is_empty() {
            return "0".to_string();
        }

        let mut bits = [0u64; 12]; // 768 bits / 64 = 12 u64s

        for button in &config.buttons {
            let code = button.to_ev_code() as usize;
            let index = code / 64;
            let bit = code % 64;
            if index < bits.len() {
                bits[index] |= 1u64 << bit;
            }
        }

        // Format as hex string (filter out leading zeros)
        let formatted: Vec<String> = bits
            .iter()
            .rev()
            .skip_while(|&&b| b == 0)
            .map(|b| format!("{:x}", b))
            .collect();

        if formatted.is_empty() {
            "0".to_string()
        } else {
            formatted.join(" ")
        }
    }

    /// Calculate ABS bitmask (supported axes)
    fn calculate_abs_bits(config: &DeviceConfig) -> String {
        if config.axes.is_empty() {
            return "0".to_string();
        }

        let mut bits = [0u64; 1]; // 64 bits for now (covers standard axes)

        for axis_config in &config.axes {
            let code = axis_config.axis.to_ev_code() as usize;
            if code < 64 {
                bits[0] |= 1u64 << code;
            }
        }

        format!("{:x}", bits[0])
    }

    /// Remove sysfs files for a device
    pub fn remove_device_files(id: DeviceId, base_path: &Path) -> Result<()> {
        let event_node = format!("event{}", id);
        let input_node = format!("input{}", id);
        let minor = 64 + id;

        // Remove class/input/eventX
        let _ = std::fs::remove_dir_all(base_path.join("sysfs/class/input").join(&event_node));

        // Remove devices/virtual/input/inputX
        let _ = std::fs::remove_dir_all(
            base_path
                .join("sysfs/devices/virtual/input")
                .join(&input_node),
        );

        // Remove udev data files
        let _ = std::fs::remove_file(base_path.join("udev_data").join(format!("c13:{}", minor)));

        Ok(())
    }
}
