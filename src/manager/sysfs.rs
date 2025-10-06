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

        // Create both /sys/class/input/eventX and /sys/devices/virtual/input/inputX
        Self::create_class_input(&event_node, config, base_path)?;
        Self::create_devices_virtual(&input_node, &event_node, config, base_path)?;

        Ok(())
    }

    /// Create /sys/class/input/eventX structure
    fn create_class_input(event_node: &str, config: &DeviceConfig, base_path: &Path) -> Result<()> {
        let sysfs_base = base_path.join("sysfs/class/input").join(event_node);

        // Create directory structure - make sure ALL directories exist
        std::fs::create_dir_all(sysfs_base.join("device/id"))?;
        std::fs::create_dir_all(sysfs_base.join("device/capabilities"))?;

        // Write device name
        std::fs::write(sysfs_base.join("device/name"), &config.name)?;

        // Write phys
        std::fs::write(
            sysfs_base.join("device/phys"),
            format!("vimputti-{}\n", event_node),
        )?;

        // Write uniq
        std::fs::write(sysfs_base.join("device/uniq"), "")?;

        // Write device IDs
        std::fs::write(
            sysfs_base.join("device/id/bustype"),
            format!("{:04x}\n", config.bustype as u16),
        )?;
        std::fs::write(
            sysfs_base.join("device/id/vendor"),
            format!("{:04x}\n", config.vendor_id),
        )?;
        std::fs::write(
            sysfs_base.join("device/id/product"),
            format!("{:04x}\n", config.product_id),
        )?;
        std::fs::write(
            sysfs_base.join("device/id/version"),
            format!("{:04x}\n", config.version),
        )?;

        // Write capabilities (sysfs_base/device/capabilities already exists)
        Self::write_capabilities(&sysfs_base.join("device"), config)?;

        // Write properties
        std::fs::write(sysfs_base.join("device/properties"), "0\n")?;

        // Write uevent file
        let uevent_content = format!(
            "PRODUCT={:x}/{:x}/{:x}/{:x}\n\
             NAME=\"{}\"\n\
             PHYS=\"vimputti-{}\"\n\
             UNIQ=\"\"\n\
             EV={}\n\
             KEY={}\n\
             ABS={}\n",
            config.bustype as u16,
            config.vendor_id,
            config.product_id,
            config.version,
            config.name,
            event_node,
            Self::calculate_ev_bits(config),
            Self::calculate_key_bits(config),
            Self::calculate_abs_bits(config),
        );
        std::fs::write(sysfs_base.join("device/uevent"), uevent_content)?;

        Ok(())
    }

    /// Create /sys/devices/virtual/input/inputX structure
    fn create_devices_virtual(
        input_node: &str,
        event_node: &str,
        config: &DeviceConfig,
        base_path: &Path,
    ) -> Result<()> {
        let device_base = base_path
            .join("sysfs/devices/virtual/input")
            .join(input_node);

        // Create directory structure - make sure ALL directories exist
        std::fs::create_dir_all(device_base.join("id"))?;
        std::fs::create_dir_all(device_base.join("capabilities"))?;
        std::fs::create_dir_all(device_base.join(event_node))?;

        // Write device properties
        std::fs::write(device_base.join("name"), &config.name)?;
        std::fs::write(
            device_base.join("phys"),
            format!("vimputti-{}\n", event_node),
        )?;
        std::fs::write(device_base.join("uniq"), "")?;

        // Write IDs
        std::fs::write(
            device_base.join("id/bustype"),
            format!("{:04x}\n", config.bustype as u16),
        )?;
        std::fs::write(
            device_base.join("id/vendor"),
            format!("{:04x}\n", config.vendor_id),
        )?;
        std::fs::write(
            device_base.join("id/product"),
            format!("{:04x}\n", config.product_id),
        )?;
        std::fs::write(
            device_base.join("id/version"),
            format!("{:04x}\n", config.version),
        )?;

        // Write capabilities (device_base/capabilities already exists)
        Self::write_capabilities(&device_base, config)?;

        // Write modalias
        std::fs::write(
            device_base.join("modalias"),
            format!(
                "input:b{:04X}v{:04X}p{:04X}e{:04X}\n",
                config.bustype as u16, config.vendor_id, config.product_id, config.version
            ),
        )?;

        // Create event node subdirectory with minimal info
        std::fs::write(
            device_base.join(event_node).join("dev"),
            format!("13:{}\n", event_node.trim_start_matches("event")),
        )?;

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

        // Remove class/input/eventX
        let _ = std::fs::remove_dir_all(base_path.join("sysfs/class/input").join(&event_node));

        // Remove devices/virtual/input/inputX
        let _ = std::fs::remove_dir_all(
            base_path
                .join("sysfs/devices/virtual/input")
                .join(&input_node),
        );

        Ok(())
    }
}
