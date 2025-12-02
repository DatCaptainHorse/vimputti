use anyhow::{Result, anyhow};
use nix::unistd::Pid;

/// Read a null-terminated string from process memory using process_vm_readv
pub fn read_string(pid: Pid, addr: usize) -> Result<String> {
    let mut result = Vec::new();
    let mut offset = 0;
    let chunk_size = 256;

    loop {
        let mut buf = vec![0u8; chunk_size];

        let local_iov = libc::iovec {
            iov_base: buf.as_mut_ptr() as *mut _,
            iov_len: chunk_size,
        };

        let remote_iov = libc::iovec {
            iov_base: (addr + offset) as *mut _,
            iov_len: chunk_size,
        };

        let ret = unsafe { libc::process_vm_readv(pid.as_raw(), &local_iov, 1, &remote_iov, 1, 0) };

        if ret < 0 {
            let err = std::io::Error::last_os_error();
            // If we've read something and hit an error, try to use what we have
            if !result.is_empty() {
                break;
            }
            return Err(anyhow!("process_vm_readv failed: {}", err));
        }

        let bytes_read = ret as usize;
        if bytes_read == 0 {
            break;
        }

        // Look for null terminator
        if let Some(pos) = buf[..bytes_read].iter().position(|&b| b == 0) {
            result.extend_from_slice(&buf[..pos]);
            break;
        }

        result.extend_from_slice(&buf[..bytes_read]);
        offset += bytes_read;

        // Sanity limit
        if offset > 4096 {
            return Err(anyhow!("String too long"));
        }
    }

    String::from_utf8(result).map_err(|e| anyhow!("Invalid UTF-8: {}", e))
}

/// Read bytes from process memory
pub fn read_bytes(pid: Pid, addr: usize, len: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; len];

    let local_iov = libc::iovec {
        iov_base: buf.as_mut_ptr() as *mut _,
        iov_len: len,
    };

    let remote_iov = libc::iovec {
        iov_base: addr as *mut _,
        iov_len: len,
    };

    let ret = unsafe { libc::process_vm_readv(pid.as_raw(), &local_iov, 1, &remote_iov, 1, 0) };

    if ret < 0 {
        return Err(anyhow!(
            "process_vm_readv failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    buf.truncate(ret as usize);
    Ok(buf)
}

/// Read a struct from process memory
pub fn read_struct<T: Copy>(pid: Pid, addr: usize) -> Result<T> {
    let size = std::mem::size_of::<T>();
    let bytes = read_bytes(pid, addr, size)?;

    if bytes.len() < size {
        return Err(anyhow!(
            "Short read: got {} bytes, expected {}",
            bytes.len(),
            size
        ));
    }

    let mut result: T = unsafe { std::mem::zeroed() };
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), &mut result as *mut _ as *mut u8, size);
    }

    Ok(result)
}

/// Write bytes to process memory
pub fn write_bytes(pid: Pid, addr: usize, data: &[u8]) -> Result<()> {
    let local_iov = libc::iovec {
        iov_base: data.as_ptr() as *mut _,
        iov_len: data.len(),
    };

    let remote_iov = libc::iovec {
        iov_base: addr as *mut _,
        iov_len: data.len(),
    };

    let ret = unsafe { libc::process_vm_writev(pid.as_raw(), &local_iov, 1, &remote_iov, 1, 0) };

    if ret < 0 {
        return Err(anyhow!(
            "process_vm_writev failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    Ok(())
}

/// Write a struct to process memory
pub fn write_struct<T: Copy>(pid: Pid, addr: usize, value: &T) -> Result<()> {
    let size = std::mem::size_of::<T>();
    let data = unsafe { std::slice::from_raw_parts(value as *const _ as *const u8, size) };
    write_bytes(pid, addr, data)
}
