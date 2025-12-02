use lazy_static::lazy_static;
use nix::unistd::Pid;
use std::collections::{HashMap, HashSet};
use std::os::unix::io::RawFd;
use std::sync::{Arc, Mutex};
use vimputti::protocol::DeviceConfig;

#[derive(Clone, Debug)]
pub struct VirtualFdContext {
    pub event_node: String,
    pub device_type: DeviceType,
    pub device_id: u64,
    pub manager_fd: RawFd,
    pub config: DeviceConfig, // Store for ioctl emulation
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DeviceType {
    Event,
    Joystick,
    Uinput,
}

lazy_static! {
    pub static ref PROCESS_STATE: Arc<Mutex<HashMap<Pid, Arc<Mutex<ProcessFdMap>>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Global list of our ends of datagram socket pairs (for broadcasting events)
    static ref UDEV_BROADCAST_SOCKETS: Arc<Mutex<Vec<RawFd>>> = Arc::new(Mutex::new(Vec::new()));
}

#[derive(Default)]
pub struct ProcessFdMap {
    pub virtual_fds: HashMap<RawFd, VirtualFdContext>,
    pub tracked_sockets: HashSet<RawFd>,
    pub netlink_sockets: HashSet<RawFd>,
    pub udev_sockets: HashMap<RawFd, RawFd>, // target_fd -> our_fd
}

pub fn get_fd_map(pid: Pid) -> Arc<Mutex<ProcessFdMap>> {
    let mut state = PROCESS_STATE.lock().unwrap();
    state
        .entry(pid)
        .or_insert_with(|| Arc::new(Mutex::new(ProcessFdMap::default())))
        .clone()
}

pub fn is_virtual_fd(pid: Pid, fd: RawFd) -> bool {
    get_fd_map(pid)
        .lock()
        .unwrap()
        .virtual_fds
        .contains_key(&fd)
}

pub fn register_virtual_fd(pid: Pid, fd: RawFd, ctx: VirtualFdContext) {
    get_fd_map(pid).lock().unwrap().virtual_fds.insert(fd, ctx);
}

pub fn get_virtual_fd(pid: Pid, fd: RawFd) -> Option<VirtualFdContext> {
    get_fd_map(pid)
        .lock()
        .unwrap()
        .virtual_fds
        .get(&fd)
        .cloned()
}

pub fn cleanup_virtual_fd(pid: Pid, fd: RawFd) {
    if let Some(ctx) = get_fd_map(pid).lock().unwrap().virtual_fds.remove(&fd) {
        // Close manager connection
        unsafe { libc::close(ctx.manager_fd) };
    }
}

pub fn track_unix_socket(pid: Pid, fd: RawFd) {
    get_fd_map(pid).lock().unwrap().tracked_sockets.insert(fd);
}

pub fn is_tracked_unix_socket(pid: Pid, fd: RawFd) -> bool {
    get_fd_map(pid)
        .lock()
        .unwrap()
        .tracked_sockets
        .contains(&fd)
}

pub fn inherit_fd_map(parent: Pid, child: Pid) {
    let parent_map = get_fd_map(parent);
    let parent_fds = parent_map.lock().unwrap().virtual_fds.clone();

    let child_map = get_fd_map(child);
    let mut child_guard = child_map.lock().unwrap();

    for (fd, ctx) in parent_fds {
        child_guard.virtual_fds.insert(fd, ctx);
    }
}

pub fn track_netlink_socket(pid: Pid, fd: RawFd) {
    get_fd_map(pid).lock().unwrap().netlink_sockets.insert(fd);
}

pub fn is_netlink_socket(pid: Pid, fd: RawFd) -> bool {
    get_fd_map(pid)
        .lock()
        .unwrap()
        .netlink_sockets
        .contains(&fd)
}

pub fn register_udev_socket(pid: Pid, target_fd: RawFd, our_fd: RawFd) {
    get_fd_map(pid)
        .lock()
        .unwrap()
        .udev_sockets
        .insert(target_fd, our_fd);
}

pub fn get_udev_socket(pid: Pid, fd: RawFd) -> Option<RawFd> {
    get_fd_map(pid)
        .lock()
        .unwrap()
        .udev_sockets
        .get(&fd)
        .copied()
}

pub fn register_udev_broadcast_socket(our_fd: RawFd) {
    UDEV_BROADCAST_SOCKETS.lock().unwrap().push(our_fd);
}

pub fn get_all_udev_broadcast_sockets() -> Vec<RawFd> {
    UDEV_BROADCAST_SOCKETS.lock().unwrap().clone()
}

pub fn remove_udev_broadcast_socket(fd: RawFd) {
    UDEV_BROADCAST_SOCKETS.lock().unwrap().retain(|&f| f != fd);
}
