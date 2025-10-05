pub struct PathRedirector {
    base_path: String,
}

impl PathRedirector {
    pub fn new() -> Self {
        let uid = unsafe { libc::getuid() };
        let base_path = std::env::var("VIMPUTTI_PATH")
            .unwrap_or_else(|_| format!("/run/user/{}/vimputti", uid));

        Self { base_path }
    }

    /// Check if a path should be redirected, and return the new path
    pub fn redirect(&self, path: &str) -> Option<String> {
        // Redirect /dev/input/eventX to our device sockets
        if path.starts_with("/dev/input/event") {
            return Some(format!(
                "{}/devices/{}",
                self.base_path,
                path.strip_prefix("/dev/input/").unwrap()
            ));
        }

        // Redirect /dev/input/jsX to our joystick sockets
        if path.starts_with("/dev/input/js") {
            return Some(format!(
                "{}/devices/{}",
                self.base_path,
                path.strip_prefix("/dev/input/").unwrap()
            ));
        }

        // Redirect /sys/class/input to our sysfs
        if path.starts_with("/sys/class/input/") {
            let suffix = path.strip_prefix("/sys/class/input/").unwrap();
            return Some(format!("{}/sysfs/class/input/{}", self.base_path, suffix));
        }

        // Redirect /sys/devices/virtual/input to our sysfs
        if path.starts_with("/sys/devices/virtual/input/") {
            let suffix = path.strip_prefix("/sys/devices/virtual/input/").unwrap();
            return Some(format!(
                "{}/sysfs/devices/virtual/input/{}",
                self.base_path, suffix
            ));
        }

        // Redirect /sys/class/input itself (for directory listing)
        if path == "/sys/class/input" {
            return Some(format!("{}/sysfs/class/input", self.base_path));
        }

        // Redirect /sys/devices/virtual/input itself
        if path == "/sys/devices/virtual/input" {
            return Some(format!("{}/sysfs/devices/virtual/input", self.base_path));
        }

        None
    }
}
