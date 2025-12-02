pub struct PathRedirector;

impl PathRedirector {
    const BASE_PATH: &'static str = "/tmp/vimputti";

    /// Check if path should be redirected and return new path
    pub fn redirect(path: &str) -> Option<String> {
        match path {
            "/dev/uinput" => Some(format!("{}/uinput", Self::BASE_PATH)),
            "/dev/input" => Some(format!("{}/devices", Self::BASE_PATH)),
            "/sys/class/input" => Some(format!("{}/sysfs/class/input", Self::BASE_PATH)),
            "/sys/devices/virtual/input" => {
                Some(format!("{}/sysfs/devices/virtual/input", Self::BASE_PATH))
            }
            "/run/udev/control" => Some(format!("{}/udev", Self::BASE_PATH)),
            "/run/udev/data" => Some(format!("{}/udev_data", Self::BASE_PATH)),
            _ => Self::redirect_prefix(path),
        }
    }

    fn redirect_prefix(path: &str) -> Option<String> {
        if let Some(suffix) = path.strip_prefix("/dev/input/") {
            return Some(format!("{}/devices/{}", Self::BASE_PATH, suffix));
        }
        if let Some(suffix) = path.strip_prefix("/sys/class/input/") {
            return Some(format!("{}/sysfs/class/input/{}", Self::BASE_PATH, suffix));
        }
        if let Some(suffix) = path.strip_prefix("/sys/devices/virtual/input/") {
            return Some(format!(
                "{}/sysfs/devices/virtual/input/{}",
                Self::BASE_PATH,
                suffix
            ));
        }
        if let Some(suffix) = path.strip_prefix("/run/udev/data/") {
            return Some(format!("{}/udev_data/{}", Self::BASE_PATH, suffix));
        }
        None
    }

    /// Check if path is an input device node requiring socket connection + handshake
    /// This excludes feedback sockets and other special files
    pub fn is_input_device(path: &str) -> bool {
        // Exclude feedback sockets
        if path.contains(".feedback") {
            return false;
        }

        path == "/dev/uinput" || Self::is_event_device(path) || Self::is_joystick_device(path)
    }

    /// Check if path is an evdev event node (e.g., /dev/input/event0)
    pub fn is_event_device(path: &str) -> bool {
        if let Some(suffix) = path.strip_prefix("/dev/input/event") {
            // Must be followed by digits only
            !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit())
        } else {
            false
        }
    }

    /// Check if path is a joystick node (e.g., /dev/input/js0)
    pub fn is_joystick_device(path: &str) -> bool {
        if let Some(suffix) = path.strip_prefix("/dev/input/js") {
            // Must be followed by digits only
            !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit())
        } else {
            false
        }
    }
}
