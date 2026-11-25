//! USB HID access for the hardware displays, without hidapi or libusb:
//!
//! * Linux – `/dev/hidrawN` nodes, found through sysfs (the udev rules
//!   TuxRGB installs give the logged-in user access);
//! * Windows – the HID class driver (SetupAPI + `hid.dll`), overlapped I/O
//!   so nothing ever waits forever.
//!
//! Reports are written exactly like hidapi does (first byte: the report
//! number, or the first data byte for devices without numbered reports),
//! so the byte sequences of the reference projects apply unchanged.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

/// A HID interface of a USB device.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HidInfo {
    /// Device node (`/dev/hidraw3`) or Windows device interface path.
    pub path: String,
    pub vendor_id: u16,
    pub product_id: u16,
    /// USB interface number, when known.
    pub interface: Option<u8>,
    /// USB product string.
    pub product: String,
    pub serial: String,
    /// Where it is plugged (USB port path): stable across reboots.
    pub location: String,
    /// Linux: the USB device in sysfs (bulk transfers of the Kraken).
    pub usb_sysfs: Option<PathBuf>,
}

impl HidInfo {
    /// Stable identifier of the display (settings are saved under it).
    pub fn key(&self) -> String {
        let id = if !self.serial.trim().is_empty() {
            self.serial.trim().to_owned()
        } else {
            self.location.clone()
        };
        format!("{:04x}:{:04x}:{}", self.vendor_id, self.product_id, id)
    }
}

/// Reports to and from a device.
pub trait Transport: Send {
    /// One output report (first byte: report number, as with hidapi).
    fn write(&mut self, report: &[u8]) -> io::Result<()>;
    /// One input report into `buf`; `Ok(0)` when nothing came within
    /// `timeout`.
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize>;
    /// Drops the input reports already queued.
    fn flush_input(&mut self) {
        let mut buf = [0u8; 1100];
        for _ in 0..64 {
            if !matches!(self.read(&mut buf, Duration::ZERO), Ok(n) if n > 0) {
                break;
            }
        }
    }
}

/// Every HID interface of the machine.
pub fn enumerate() -> Vec<HidInfo> {
    #[cfg(target_os = "linux")]
    {
        linux::enumerate_in(std::path::Path::new("/sys"), std::path::Path::new("/dev"))
    }
    #[cfg(windows)]
    {
        windows_hid::enumerate()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Vec::new()
    }
}

/// Opens a HID interface for reading and writing.
pub fn open(info: &HidInfo) -> io::Result<Box<dyn Transport>> {
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(linux::Hidraw::open(&info.path)?))
    }
    #[cfg(windows)]
    {
        Ok(Box::new(windows_hid::WinHid::open(&info.path)?))
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = info;
        Err(io::Error::new(io::ErrorKind::Unsupported, "HID"))
    }
}

#[cfg(target_os = "linux")]
pub mod linux {
    use super::HidInfo;
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    fn read_trimmed(p: &Path) -> Option<String> {
        std::fs::read_to_string(p)
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    }

    /// `HID_ID=0003:00003633:00000002` → (bus, vendor, product).
    fn parse_hid_id(value: &str) -> Option<(u16, u16, u16)> {
        let mut it = value.split(':');
        let bus = u16::from_str_radix(it.next()?, 16).ok()?;
        let vid = u32::from_str_radix(it.next()?, 16).ok()?;
        let pid = u32::from_str_radix(it.next()?, 16).ok()?;
        Some((bus, vid as u16, pid as u16))
    }

    /// HID interfaces under `sys` (normally `/sys`), with their nodes in
    /// `dev` (normally `/dev`).
    pub fn enumerate_in(sys: &Path, dev: &Path) -> Vec<HidInfo> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(sys.join("class/hidraw")) else {
            return out;
        };
        let mut names: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        names.sort();
        for entry in names {
            let Some(name) = entry.file_name().map(|n| n.to_owned()) else {
                continue;
            };
            let device = entry.join("device");
            let Some(uevent) = read_trimmed(&device.join("uevent")) else {
                continue;
            };
            let field = |key: &str| {
                uevent
                    .lines()
                    .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
                    .map(str::to_owned)
            };
            let Some((_bus, vendor_id, product_id)) =
                field("HID_ID").as_deref().and_then(parse_hid_id)
            else {
                continue;
            };
            // hidraw → HID device → USB interface → USB device.
            let real = std::fs::canonicalize(&device).unwrap_or(device);
            let iface = real.parent().map(Path::to_path_buf);
            let interface = iface
                .as_deref()
                .and_then(|i| read_trimmed(&i.join("bInterfaceNumber")))
                .and_then(|n| u8::from_str_radix(&n, 16).ok())
                .or_else(|| {
                    // HID_PHYS=usb-0000:00:14.0-3/input1
                    field("HID_PHYS")?.rsplit_once("/input")?.1.parse().ok()
                });
            let usb = iface
                .as_deref()
                .and_then(Path::parent)
                .filter(|u| u.join("idVendor").exists())
                .map(Path::to_path_buf);
            let product = usb
                .as_deref()
                .and_then(|u| read_trimmed(&u.join("product")))
                .or_else(|| field("HID_NAME"))
                .unwrap_or_default();
            let serial = usb
                .as_deref()
                .and_then(|u| read_trimmed(&u.join("serial")))
                .or_else(|| field("HID_UNIQ"))
                .unwrap_or_default();
            let location = usb
                .as_deref()
                .and_then(|u| u.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .or_else(|| {
                    field("HID_PHYS").map(|p| {
                        p.rsplit_once("/input")
                            .map(|(a, _)| a.to_owned())
                            .unwrap_or(p)
                    })
                })
                .unwrap_or_default();
            out.push(HidInfo {
                path: dev.join(&name).to_string_lossy().into_owned(),
                vendor_id,
                product_id,
                interface,
                product,
                serial,
                location,
                usb_sysfs: usb,
            });
        }
        out
    }

    /// An open `/dev/hidrawN`.
    pub struct Hidraw {
        file: File,
    }

    impl Hidraw {
        pub fn open(path: &str) -> io::Result<Self> {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(path)?;
            Ok(Self { file })
        }
    }

    /// Waits until `fd` is ready (`events`), at most `timeout`.
    pub fn wait(fd: i32, events: i16, timeout: Duration) -> io::Result<bool> {
        let mut pfd = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        loop {
            // SAFETY: one valid pollfd.
            let r = unsafe { libc::poll(&mut pfd, 1, ms) };
            if r < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            if r > 0 && pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "device disconnected",
                ));
            }
            return Ok(r > 0);
        }
    }

    impl super::Transport for Hidraw {
        fn write(&mut self, report: &[u8]) -> io::Result<()> {
            // A report is one write() (the kernel sends it whole).
            for _ in 0..50 {
                match self.file.write(report) {
                    Ok(_) => return Ok(()),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        wait(
                            self.file.as_raw_fd(),
                            libc::POLLOUT,
                            Duration::from_millis(20),
                        )?;
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "HID write timed out",
            ))
        }

        fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
            if !wait(self.file.as_raw_fd(), libc::POLLIN, timeout)? {
                return Ok(0);
            }
            match self.file.read(buf) {
                Ok(n) => Ok(n),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(0),
                Err(e) => Err(e),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn finds_displays_in_a_fake_sysfs() {
            let root = std::env::temp_dir().join(format!("tuxrgb-hid-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            // A DeepCool AK620 behind a USB device, and a bare HID device.
            let usb = root.join("devices/pci0000:00/usb1/1-3");
            let hid = usb.join("1-3:1.0/0003:3633:0002.0004");
            std::fs::create_dir_all(&hid).unwrap();
            std::fs::write(usb.join("idVendor"), "3633\n").unwrap();
            std::fs::write(usb.join("product"), "AK620-DIGITAL\n").unwrap();
            std::fs::write(usb.join("serial"), "\n").unwrap();
            std::fs::write(usb.join("1-3:1.0/bInterfaceNumber"), "00\n").unwrap();
            std::fs::write(
                hid.join("uevent"),
                "DRIVER=hid-generic\nHID_ID=0003:00003633:00000002\nHID_NAME=DeepCool AK620-DIGITAL\nHID_PHYS=usb-0000:00:14.0-3/input0\nHID_UNIQ=\n",
            )
            .unwrap();
            let class = root.join("class/hidraw");
            std::fs::create_dir_all(class.join("hidraw0")).unwrap();
            std::os::unix::fs::symlink(&hid, class.join("hidraw0/device")).unwrap();
            let bare = class.join("hidraw1/device");
            std::fs::create_dir_all(&bare).unwrap();
            std::fs::write(
                bare.join("uevent"),
                "HID_ID=0003:00001E71:00003008\nHID_NAME=NZXT Kraken Z\nHID_PHYS=usb-0000:00:14.0-7/input1\nHID_UNIQ=ABC123\n",
            )
            .unwrap();
            let found = enumerate_in(&root, Path::new("/dev"));
            assert_eq!(found.len(), 2);
            let ak = &found[0];
            assert_eq!((ak.vendor_id, ak.product_id), (0x3633, 0x0002));
            assert_eq!(ak.path, "/dev/hidraw0");
            assert_eq!(ak.interface, Some(0));
            assert_eq!(ak.product, "AK620-DIGITAL");
            assert_eq!(ak.location, "1-3");
            assert_eq!(ak.key(), "3633:0002:1-3");
            assert!(ak.usb_sysfs.is_some());
            let kraken = &found[1];
            assert_eq!((kraken.vendor_id, kraken.product_id), (0x1e71, 0x3008));
            assert_eq!(kraken.interface, Some(1));
            assert_eq!(kraken.serial, "ABC123");
            assert_eq!(kraken.product, "NZXT Kraken Z");
            assert_eq!(kraken.location, "usb-0000:00:14.0-7");
            assert_eq!(kraken.key(), "1e71:3008:ABC123");
            let _ = std::fs::remove_dir_all(&root);
        }

        /// The real I/O path (open, poll, write, read) on a pseudo-terminal
        /// standing in for a hidraw node.
        #[test]
        fn reports_go_through_a_device_node() {
            use crate::displays::hid::Transport;
            // SAFETY: plain libc calls on descriptors owned by the test.
            let master = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
            assert!(master >= 0);
            unsafe {
                assert_eq!(libc::grantpt(master), 0);
                assert_eq!(libc::unlockpt(master), 0);
            }
            let mut name = [0 as libc::c_char; 128];
            assert_eq!(
                unsafe { libc::ptsname_r(master, name.as_mut_ptr(), name.len()) },
                0
            );
            let slave = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            // Raw mode on both sides: bytes as they are.
            let raw = |fd: i32| {
                let mut t: libc::termios = unsafe { std::mem::zeroed() };
                unsafe {
                    libc::tcgetattr(fd, &mut t);
                    libc::cfmakeraw(&mut t);
                    libc::tcsetattr(fd, libc::TCSANOW, &t);
                }
            };
            raw(master);
            let mut dev = Hidraw::open(&slave).unwrap();
            raw(dev.file.as_raw_fd());
            // Nothing to read yet: a timeout, not an error.
            let mut buf = [0u8; 64];
            assert_eq!(dev.read(&mut buf, Duration::from_millis(30)).unwrap(), 0);
            // Device → host.
            let reply = [0x31u8, 0x01, 50, 0];
            assert_eq!(
                unsafe { libc::write(master, reply.as_ptr().cast(), reply.len()) },
                4
            );
            let n = dev.read(&mut buf, Duration::from_millis(500)).unwrap();
            assert_eq!(&buf[..n], &reply);
            // Host → device.
            dev.write(&[16, 170, 0, 0]).unwrap();
            let mut got = [0u8; 8];
            assert!(wait(master, libc::POLLIN, Duration::from_millis(500)).unwrap());
            let n = unsafe { libc::read(master, got.as_mut_ptr().cast(), got.len()) };
            assert_eq!(&got[..n as usize], &[16, 170, 0, 0]);
            unsafe { libc::close(master) };
        }
    }
}

#[cfg(windows)]
pub mod windows_hid {
    use super::HidInfo;
    use std::io;
    use std::time::Duration;
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA,
        SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList,
        SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
    };
    use windows::Win32::Devices::HumanInterfaceDevice::{
        HIDD_ATTRIBUTES, HIDP_CAPS, HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetHidGuid,
        HidD_GetPreparsedData, HidD_GetProductString, HidD_GetSerialNumberString, HidP_GetCaps,
        PHIDP_PREPARSED_DATA,
    };
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_IO_PENDING, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile, WriteFile,
    };
    use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    use windows::core::PCWSTR;

    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn string_of(buf: &[u16]) -> String {
        let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    /// Device interface paths of every HID device present.
    fn paths() -> Vec<String> {
        let mut out = Vec::new();
        // SAFETY: SetupAPI calls with buffers sized as the API reports.
        unsafe {
            let guid = HidD_GetHidGuid();
            let Ok(set) = SetupDiGetClassDevsW(
                Some(&guid),
                PCWSTR::null(),
                None,
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            ) else {
                return out;
            };
            for index in 0..1024 {
                let mut data = SP_DEVICE_INTERFACE_DATA {
                    cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                    ..Default::default()
                };
                if SetupDiEnumDeviceInterfaces(set, None, &guid, index, &mut data).is_err() {
                    break;
                }
                let mut needed = 0u32;
                let _ =
                    SetupDiGetDeviceInterfaceDetailW(set, &data, None, 0, Some(&mut needed), None);
                if needed < 8 {
                    continue;
                }
                // u32-aligned buffer holding the variable-size structure.
                let mut buf = vec![0u32; (needed as usize).div_ceil(4)];
                let detail = buf.as_mut_ptr().cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
                (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
                if SetupDiGetDeviceInterfaceDetailW(set, &data, Some(detail), needed, None, None)
                    .is_ok()
                {
                    let path_ptr = std::ptr::addr_of!((*detail).DevicePath).cast::<u16>();
                    let max = (needed as usize - 4) / 2;
                    let slice = std::slice::from_raw_parts(path_ptr, max);
                    out.push(string_of(slice));
                }
            }
            let _ = SetupDiDestroyDeviceInfoList(set);
        }
        out
    }

    fn open_handle(path: &str, access: u32, overlapped: bool) -> io::Result<HANDLE> {
        let w = wide(path);
        // SAFETY: valid NUL-terminated path.
        unsafe {
            CreateFileW(
                PCWSTR(w.as_ptr()),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                if overlapped {
                    FILE_FLAG_OVERLAPPED
                } else {
                    FILE_FLAGS_AND_ATTRIBUTES(0)
                },
                None,
            )
        }
        .map_err(|e| io::Error::other(e.to_string()))
    }

    pub fn enumerate() -> Vec<HidInfo> {
        let mut out = Vec::new();
        for path in paths() {
            // Attributes need no access rights (keyboards and mice too).
            let Ok(h) = open_handle(&path, 0, false) else {
                continue;
            };
            // SAFETY: valid handle; buffers sized as given.
            unsafe {
                let mut attrs = HIDD_ATTRIBUTES {
                    Size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32,
                    ..Default::default()
                };
                if HidD_GetAttributes(h, &mut attrs).as_bool() {
                    let mut product = [0u16; 128];
                    let mut serial = [0u16; 128];
                    let _ = HidD_GetProductString(h, product.as_mut_ptr().cast(), 256);
                    let _ = HidD_GetSerialNumberString(h, serial.as_mut_ptr().cast(), 256);
                    let lower = path.to_ascii_lowercase();
                    let interface = lower
                        .find("&mi_")
                        .and_then(|i| lower.get(i + 4..i + 6))
                        .and_then(|n| u8::from_str_radix(n, 16).ok());
                    // \\?\hid#vid_3633&pid_0002#7&2a3f&0&0000#{guid}: the
                    // instance part identifies the port.
                    let location = lower.split('#').nth(2).unwrap_or_default().to_owned();
                    out.push(HidInfo {
                        path: path.clone(),
                        vendor_id: attrs.VendorID,
                        product_id: attrs.ProductID,
                        interface,
                        product: string_of(&product),
                        serial: string_of(&serial),
                        location,
                        usb_sysfs: None,
                    });
                }
                let _ = CloseHandle(h);
            }
        }
        out
    }

    /// An open HID device (overlapped I/O).
    pub struct WinHid {
        handle: HANDLE,
        output_len: usize,
        input_len: usize,
        event: HANDLE,
    }

    // SAFETY: the handles are only used from the thread owning the value.
    unsafe impl Send for WinHid {}

    impl WinHid {
        pub fn open(path: &str) -> io::Result<Self> {
            let handle = open_handle(path, GENERIC_READ | GENERIC_WRITE, true)?;
            // SAFETY: valid handle; the preparsed data is freed.
            let caps = unsafe {
                let mut pp = PHIDP_PREPARSED_DATA::default();
                let mut caps = HIDP_CAPS::default();
                if HidD_GetPreparsedData(handle, &mut pp).as_bool() {
                    let _ = HidP_GetCaps(pp, &mut caps);
                    let _ = HidD_FreePreparsedData(pp);
                }
                caps
            };
            // SAFETY: plain event creation.
            let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
                .map_err(|e| io::Error::other(e.to_string()))?;
            Ok(Self {
                handle,
                output_len: usize::from(caps.OutputReportByteLength),
                input_len: usize::from(caps.InputReportByteLength),
                event,
            })
        }

        /// Waits for an overlapped operation, cancelling it after `timeout`.
        fn finish(&self, ov: &mut OVERLAPPED, timeout: Duration) -> io::Result<Option<u32>> {
            let ms = timeout.as_millis().min(u32::MAX as u128) as u32;
            // SAFETY: the OVERLAPPED and its buffer outlive the operation
            // (cancelled and completed before returning).
            unsafe {
                let r = WaitForSingleObject(self.event, ms);
                if r == WAIT_TIMEOUT {
                    let _ = CancelIoEx(self.handle, Some(ov));
                    let mut n = 0;
                    let _ = GetOverlappedResult(self.handle, ov, &mut n, true);
                    return Ok(None);
                }
                if r != WAIT_OBJECT_0 {
                    return Err(io::Error::last_os_error());
                }
                let mut n = 0;
                GetOverlappedResult(self.handle, ov, &mut n, false)
                    .map_err(|e| io::Error::other(e.to_string()))?;
                Ok(Some(n))
            }
        }
    }

    impl super::Transport for WinHid {
        fn write(&mut self, report: &[u8]) -> io::Result<()> {
            // Windows wants exactly the report length (padded with zeros).
            let mut buf = report.to_vec();
            if buf.len() < self.output_len {
                buf.resize(self.output_len, 0);
            }
            let mut ov = OVERLAPPED {
                hEvent: self.event,
                ..Default::default()
            };
            // SAFETY: buffer and OVERLAPPED live until `finish` returns.
            let started = unsafe { WriteFile(self.handle, Some(&buf), None, Some(&mut ov)) };
            if let Err(e) = started
                && e.code() != ERROR_IO_PENDING.to_hresult()
            {
                return Err(io::Error::other(e.to_string()));
            }
            match self.finish(&mut ov, Duration::from_millis(1000))? {
                Some(_) => Ok(()),
                None => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "HID write timed out",
                )),
            }
        }

        fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
            let len = self.input_len.max(buf.len()).max(1);
            let mut tmp = vec![0u8; len];
            let mut ov = OVERLAPPED {
                hEvent: self.event,
                ..Default::default()
            };
            // SAFETY: buffer and OVERLAPPED live until `finish` returns.
            let started = unsafe { ReadFile(self.handle, Some(&mut tmp), None, Some(&mut ov)) };
            if let Err(e) = started
                && e.code() != ERROR_IO_PENDING.to_hresult()
            {
                return Err(io::Error::other(e.to_string()));
            }
            match self.finish(&mut ov, timeout)? {
                Some(n) => {
                    let n = (n as usize).min(tmp.len());
                    // Without numbered reports Windows prepends a zero.
                    let data = if self.input_len > 0 && tmp.first() == Some(&0) && n > 1 {
                        &tmp[1..n]
                    } else {
                        &tmp[..n]
                    };
                    let k = data.len().min(buf.len());
                    buf[..k].copy_from_slice(&data[..k]);
                    Ok(k)
                }
                None => Ok(0),
            }
        }
    }

    impl Drop for WinHid {
        fn drop(&mut self) {
            // SAFETY: handles owned by this value.
            unsafe {
                let _ = CloseHandle(self.event);
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

/// A scripted device for the tests: records what is written, answers with
/// a function of the request.
#[cfg(test)]
pub mod mock {
    use std::collections::VecDeque;
    use std::io;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    pub type Responder = Box<dyn FnMut(&[u8]) -> Vec<Vec<u8>> + Send>;

    #[derive(Clone, Default)]
    pub struct Log(pub Arc<Mutex<Vec<Vec<u8>>>>);

    impl Log {
        pub fn take(&self) -> Vec<Vec<u8>> {
            std::mem::take(&mut self.0.lock().unwrap())
        }
    }

    pub struct MockHid {
        pub log: Log,
        replies: VecDeque<Vec<u8>>,
        responder: Responder,
    }

    impl MockHid {
        pub fn new(responder: Responder) -> (Self, Log) {
            let log = Log::default();
            (
                Self {
                    log: log.clone(),
                    replies: VecDeque::new(),
                    responder,
                },
                log,
            )
        }

        pub fn silent() -> (Self, Log) {
            Self::new(Box::new(|_| Vec::new()))
        }
    }

    impl super::Transport for MockHid {
        fn write(&mut self, report: &[u8]) -> io::Result<()> {
            self.log.0.lock().unwrap().push(report.to_vec());
            let replies = (self.responder)(report);
            self.replies.extend(replies);
            Ok(())
        }

        fn read(&mut self, buf: &mut [u8], _timeout: Duration) -> io::Result<usize> {
            match self.replies.pop_front() {
                Some(r) => {
                    let n = r.len().min(buf.len());
                    buf[..n].copy_from_slice(&r[..n]);
                    Ok(n)
                }
                None => Ok(0),
            }
        }
    }
}
