use anyhow::{Result, anyhow};
use libc::c_ulong;
use std::os::unix::io::RawFd;
use tracing::*;

// seccomp constants
const SECCOMP_SET_MODE_FILTER: c_ulong = 1;
const SECCOMP_FILTER_FLAG_NEW_LISTENER: c_ulong = 1 << 3;
const SECCOMP_USER_NOTIF_FLAG_CONTINUE: u32 = 1;

const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
const SECCOMP_RET_USER_NOTIF: u32 = 0x7fc0_0000;

// BPF instruction constants
const BPF_LD: u16 = 0x00;
const BPF_W: u16 = 0x00;
const BPF_ABS: u16 = 0x20;
const BPF_JMP: u16 = 0x05;
const BPF_JEQ: u16 = 0x10;
const BPF_K: u16 = 0x00;
const BPF_RET: u16 = 0x06;

// ioctl commands - these need to match the kernel exactly
// From linux/seccomp.h:
// #define SECCOMP_IOCTL_NOTIF_RECV        SECCOMP_IOWR(0, struct seccomp_notif)
// #define SECCOMP_IOCTL_NOTIF_SEND        SECCOMP_IOWR(1, struct seccomp_notif_resp)
// #define SECCOMP_IOCTL_NOTIF_ID_VALID    SECCOMP_IOW(2, __u64)
// #define SECCOMP_IOCTL_NOTIF_ADDFD       SECCOMP_IOW(3, struct seccomp_notif_addfd)

// Let's compute these properly
// _IOWR('!', 0, struct seccomp_notif) where seccomp_notif is 80 bytes
// _IOWR('!', 1, struct seccomp_notif_resp) where seccomp_notif_resp is 24 bytes
// _IOW('!', 2, __u64)
// _IOW('!', 3, struct seccomp_notif_addfd) where seccomp_notif_addfd is 24 bytes

fn _IOC(dir: c_ulong, ty: c_ulong, nr: c_ulong, size: c_ulong) -> c_ulong {
    (dir << 30) | (ty << 8) | nr | (size << 16)
}

const _IOC_WRITE: c_ulong = 1;
const _IOC_READ: c_ulong = 2;

fn _IOW(ty: u8, nr: u8, size: usize) -> c_ulong {
    _IOC(_IOC_WRITE, ty as c_ulong, nr as c_ulong, size as c_ulong)
}

fn _IOWR(ty: u8, nr: u8, size: usize) -> c_ulong {
    _IOC(
        _IOC_READ | _IOC_WRITE,
        ty as c_ulong,
        nr as c_ulong,
        size as c_ulong,
    )
}

// '!' = 0x21
const SECCOMP_IOC_MAGIC: u8 = b'!';

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const libc::sock_filter,
}

// Must match kernel's struct seccomp_data exactly (56 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SeccompData {
    pub nr: i32,                  // 4 bytes
    pub arch: u32,                // 4 bytes
    pub instruction_pointer: u64, // 8 bytes
    pub args: [u64; 6],           // 48 bytes
} // Total: 64 bytes

// Must match kernel's struct seccomp_notif exactly
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SeccompNotif {
    pub id: u64,           // 8 bytes
    pub pid: u32,          // 4 bytes
    pub flags: u32,        // 4 bytes
    pub data: SeccompData, // 64 bytes
} // Total: 80 bytes

#[derive(Debug)]
pub struct SeccompNotifData {
    pub id: u64,
    pub pid: u32,
    pub data: SeccompData,
}

// Must match kernel's struct seccomp_notif_resp exactly (24 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SeccompNotifResp {
    pub id: u64,    // 8 bytes
    pub val: i64,   // 8 bytes
    pub error: i32, // 4 bytes
    pub flags: u32, // 4 bytes
} // Total: 24 bytes

// Must match kernel's struct seccomp_notif_addfd (24 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SeccompNotifAddFd {
    pub id: u64,          // 8 bytes
    pub flags: u32,       // 4 bytes
    pub srcfd: u32,       // 4 bytes
    pub newfd: u32,       // 4 bytes
    pub newfd_flags: u32, // 4 bytes
} // Total: 24 bytes

impl SeccompNotifResp {
    pub fn new_success(val: i64) -> Self {
        Self {
            id: 0,
            val,
            error: 0,
            flags: 0,
        }
    }

    pub fn new_error(error: i32) -> Self {
        Self {
            id: 0,
            val: -1,
            error: -error.abs(),
            flags: 0,
        }
    }

    /// Tell the kernel to continue executing the syscall normally.
    /// This is used when we intercept a syscall but decide not to handle it.
    pub fn new_continue() -> Self {
        Self {
            id: 0,
            val: 0,
            error: 0,
            flags: SECCOMP_USER_NOTIF_FLAG_CONTINUE,
        }
    }
}

/// Check if notification ID is still valid
pub fn notif_id_valid(notif_fd: RawFd, id: u64) -> bool {
    let cmd = _IOW(SECCOMP_IOC_MAGIC, 2, std::mem::size_of::<u64>());
    let ret = unsafe { libc::ioctl(notif_fd, cmd as _, &id as *const _) };
    ret == 0
}

/// Inject an FD into the target process, returning the FD number in the target.
pub fn notif_addfd(notif_fd: RawFd, id: u64, src_fd: RawFd) -> Result<RawFd> {
    let addfd = SeccompNotifAddFd {
        id,
        flags: 0,
        srcfd: src_fd as u32,
        newfd: 0,
        newfd_flags: 0,
    };

    let cmd = _IOW(
        SECCOMP_IOC_MAGIC,
        3,
        std::mem::size_of::<SeccompNotifAddFd>(),
    );
    debug!(
        "ADDFD ioctl cmd: {:#x}, struct size: {}",
        cmd,
        std::mem::size_of::<SeccompNotifAddFd>()
    );

    let ret = unsafe { libc::ioctl(notif_fd, cmd as _, &addfd as *const _) };

    if ret < 0 {
        return Err(anyhow!(
            "SECCOMP_IOCTL_NOTIF_ADDFD failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    Ok(ret as RawFd)
}

/// Install seccomp filter and return notification FD
pub fn install_filter() -> Result<RawFd> {
    let ret = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if ret < 0 {
        return Err(anyhow!(
            "prctl(PR_SET_NO_NEW_PRIVS) failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    let filter = build_input_device_filter();

    let prog = SockFprog {
        len: filter.len() as u16,
        filter: filter.as_ptr(),
    };

    let ret = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            SECCOMP_SET_MODE_FILTER,
            SECCOMP_FILTER_FLAG_NEW_LISTENER,
            &prog as *const _,
        )
    };

    if ret < 0 {
        return Err(anyhow!(
            "seccomp failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    info!("Seccomp filter installed, notification fd: {}", ret);
    debug!("SeccompNotif size: {}", std::mem::size_of::<SeccompNotif>());
    debug!(
        "SeccompNotifResp size: {}",
        std::mem::size_of::<SeccompNotifResp>()
    );
    debug!(
        "SeccompNotifAddFd size: {}",
        std::mem::size_of::<SeccompNotifAddFd>()
    );

    Ok(ret as RawFd)
}

fn build_input_device_filter() -> Vec<libc::sock_filter> {
    let syscalls: &[i64] = &[
        libc::SYS_openat,
        libc::SYS_ioctl,
        libc::SYS_newfstatat, // This is what stat() uses on x86_64
        libc::SYS_socket,
        libc::SYS_bind,
    ];

    let mut filter = Vec::new();

    // Load syscall number
    filter.push(libc::sock_filter {
        code: BPF_LD | BPF_W | BPF_ABS,
        jt: 0,
        jf: 0,
        k: 0, // offsetof(struct seccomp_data, nr)
    });

    // Check each syscall
    for &nr in syscalls.iter() {
        filter.push(libc::sock_filter {
            code: BPF_JMP | BPF_JEQ | BPF_K,
            jt: 0,
            jf: 1,
            k: nr as u32,
        });

        filter.push(libc::sock_filter {
            code: BPF_RET | BPF_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_USER_NOTIF,
        });
    }

    // Default: allow
    filter.push(libc::sock_filter {
        code: BPF_RET | BPF_K,
        jt: 0,
        jf: 0,
        k: SECCOMP_RET_ALLOW,
    });

    debug!(
        "Built BPF filter with {} instructions for {} syscalls",
        filter.len(),
        syscalls.len()
    );
    filter
}

pub fn notif_receive(fd: RawFd) -> Result<SeccompNotifData> {
    let mut req: SeccompNotif = unsafe { std::mem::zeroed() };

    let cmd = _IOWR(SECCOMP_IOC_MAGIC, 0, std::mem::size_of::<SeccompNotif>());

    let ret = unsafe { libc::ioctl(fd, cmd as _, &mut req as *mut _) };
    if ret < 0 {
        return Err(anyhow!(
            "ioctl NOTIF_RECV failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    Ok(SeccompNotifData {
        id: req.id,
        pid: req.pid,
        data: req.data,
    })
}

pub fn notif_respond(fd: RawFd, id: u64, val: i64, error: i32, flags: u32) -> Result<()> {
    let resp = SeccompNotifResp {
        id,
        val,
        error,
        flags,
    };

    let cmd = _IOWR(
        SECCOMP_IOC_MAGIC,
        1,
        std::mem::size_of::<SeccompNotifResp>(),
    );
    trace!(
        "SEND ioctl cmd: {:#x}, id: {}, val: {}, error: {}",
        cmd, id, val, error
    );

    let ret = unsafe { libc::ioctl(fd, cmd as _, &resp as *const _) };
    if ret < 0 {
        return Err(anyhow!(
            "ioctl NOTIF_SEND failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    Ok(())
}
