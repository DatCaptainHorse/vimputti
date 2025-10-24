use crate::manager::udev::{UdevAction, UdevDeviceInfo, UdevEvent};
use crate::{BusType, DeviceConfig, DeviceId};
use anyhow::Result;
use tracing::info;

pub struct NetlinkBroadcaster {
    socket: i32,
}
impl NetlinkBroadcaster {
    pub fn new() -> Result<Self> {
        const AF_NETLINK: i32 = 16;
        const NETLINK_KOBJECT_UEVENT: i32 = 15;
        const SOCK_RAW: i32 = 3;

        let sock = unsafe { libc::socket(AF_NETLINK, SOCK_RAW, NETLINK_KOBJECT_UEVENT) };
        if sock < 0 {
            return Err(anyhow::anyhow!("Failed to create netlink socket"));
        }

        info!("netlink broadcaster created");
        Ok(Self { socket: sock })
    }

    /// Send a udev event via real netlink
    pub fn send_event(&self, event: &UdevEvent) -> Result<()> {
        let action = match event.action {
            UdevAction::Add => "add",
            UdevAction::Remove => "remove",
            UdevAction::Change => "change",
        };

        // First line is special: action@devpath WITHOUT null terminator
        let mut message = Vec::new();
        message.extend_from_slice(format!("{}@{}", action, event.device_info.devpath).as_bytes());

        // Then null-terminated key=value pairs
        message.extend_from_slice(format!("\0ACTION={}\0", action).as_bytes());
        message.extend_from_slice(format!("DEVPATH={}\0", event.device_info.devpath).as_bytes());
        message
            .extend_from_slice(format!("SUBSYSTEM={}\0", event.device_info.subsystem).as_bytes());

        // Only add DEVNAME if it's not empty
        if !event.device_info.devname.is_empty() {
            message
                .extend_from_slice(format!("DEVNAME={}\0", event.device_info.devname).as_bytes());
        }

        // Add sequence number (udevadm expects this)
        let seq = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        message.extend_from_slice(format!("SEQNUM={}\0", seq).as_bytes());

        // Add properties
        for (key, value) in &event.device_info.properties {
            message.extend_from_slice(format!("{}={}\0", key, value).as_bytes());
        }

        let message_bytes = message.as_slice();

        // Send to GROUP_UDEV (2) if kernel events not allowed, otherwise to kernel
        let mut sa: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        sa.nl_family = 16; // AF_NETLINK
        sa.nl_groups = 2;
        sa.nl_pid = 0;

        let iov = libc::iovec {
            iov_base: message_bytes.as_ptr() as *mut _,
            iov_len: message_bytes.len(),
        };

        let msg = libc::msghdr {
            msg_name: &sa as *const _ as *mut _,
            msg_namelen: size_of::<libc::sockaddr_nl>() as u32,
            msg_iov: &iov as *const _ as *mut _,
            msg_iovlen: 1,
            msg_control: std::ptr::null_mut(),
            msg_controllen: 0,
            msg_flags: 0,
        };

        let rc = unsafe { libc::sendmsg(self.socket, &msg, 0) };
        tracing::debug!("sendmsg result: {}", rc);
        if rc < 0 {
            let err = std::io::Error::last_os_error();
            return Err(anyhow::anyhow!("Failed to send netlink message: {}", err));
        }

        Ok(())
    }

    /// Broadcast a device add event via netlink
    pub fn broadcast_add(&self, device_id: DeviceId, config: &DeviceConfig) -> Result<()> {
        let event_node = format!("event{}", device_id);
        let input_node = format!("input{}", device_id);

        let mut properties = vec![
            ("ID_INPUT".to_string(), "1".to_string()),
            ("ID_INPUT_JOYSTICK".to_string(), "1".to_string()),
            (
                "ID_MODEL".to_string(),
                format!("{}_{}", config.name.replace(' ', "_"), device_id),
            ),
            (
                "ID_VENDOR_ID".to_string(),
                format!("{:04x}", config.vendor_id),
            ),
            (
                "ID_MODEL_ID".to_string(),
                format!("{:04x}", config.product_id),
            ),
            (
                "ID_BUS".to_string(),
                match config.bustype {
                    BusType::Usb => "usb".to_string(),
                    BusType::Bluetooth => "bluetooth".to_string(),
                    BusType::Virtual => "virtual".to_string(),
                },
            ),
            ("NAME".to_string(), format!("\"{}\"", config.name)),
            (
                "PRODUCT".to_string(),
                format!(
                    "{:x}/{:x}/{:x}/{:x}",
                    config.bustype as u16, config.vendor_id, config.product_id, config.version
                ),
            ),
        ];

        if matches!(config.bustype, BusType::Usb) {
            properties.push(("BUSNUM".to_string(), "253".to_string()));
            properties.push(("DEVNUM".to_string(), format!("{:03}", device_id + 1)));
        }

        let event = UdevEvent {
            action: UdevAction::Add,
            device_info: UdevDeviceInfo {
                subsystem: "input".to_string(),
                devtype: "".to_string(),
                devname: format!("/dev/input/{}", event_node),
                devpath: format!("/devices/virtual/input/{}/{}", input_node, event_node),
                syspath: format!("/sys/devices/virtual/input/{}/{}", input_node, event_node),
                properties,
            },
        };

        self.send_event(&event)?;
        info!("Sent netlink add event for {}", event_node);
        Ok(())
    }

    /// Broadcast a device remove event via netlink
    pub fn broadcast_remove(&self, device_id: DeviceId, config: &DeviceConfig) -> Result<()> {
        let event_node = format!("event{}", device_id);
        let input_node = format!("input{}", device_id);

        let mut event = UdevEvent {
            action: UdevAction::Remove,
            device_info: UdevDeviceInfo {
                subsystem: "input".to_string(),
                devtype: "".to_string(),
                devname: format!("/dev/input/{}", event_node),
                devpath: format!("/devices/virtual/input/{}/{}", input_node, event_node),
                syspath: format!("/sys/devices/virtual/input/{}/{}", input_node, event_node),
                properties: vec![
                    ("NAME".to_string(), format!("\"{}\"", config.name)),
                    (
                        "ID_MODEL".to_string(),
                        format!("{}_{}", config.name.replace(' ', "_"), device_id),
                    ),
                    ("ID_SERIAL".to_string(), format!("vimputti_{}", event_node)),
                    ("ID_SERIAL_SHORT".to_string(), event_node.clone()),
                    ("UNIQ".to_string(), event_node.clone()),
                ],
            },
        };

        if matches!(config.bustype, BusType::Usb) {
            event
                .device_info
                .properties
                .push(("BUSNUM".to_string(), "253".to_string()));
            event
                .device_info
                .properties
                .push(("DEVNUM".to_string(), format!("{:03}", device_id + 1)));
        }

        self.send_event(&event)?;
        info!("Sent netlink remove event for {}", event_node);
        Ok(())
    }
}
