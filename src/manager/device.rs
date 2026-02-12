use crate::protocol::*;
use crate::uinput::*;
use anyhow::{Context, Result, anyhow};
use std::ffi::{CStr, CString};
use std::fs::OpenOptions;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::PathBuf;
use tracing::{debug, info, warn};

pub struct VirtualDevice {
    pub id: DeviceId,
    pub config: DeviceConfig,
    pub event_node: String,
    pub joystick_node: Option<String>,
    uinput_fd: OwnedFd,
}

impl VirtualDevice {
    /// Create a new virtual device using real uinput
    pub fn create(id: DeviceId, config: DeviceConfig) -> Result<Self> {
        info!(
            "Creating virtual device {} with config: {:?}",
            id, config.name
        );

        // Step 1: Open real /dev/uinput
        let uinput_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/uinput")
            .context("Failed to open /dev/uinput - is uinput kernel module loaded?")?;

        let uinput_fd: OwnedFd = uinput_file.into();
        let raw_fd = uinput_fd.as_raw_fd();

        debug!("Opened /dev/uinput, fd={}", raw_fd);

        // Step 2: Configure uinput device via ioctl
        unsafe {
            // Set event types
            ioctl(raw_fd, UI_SET_EVBIT, EV_SYN as u64);
            ioctl(raw_fd, UI_SET_EVBIT, EV_KEY as u64);
            ioctl(raw_fd, UI_SET_EVBIT, EV_ABS as u64);
            ioctl(raw_fd, UI_SET_EVBIT, EV_FF as u64);

            debug!("Set event types");

            // Set buttons
            for button in &config.buttons {
                let code = button.to_ev_code();
                ioctl(raw_fd, UI_SET_KEYBIT, code as u64);
            }
            debug!("Set {} buttons", config.buttons.len());

            // Set axes with parameters
            for axis_config in &config.axes {
                let code = axis_config.axis.to_ev_code();
                ioctl(raw_fd, UI_SET_ABSBIT, code as u64);

                // Configure axis parameters
                let abs_setup = uinput_abs_setup {
                    code,
                    absinfo: input_absinfo {
                        value: 0,
                        minimum: axis_config.min,
                        maximum: axis_config.max,
                        fuzz: axis_config.fuzz,
                        flat: axis_config.flat,
                        resolution: 0,
                    },
                };

                if ioctl(raw_fd, UI_ABS_SETUP, &abs_setup as *const _ as u64) < 0 {
                    warn!("Failed to setup axis {}", code);
                }
            }
            debug!("Set {} axes", config.axes.len());

            // Set rumble capability
            ioctl(raw_fd, UI_SET_FFBIT, FF_RUMBLE as u64);
            debug!("Set rumble support");

            // Set device info
            let mut setup = uinput_setup {
                id: input_id {
                    bustype: config.bustype as u16,
                    vendor: config.vendor_id,
                    product: config.product_id,
                    version: config.version,
                },
                name: [0; 80],
                ff_effects_max: 1, // Support 1 rumble effect at a time
            };

            let name_bytes = config.name.as_bytes();
            let copy_len = name_bytes.len().min(79);
            setup.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

            if ioctl(raw_fd, UI_DEV_SETUP, &setup as *const _ as u64) < 0 {
                return Err(anyhow!(
                    "UI_DEV_SETUP failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            debug!("Device setup complete");

            // Step 3: Create the device
            if ioctl(raw_fd, UI_DEV_CREATE, 0) < 0 {
                return Err(anyhow!(
                    "UI_DEV_CREATE failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            info!("Created uinput device");
        }

        // Step 4: Get sysfs name from kernel
        let mut sysname_buf = [0u8; 80];
        unsafe {
            let ioctl_code = ui_get_sysname(sysname_buf.len());
            if ioctl(raw_fd, ioctl_code, sysname_buf.as_mut_ptr() as u64) < 0 {
                return Err(anyhow!(
                    "UI_GET_SYSNAME failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }

        let sysname = CStr::from_bytes_until_nul(&sysname_buf)
            .context("Invalid sysname from kernel")?
            .to_str()
            .context("Non-UTF8 sysname")?;

        info!("Kernel assigned sysfs name: {}", sysname);

        // Step 5: Discover event/js nodes from sysfs
        let sysfs_device = PathBuf::from("/sys/devices/virtual/input").join(sysname);

        // Give kernel a moment to create sysfs entries
        std::thread::sleep(std::time::Duration::from_millis(50));

        let mut event_node = None;
        let mut joystick_node = None;

        for entry in std::fs::read_dir(&sysfs_device).context(format!(
            "Failed to read sysfs dir: {}",
            sysfs_device.display()
        ))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();

            if name.starts_with("event") {
                event_node = Some(name.clone());

                // Read dev file to get major:minor
                let dev_file = entry.path().join("dev");
                let dev_str = std::fs::read_to_string(&dev_file)
                    .context(format!("Failed to read {}", dev_file.display()))?;
                let (major, minor) = parse_dev_string(&dev_str)?;

                create_device_node(&name, major, minor)?;
                info!("Created /dev/input/{} ({}:{})", name, major, minor);
            } else if name.starts_with("js") {
                joystick_node = Some(name.clone());

                let dev_file = entry.path().join("dev");
                let dev_str = std::fs::read_to_string(&dev_file)
                    .context(format!("Failed to read {}", dev_file.display()))?;
                let (major, minor) = parse_dev_string(&dev_str)?;

                create_device_node(&name, major, minor)?;
                info!("Created /dev/input/{} ({}:{})", name, major, minor);
            }
        }

        let event_node = event_node.ok_or_else(|| anyhow!("No event node found in sysfs"))?;

        Ok(Self {
            id,
            config,
            event_node,
            joystick_node,
            uinput_fd,
        })
    }

    /// Send input events to kernel uinput device
    pub fn send_events(&self, events: &[InputEvent]) -> Result<()> {
        let linux_events: Vec<LinuxInputEvent> =
            events.iter().map(|e| e.to_linux_input_event()).collect();

        let raw_fd = self.uinput_fd.as_raw_fd();

        // Write events directly to kernel
        for event in &linux_events {
            let bytes = event.to_bytes();
            let written = unsafe { libc::write(raw_fd, bytes.as_ptr() as *const _, bytes.len()) };

            if written < 0 {
                return Err(anyhow!(
                    "Failed to write event: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }

        Ok(())
    }

    /// Read force feedback events from uinput (blocking)
    pub fn read_feedback_event(&self) -> Result<FeedbackEvent> {
        let mut event_buf = [0u8; 24]; // sizeof(input_event)
        let raw_fd = self.uinput_fd.as_raw_fd();

        let read_bytes =
            unsafe { libc::read(raw_fd, event_buf.as_mut_ptr() as *mut _, event_buf.len()) };

        if read_bytes != 24 {
            return Err(anyhow!("Failed to read full event"));
        }

        let event: LinuxInputEvent = unsafe { std::ptr::read(event_buf.as_ptr() as *const _) };

        // Parse force feedback event
        if event.event_type == EV_FF {
            match event.code {
                FF_RUMBLE => {
                    if event.value == 0 {
                        Ok(FeedbackEvent::RumbleStop)
                    } else {
                        // Rumble magnitude encoded in value
                        let strong = (event.value >> 16) as u16;
                        let weak = (event.value & 0xFFFF) as u16;

                        Ok(FeedbackEvent::Rumble {
                            strong_magnitude: strong,
                            weak_magnitude: weak,
                            duration_ms: 0, // TODO: parse duration from effect
                        })
                    }
                }
                _ => Ok(FeedbackEvent::Raw {
                    code: event.code,
                    value: event.value,
                }),
            }
        } else {
            Err(anyhow!("Not a force feedback event"))
        }
    }
}

impl Drop for VirtualDevice {
    fn drop(&mut self) {
        let raw_fd = self.uinput_fd.as_raw_fd();

        // Destroy uinput device
        unsafe {
            ioctl(raw_fd, UI_DEV_DESTROY, 0);
        }

        // Remove device nodes
        let _ = std::fs::remove_file(PathBuf::from("/dev/input").join(&self.event_node));
        if let Some(js_node) = &self.joystick_node {
            let _ = std::fs::remove_file(PathBuf::from("/dev/input").join(js_node));
        }

        info!("Device {} destroyed and cleaned up", self.event_node);
    }
}

// Helper functions

fn parse_dev_string(s: &str) -> Result<(u32, u32)> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() != 2 {
        return Err(anyhow!("Invalid dev string format: {}", s));
    }
    Ok((parts[0].parse()?, parts[1].parse()?))
}

fn create_device_node(name: &str, major: u32, minor: u32) -> Result<()> {
    let path = PathBuf::from("/dev/input").join(name);

    // Create /dev/input if it doesn't exist
    std::fs::create_dir_all("/dev/input")?;

    // Remove old node if exists
    let _ = std::fs::remove_file(&path);

    // Create character device node
    let path_cstr = CString::new(path.to_str().unwrap())?;
    let dev = libc::makedev(major, minor);

    let result = unsafe { libc::mknod(path_cstr.as_ptr(), libc::S_IFCHR | 0o666, dev) };

    if result != 0 {
        return Err(anyhow!("mknod failed: {}", std::io::Error::last_os_error()));
    }

    Ok(())
}

unsafe fn ioctl(fd: i32, request: u64, arg: u64) -> i32 {
    unsafe { libc::ioctl(fd, request as libc::c_ulong, arg) }
}
