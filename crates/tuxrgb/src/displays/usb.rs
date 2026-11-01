//! USB bulk transfers through usbfs (Linux), for the pictures of the NZXT
//! Kraken screens: they are sent to a bulk endpoint of the cooler, next to
//! its HID interface. No libusb: one ioctl per transfer.
//!
//! On Windows the bulk interface has no driver unless one is installed by
//! hand (WinUSB), so the Kraken keeps its own screens there.

use std::io;

/// Writes to a bulk OUT endpoint.
pub trait BulkOut: Send {
    fn write(&mut self, data: &[u8]) -> io::Result<()>;
}

#[cfg(target_os = "linux")]
pub use linux::UsbBulk;

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::File;
    use std::io;
    use std::os::fd::AsRawFd;
    use std::path::Path;

    #[repr(C)]
    struct BulkTransfer {
        ep: u32,
        len: u32,
        timeout: u32,
        data: *mut libc::c_void,
    }

    // _IOWR('U', 2, struct usbdevfs_bulktransfer), _IOR('U', 15 / 16, uint).
    const IOC_READ: libc::c_ulong = 2;
    const IOC_WRITE: libc::c_ulong = 1;
    const fn ioc(dir: libc::c_ulong, nr: libc::c_ulong, size: usize) -> libc::c_ulong {
        (dir << 30) | ((size as libc::c_ulong) << 16) | ((b'U' as libc::c_ulong) << 8) | nr
    }
    const USBDEVFS_BULK: libc::c_ulong =
        ioc(IOC_READ | IOC_WRITE, 2, std::mem::size_of::<BulkTransfer>());
    const USBDEVFS_CLAIMINTERFACE: libc::c_ulong = ioc(IOC_READ, 15, 4);
    const USBDEVFS_RELEASEINTERFACE: libc::c_ulong = ioc(IOC_READ, 16, 4);

    /// Largest single transfer (usbfs splits nothing for us).
    const CHUNK: usize = 16 * 1024;

    /// A claimed interface of a USB device and one of its bulk OUT
    /// endpoints.
    pub struct UsbBulk {
        file: File,
        interface: u32,
        endpoint: u8,
        /// Largest transfer (the Kraken Z wants 512-byte ones).
        chunk: usize,
    }

    fn read_trimmed(p: &Path) -> Option<String> {
        std::fs::read_to_string(p).ok().map(|s| s.trim().to_owned())
    }

    impl UsbBulk {
        /// Opens the device described by `usb_sysfs` (its sysfs directory)
        /// and claims the interface owning `endpoint`.
        pub fn open(usb_sysfs: &Path, endpoint: u8, chunk: usize) -> io::Result<Self> {
            let bus: u32 = read_trimmed(&usb_sysfs.join("busnum"))
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "busnum"))?;
            let dev: u32 = read_trimmed(&usb_sysfs.join("devnum"))
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "devnum"))?;
            let interface = interface_of(usb_sysfs, endpoint).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "no interface with that endpoint")
            })?;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(format!("/dev/bus/usb/{bus:03}/{dev:03}"))?;
            let mut n: libc::c_uint = interface;
            // SAFETY: valid fd and pointer to an unsigned int.
            if unsafe { libc::ioctl(file.as_raw_fd(), USBDEVFS_CLAIMINTERFACE, &mut n) } < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                file,
                interface,
                endpoint,
                chunk: chunk.clamp(64, CHUNK),
            })
        }
    }

    /// Number of the interface of the device at `usb` that has endpoint
    /// `endpoint` (sysfs `<dev>:<cfg>.<n>/ep_XX`).
    pub fn interface_of(usb: &Path, endpoint: u8) -> Option<u32> {
        let name = usb.file_name()?.to_string_lossy().into_owned();
        let ep = format!("ep_{endpoint:02x}");
        std::fs::read_dir(usb)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(&format!("{name}:")))
            })
            .find(|p| p.join(&ep).exists())
            .and_then(|p| read_trimmed(&p.join("bInterfaceNumber")))
            .and_then(|n| u32::from_str_radix(&n, 16).ok())
    }

    impl super::BulkOut for UsbBulk {
        fn write(&mut self, data: &[u8]) -> io::Result<()> {
            for chunk in data.chunks(self.chunk) {
                let mut buf = chunk.to_vec();
                let mut t = BulkTransfer {
                    ep: u32::from(self.endpoint),
                    len: buf.len() as u32,
                    timeout: 2000,
                    data: buf.as_mut_ptr().cast(),
                };
                // SAFETY: the buffer outlives the synchronous ioctl.
                let r = unsafe { libc::ioctl(self.file.as_raw_fd(), USBDEVFS_BULK, &mut t) };
                if r < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        }
    }

    impl Drop for UsbBulk {
        fn drop(&mut self) {
            let mut n: libc::c_uint = self.interface;
            // SAFETY: valid fd and pointer.
            unsafe { libc::ioctl(self.file.as_raw_fd(), USBDEVFS_RELEASEINTERFACE, &mut n) };
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn ioctl_numbers_match_the_kernel_headers() {
            assert_eq!(std::mem::size_of::<BulkTransfer>(), 24);
            assert_eq!(USBDEVFS_BULK, 0xC018_5502);
            assert_eq!(USBDEVFS_CLAIMINTERFACE, 0x8004_550F);
            assert_eq!(USBDEVFS_RELEASEINTERFACE, 0x8004_5510);
        }

        #[test]
        fn finds_the_interface_of_an_endpoint() {
            let root = std::env::temp_dir().join(format!("tuxrgb-usb-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let dev = root.join("1-7");
            for (iface, ep) in [("1-7:1.0", "ep_81"), ("1-7:1.1", "ep_02")] {
                std::fs::create_dir_all(dev.join(iface).join(ep)).unwrap();
                std::fs::write(
                    dev.join(iface).join("bInterfaceNumber"),
                    format!("0{}\n", &iface[iface.len() - 1..]),
                )
                .unwrap();
            }
            assert_eq!(interface_of(&dev, 0x02), Some(1));
            assert_eq!(interface_of(&dev, 0x81), Some(0));
            assert_eq!(interface_of(&dev, 0x03), None);
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}

/// Records bulk writes (tests; pictures are Linux-only).
#[cfg(all(test, target_os = "linux"))]
pub struct MockBulk(pub std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>);

#[cfg(all(test, target_os = "linux"))]
impl BulkOut for MockBulk {
    fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.0.lock().unwrap().push(data.to_vec());
        Ok(())
    }
}
