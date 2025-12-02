use crate::ptrace_util::{write_bytes, write_struct};
use crate::seccomp::SeccompNotifResp;
use crate::state::{VirtualFdContext, get_virtual_fd};
use nix::unistd::Pid;
use tracing::*;

// Event type constants
const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const EV_ABS: u16 = 0x03;
const EV_FF: u16 = 0x15;

// Force feedback
const FF_RUMBLE: u16 = 0x50;

// ioctl direction bits
const IOC_NONE: u32 = 0;
const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;

const fn ioc_dir(cmd: u32) -> u32 {
    (cmd >> 30) & 0x3
}

const fn ioc_type(cmd: u32) -> u8 {
    ((cmd >> 8) & 0xff) as u8
}

const fn ioc_nr(cmd: u32) -> u8 {
    (cmd & 0xff) as u8
}

const fn ioc_size(cmd: u32) -> usize {
    ((cmd >> 16) & 0x3fff) as usize
}

// Fixed ioctl codes
const EVIOCGVERSION: u32 = 0x80044501;
const EVIOCGID: u32 = 0x80084502;
const EVIOCGRAB: u32 = 0x40044590;
const EVIOCREVOKE: u32 = 0x40044591;

// struct input_id
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

// struct input_absinfo
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct InputAbsinfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

pub enum IoctlResult {
    Handled(SeccompNotifResp),
    NotVirtualFd,
}

pub fn handle_ioctl(pid: Pid, fd: i32, cmd: u32, arg: u64) -> IoctlResult {
    // Check if this FD is a virtual device
    let ctx = match get_virtual_fd(pid, fd) {
        Some(ctx) => ctx,
        None => return IoctlResult::NotVirtualFd,
    };

    let request_type = ioc_type(cmd);
    let request_nr = ioc_nr(cmd);
    let request_size = ioc_size(cmd);

    debug!(
        "ioctl on virtual fd {}: cmd={:#x}, type={} (0x{:02x}), nr=0x{:02x}, size={}, dir={}",
        fd,
        cmd,
        request_type as char,
        request_type,
        request_nr,
        request_size,
        ioc_dir(cmd)
    );

    // Handle by ioctl type
    let result = if request_type == b'E' {
        handle_evdev_ioctl(pid, cmd, arg, &ctx)
    } else if request_type == b'j' {
        handle_joystick_ioctl(pid, cmd, arg, &ctx)
    } else {
        debug!(
            "Unknown ioctl type '{}' (0x{:02x})",
            request_type as char, request_type
        );
        Err(libc::ENOTTY)
    };

    match result {
        Ok(ret) => IoctlResult::Handled(SeccompNotifResp::new_success(ret)),
        Err(errno) => IoctlResult::Handled(SeccompNotifResp::new_error(errno)),
    }
}

fn handle_evdev_ioctl(pid: Pid, cmd: u32, arg: u64, ctx: &VirtualFdContext) -> Result<i64, i32> {
    let request_nr = ioc_nr(cmd);
    let request_size = ioc_size(cmd);

    match cmd {
        EVIOCGVERSION => {
            let version: i32 = 0x010001;
            write_struct(pid, arg as usize, &version).map_err(|e| {
                debug!("Failed to write version: {}", e);
                libc::EFAULT
            })?;
            debug!("EVIOCGVERSION: returned {:#x}", version);
            Ok(0)
        }

        EVIOCGID => {
            let id = InputId {
                bustype: ctx.config.bustype as u16,
                vendor: ctx.config.vendor_id,
                product: ctx.config.product_id,
                version: ctx.config.version,
            };
            write_struct(pid, arg as usize, &id).map_err(|e| {
                debug!("Failed to write input_id: {}", e);
                libc::EFAULT
            })?;
            debug!(
                "EVIOCGID: bus={:#x}, vendor={:#x}, product={:#x}, version={:#x}",
                id.bustype, id.vendor, id.product, id.version
            );
            Ok(0)
        }

        EVIOCGRAB => {
            // Pretend grab succeeded
            debug!("EVIOCGRAB: pretending success");
            Ok(0)
        }

        EVIOCREVOKE => {
            debug!("EVIOCREVOKE: pretending success");
            Ok(0)
        }

        _ => {
            // Handle by request number
            match request_nr {
                // EVIOCGNAME
                0x06 => {
                    let name = &ctx.config.name;
                    let name_bytes = name.as_bytes();
                    let copy_len = std::cmp::min(name_bytes.len(), request_size.saturating_sub(1));
                    let mut buf = vec![0u8; request_size];
                    buf[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

                    write_bytes(pid, arg as usize, &buf).map_err(|e| {
                        debug!("Failed to write name: {}", e);
                        libc::EFAULT
                    })?;
                    debug!("EVIOCGNAME: {} ({} bytes)", name, copy_len);
                    Ok(copy_len as i64)
                }

                // EVIOCGPHYS
                0x07 => {
                    let phys = format!("usb-vimputti.0/input{}", ctx.device_id);
                    let phys_bytes = phys.as_bytes();
                    let copy_len = std::cmp::min(phys_bytes.len(), request_size.saturating_sub(1));
                    let mut buf = vec![0u8; request_size];
                    buf[..copy_len].copy_from_slice(&phys_bytes[..copy_len]);

                    write_bytes(pid, arg as usize, &buf).map_err(|e| {
                        debug!("Failed to write phys: {}", e);
                        libc::EFAULT
                    })?;
                    debug!("EVIOCGPHYS: {}", phys);
                    Ok(copy_len as i64)
                }

                // EVIOCGUNIQ
                0x08 => {
                    let uniq = format!("{}", ctx.device_id);
                    let uniq_bytes = uniq.as_bytes();
                    let copy_len = std::cmp::min(uniq_bytes.len(), request_size.saturating_sub(1));
                    let mut buf = vec![0u8; request_size];
                    buf[..copy_len].copy_from_slice(&uniq_bytes[..copy_len]);

                    write_bytes(pid, arg as usize, &buf).map_err(|e| {
                        debug!("Failed to write uniq: {}", e);
                        libc::EFAULT
                    })?;
                    debug!("EVIOCGUNIQ: {}", uniq);
                    Ok(copy_len as i64)
                }

                // EVIOCGPROP
                0x09 => {
                    let buf = vec![0u8; request_size];
                    write_bytes(pid, arg as usize, &buf).map_err(|e| {
                        debug!("Failed to write prop: {}", e);
                        libc::EFAULT
                    })?;
                    debug!("EVIOCGPROP: (none)");
                    Ok(0)
                }

                // EVIOCGKEY - get current key state
                0x18 => {
                    let buf = vec![0u8; request_size];
                    write_bytes(pid, arg as usize, &buf).map_err(|e| {
                        debug!("Failed to write key state: {}", e);
                        libc::EFAULT
                    })?;
                    debug!("EVIOCGKEY: (all released)");
                    Ok(0)
                }

                // EVIOCGBIT range: 0x20-0x3f (ev_type = nr - 0x20)
                nr if nr >= 0x20 && nr < 0x40 => {
                    let ev_type = nr - 0x20;
                    handle_eviocgbit(pid, arg, request_size, ev_type as u16, ctx)
                }

                // EVIOCGABS range: 0x40-0x7f (abs_code = nr - 0x40)
                nr if nr >= 0x40 && nr < 0x80 => {
                    let abs_code = nr - 0x40;
                    handle_eviocgabs(pid, arg, abs_code as u16, ctx)
                }

                _ => {
                    debug!(
                        "Unhandled evdev ioctl nr=0x{:02x}, size={}",
                        request_nr, request_size
                    );
                    // For read ioctls, zero out the buffer
                    if ioc_dir(cmd) == IOC_READ && request_size > 0 {
                        let buf = vec![0u8; request_size];
                        let _ = write_bytes(pid, arg as usize, &buf);
                    }
                    Ok(0)
                }
            }
        }
    }
}

fn handle_eviocgbit(
    pid: Pid,
    arg: u64,
    size: usize,
    ev_type: u16,
    ctx: &VirtualFdContext,
) -> Result<i64, i32> {
    let mut bits = vec![0u8; size];

    match ev_type {
        0 => {
            // EV types supported - this is the critical one!
            // Bit 0 = EV_SYN, Bit 1 = EV_KEY, Bit 3 = EV_ABS
            // 0b00001011 = SYN + KEY + ABS
            if size > 0 {
                bits[0] = 0b00001011; // EV_SYN | EV_KEY | EV_ABS
            }
            // Add EV_FF if we want force feedback (bit 0x15 = 21)
            if size > 2 {
                bits[2] |= 1 << (EV_FF % 8); // EV_FF = 0x15 = 21, 21/8=2, 21%8=5
            }
            debug!("EVIOCGBIT(EV): SYN, KEY, ABS, FF");
        }
        EV_KEY => {
            // Button bits
            for button in &ctx.config.buttons {
                let code = button.to_ev_code() as usize;
                if code / 8 < size {
                    set_bit(&mut bits, code);
                }
            }
            debug!("EVIOCGBIT(KEY): {} buttons", ctx.config.buttons.len());
        }
        EV_REL => {
            // No relative axes
            debug!("EVIOCGBIT(REL): (none)");
        }
        EV_ABS => {
            // Axis bits
            for axis_config in &ctx.config.axes {
                let code = axis_config.axis.to_ev_code() as usize;
                if code / 8 < size {
                    set_bit(&mut bits, code);
                }
            }
            debug!("EVIOCGBIT(ABS): {} axes", ctx.config.axes.len());
        }
        EV_FF => {
            // Force feedback capabilities
            let ff_rumble_code = FF_RUMBLE as usize;
            if ff_rumble_code / 8 < size {
                set_bit(&mut bits, ff_rumble_code);
            }
            debug!("EVIOCGBIT(FF): RUMBLE");
        }
        _ => {
            debug!("EVIOCGBIT({}): (none)", ev_type);
        }
    }

    write_bytes(pid, arg as usize, &bits).map_err(|e| {
        debug!("Failed to write bits: {}", e);
        libc::EFAULT
    })?;

    Ok(0)
}

fn handle_eviocgabs(pid: Pid, arg: u64, abs_code: u16, ctx: &VirtualFdContext) -> Result<i64, i32> {
    // Find the axis config for this abs code
    let axis_config = ctx
        .config
        .axes
        .iter()
        .find(|a| a.axis.to_ev_code() == abs_code);

    let absinfo = match axis_config {
        Some(cfg) => {
            // Use fuzz/flat based on range like the shim does
            let (fuzz, flat) = if cfg.max > 1000 { (16, 128) } else { (0, 0) };
            InputAbsinfo {
                value: 0,
                minimum: cfg.min,
                maximum: cfg.max,
                fuzz,
                flat,
                resolution: 0,
            }
        }
        None => {
            // Return defaults for unknown axes (like the shim does)
            debug!(
                "EVIOCGABS({}): axis not in config, using defaults",
                abs_code
            );
            InputAbsinfo {
                value: 0,
                minimum: -32768,
                maximum: 32767,
                fuzz: 16,
                flat: 128,
                resolution: 0,
            }
        }
    };

    write_struct(pid, arg as usize, &absinfo).map_err(|e| {
        debug!("Failed to write absinfo: {}", e);
        libc::EFAULT
    })?;

    debug!(
        "EVIOCGABS({}): min={}, max={}, fuzz={}, flat={}",
        abs_code, absinfo.minimum, absinfo.maximum, absinfo.fuzz, absinfo.flat
    );
    Ok(0)
}

fn handle_joystick_ioctl(pid: Pid, cmd: u32, arg: u64, ctx: &VirtualFdContext) -> Result<i64, i32> {
    const JSIOCGVERSION: u32 = 0x80046a01;
    const JSIOCGAXES: u32 = 0x80016a11;
    const JSIOCGBUTTONS: u32 = 0x80016a12;

    let request_nr = ioc_nr(cmd);
    let request_size = ioc_size(cmd);

    match cmd {
        JSIOCGVERSION => {
            let version: i32 = 0x020100; // Version 2.1.0
            write_struct(pid, arg as usize, &version).map_err(|_| libc::EFAULT)?;
            debug!("JSIOCGVERSION: {:#x}", version);
            Ok(0)
        }

        JSIOCGAXES => {
            let num_axes: u8 = ctx.config.axes.len() as u8;
            write_struct(pid, arg as usize, &num_axes).map_err(|_| libc::EFAULT)?;
            debug!("JSIOCGAXES: {}", num_axes);
            Ok(0)
        }

        JSIOCGBUTTONS => {
            let num_buttons: u8 = ctx.config.buttons.len() as u8;
            write_struct(pid, arg as usize, &num_buttons).map_err(|_| libc::EFAULT)?;
            debug!("JSIOCGBUTTONS: {}", num_buttons);
            Ok(0)
        }

        _ => {
            // JSIOCGNAME has variable size
            if request_nr == 0x13 {
                let name = &ctx.config.name;
                let name_bytes = name.as_bytes();
                let copy_len = std::cmp::min(name_bytes.len(), request_size.saturating_sub(1));
                let mut buf = vec![0u8; request_size];
                buf[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

                write_bytes(pid, arg as usize, &buf).map_err(|_| libc::EFAULT)?;
                debug!("JSIOCGNAME: {}", name);
                return Ok(copy_len as i64);
            }

            // JSIOCGAXMAP (0x32)
            if request_nr == 0x32 {
                let mut axis_map = Vec::new();
                for axis_config in &ctx.config.axes {
                    axis_map.push(axis_config.axis.to_ev_code() as u8);
                }
                let copy_len = std::cmp::min(axis_map.len(), request_size);
                let mut buf = vec![0u8; request_size];
                buf[..copy_len].copy_from_slice(&axis_map[..copy_len]);

                write_bytes(pid, arg as usize, &buf).map_err(|_| libc::EFAULT)?;
                debug!("JSIOCGAXMAP: {} axes", axis_map.len());
                return Ok(0);
            }

            // JSIOCGBTNMAP (0x34)
            if request_nr == 0x34 {
                let mut button_map: Vec<u16> = Vec::new();
                for button in &ctx.config.buttons {
                    button_map.push(button.to_ev_code());
                }
                let buf_u8 = unsafe {
                    std::slice::from_raw_parts(
                        button_map.as_ptr() as *const u8,
                        button_map.len() * 2,
                    )
                };
                let copy_len = std::cmp::min(buf_u8.len(), request_size);
                let mut buf = vec![0u8; request_size];
                buf[..copy_len].copy_from_slice(&buf_u8[..copy_len]);

                write_bytes(pid, arg as usize, &buf).map_err(|_| libc::EFAULT)?;
                debug!("JSIOCGBTNMAP: {} buttons", button_map.len());
                return Ok(0);
            }

            debug!("Unhandled joystick ioctl nr=0x{:02x}", request_nr);
            Ok(0)
        }
    }
}

fn set_bit(bits: &mut [u8], bit: usize) {
    let byte_idx = bit / 8;
    let bit_idx = bit % 8;
    if byte_idx < bits.len() {
        bits[byte_idx] |= 1 << bit_idx;
    }
}
