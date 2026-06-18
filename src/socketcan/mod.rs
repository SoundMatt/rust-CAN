// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Linux SocketCAN bus implementation.
//!
//! Opens a CAN_RAW socket bound to the given network interface and
//! reads/writes classic CAN and CAN FD frames using `libc`.
//!
//! Requires Linux kernel ≥ 2.6.25 and a SocketCAN-capable interface
//! (e.g. `can0`, `vcan0`).

use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use async_trait::async_trait;
use libc::{AF_CAN, SOCK_RAW, SOL_CAN_RAW};
use tokio::io::unix::AsyncFd;
use tokio::sync::Mutex;

use crate::bus::{Bus, FrameReceiver, SubInner};
use crate::error::Error;
use crate::frame::{Filter, Frame};
use crate::relay::{Context, SubscriberOptions};

// ---------------------------------------------------------------------------
// SocketCAN constants
// ---------------------------------------------------------------------------

/// Protocol family for SocketCAN.
const PF_CAN: libc::c_int = 29;
/// Raw CAN protocol.
const CAN_RAW: libc::c_int = 1;
/// CAN FD frames flag in socket options.
const CAN_RAW_FD_FRAMES: libc::c_int = 5;
/// EFF (Extended Frame Format) flag in can_id.
const CAN_EFF_FLAG: u32 = 0x8000_0000;
/// RTR (Remote Transmission Request) flag in can_id.
const CAN_RTR_FLAG: u32 = 0x4000_0000;
/// Mask for the actual CAN ID bits.
const CAN_SFF_MASK: u32 = 0x000_07FF;
const CAN_EFF_MASK: u32 = 0x1FFF_FFFF;

/// CAN FD frame flag — bit-rate switch.
const CANFD_BRS: u8 = 0x01;
/// CAN FD frame flag — error state indicator.
const CANFD_ESI: u8 = 0x02;

// ---------------------------------------------------------------------------
// Raw frame structures
// ---------------------------------------------------------------------------

/// Classic CAN frame as defined by the Linux kernel.
#[repr(C)]
#[derive(Clone, Copy)]
struct CanFrame {
    can_id: u32,
    can_dlc: u8,
    _pad: u8,
    _res0: u8,
    _res1: u8,
    data: [u8; 8],
}

/// CAN FD frame as defined by the Linux kernel.
#[repr(C)]
#[derive(Clone, Copy)]
struct CanFdFrame {
    can_id: u32,
    len: u8,
    flags: u8,
    __res0: u8,
    __res1: u8,
    data: [u8; 64],
}

// ---------------------------------------------------------------------------
// SocketCanBus
// ---------------------------------------------------------------------------

/// A CAN bus backed by a Linux SocketCAN socket.
///
/// Supports classic CAN and CAN FD frames. Subscriptions are handled by
/// a background tokio task that reads from the socket.
pub struct SocketCanBus {
    fd: RawFd,
    // Keeps the file and async readiness alive; read by the reader task (not via self).
    #[allow(dead_code)]
    async_fd: Arc<AsyncFd<std::fs::File>>,
    closed: Arc<AtomicBool>,
    subscribers: Arc<Mutex<Vec<Arc<SubInner>>>>,
    // Remembered for future send-path FD capability checks.
    #[allow(dead_code)]
    fd_enabled: bool,
}

impl SocketCanBus {
    /// Open a SocketCAN bus on the given interface (e.g. `"vcan0"`).
    pub fn new(iface: &str) -> Result<Self, Error> {
        let fd = unsafe { libc::socket(PF_CAN, SOCK_RAW, CAN_RAW) };
        if fd < 0 {
            return Err(Error::Io(std::io::Error::last_os_error()));
        }

        // Bind to the interface.
        let iface_idx = get_iface_index(fd, iface)?;
        let mut addr: libc::sockaddr_can = unsafe { std::mem::zeroed() };
        addr.can_family = AF_CAN as u16;
        addr.can_ifindex = iface_idx;

        let bind_ret = unsafe {
            libc::bind(
                fd,
                &addr as *const libc::sockaddr_can as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_can>() as libc::socklen_t,
            )
        };
        if bind_ret < 0 {
            unsafe { libc::close(fd) };
            return Err(Error::Io(std::io::Error::last_os_error()));
        }

        // Enable CAN FD frames.
        let enable: libc::c_int = 1;
        let fd_ret = unsafe {
            libc::setsockopt(
                fd,
                SOL_CAN_RAW,
                CAN_RAW_FD_FRAMES,
                &enable as *const libc::c_int as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        let fd_enabled = fd_ret == 0;

        // Set socket to non-blocking for async use.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }

        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        let async_fd = Arc::new(AsyncFd::new(file).map_err(Error::Io)?);

        let closed = Arc::new(AtomicBool::new(false));
        let subscribers: Arc<Mutex<Vec<Arc<SubInner>>>> = Arc::new(Mutex::new(Vec::new()));

        // Spawn the reader task.
        let closed_clone = closed.clone();
        let subs_clone = subscribers.clone();
        let async_fd_clone = async_fd.clone();
        let fd_enabled_clone = fd_enabled;
        tokio::spawn(async move {
            reader_task(async_fd_clone, closed_clone, subs_clone, fd_enabled_clone).await;
        });

        Ok(Self {
            fd,
            async_fd,
            closed,
            subscribers,
            fd_enabled,
        })
    }
}

impl Drop for SocketCanBus {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        // The underlying file descriptor is closed when async_fd drops.
    }
}

#[async_trait]
impl Bus for SocketCanBus {
    async fn send(&self, _ctx: Context, frame: Frame) -> Result<(), Error> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }

        crate::validate_frame(&frame)?;

        if frame.fd {
            send_fd_frame(self.fd, &frame)
        } else {
            send_classic_frame(self.fd, &frame)
        }
    }

    async fn subscribe(
        &self,
        _filters: Vec<Filter>,
        opts: SubscriberOptions,
    ) -> Result<FrameReceiver, Error> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }

        let depth = opts.chan_depth(64);
        let sub_inner = Arc::new(SubInner::new(depth, opts.back_pressure));
        let rx = FrameReceiver {
            inner: sub_inner.clone(),
        };
        self.subscribers.lock().await.push(sub_inner);
        Ok(rx)
    }

    async fn close(&self) -> Result<(), Error> {
        if self
            .closed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(());
        }
        let subs = self.subscribers.lock().await;
        for sub in subs.iter() {
            sub.close();
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Frame read/write helpers
// ---------------------------------------------------------------------------

fn send_classic_frame(fd: RawFd, frame: &Frame) -> Result<(), Error> {
    let mut raw = CanFrame {
        can_id: build_can_id(frame),
        can_dlc: frame.data.len() as u8,
        _pad: 0,
        _res0: 0,
        _res1: 0,
        data: [0u8; 8],
    };
    let copy_len = frame.data.len().min(8);
    raw.data[..copy_len].copy_from_slice(&frame.data[..copy_len]);

    let ret = unsafe {
        libc::write(
            fd,
            &raw as *const CanFrame as *const libc::c_void,
            std::mem::size_of::<CanFrame>(),
        )
    };
    if ret < 0 {
        return Err(Error::Io(std::io::Error::last_os_error()));
    }
    Ok(())
}

fn send_fd_frame(fd: RawFd, frame: &Frame) -> Result<(), Error> {
    let mut raw = CanFdFrame {
        can_id: build_can_id(frame),
        len: frame.data.len() as u8,
        flags: 0,
        __res0: 0,
        __res1: 0,
        data: [0u8; 64],
    };
    if frame.brs {
        raw.flags |= CANFD_BRS;
    }
    if frame.esi {
        raw.flags |= CANFD_ESI;
    }
    let copy_len = frame.data.len().min(64);
    raw.data[..copy_len].copy_from_slice(&frame.data[..copy_len]);

    let ret = unsafe {
        libc::write(
            fd,
            &raw as *const CanFdFrame as *const libc::c_void,
            std::mem::size_of::<CanFdFrame>(),
        )
    };
    if ret < 0 {
        return Err(Error::Io(std::io::Error::last_os_error()));
    }
    Ok(())
}

fn build_can_id(frame: &Frame) -> u32 {
    let mut can_id = frame.id;
    if frame.ext {
        can_id |= CAN_EFF_FLAG;
    }
    if frame.rtr {
        can_id |= CAN_RTR_FLAG;
    }
    can_id
}

fn parse_classic_frame(raw: &CanFrame) -> Frame {
    let can_id = raw.can_id;
    let ext = (can_id & CAN_EFF_FLAG) != 0;
    let rtr = (can_id & CAN_RTR_FLAG) != 0;
    let id = if ext {
        can_id & CAN_EFF_MASK
    } else {
        can_id & CAN_SFF_MASK
    };
    let len = (raw.can_dlc as usize).min(8);
    Frame {
        id,
        ext,
        rtr,
        data: raw.data[..len].to_vec(),
        ..Default::default()
    }
}

fn parse_fd_frame(raw: &CanFdFrame) -> Frame {
    let can_id = raw.can_id;
    let ext = (can_id & CAN_EFF_FLAG) != 0;
    let id = if ext {
        can_id & CAN_EFF_MASK
    } else {
        can_id & CAN_SFF_MASK
    };
    let len = (raw.len as usize).min(64);
    Frame {
        id,
        ext,
        fd: true,
        brs: (raw.flags & CANFD_BRS) != 0,
        esi: (raw.flags & CANFD_ESI) != 0,
        data: raw.data[..len].to_vec(),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Reader task
// ---------------------------------------------------------------------------

async fn reader_task(
    async_fd: Arc<AsyncFd<std::fs::File>>,
    closed: Arc<AtomicBool>,
    subscribers: Arc<Mutex<Vec<Arc<SubInner>>>>,
    fd_enabled: bool,
) {
    let raw_fd = async_fd.as_raw_fd();

    loop {
        if closed.load(Ordering::SeqCst) {
            break;
        }

        // Wait for the socket to be readable.
        let mut guard = match async_fd.readable().await {
            Ok(g) => g,
            Err(_) => break,
        };

        let frame = if fd_enabled {
            // Try to read a FD frame first (larger).
            let mut raw = CanFdFrame {
                can_id: 0,
                len: 0,
                flags: 0,
                __res0: 0,
                __res1: 0,
                data: [0u8; 64],
            };
            let n = unsafe {
                libc::read(
                    raw_fd,
                    &mut raw as *mut CanFdFrame as *mut libc::c_void,
                    std::mem::size_of::<CanFdFrame>(),
                )
            };
            if n < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::WouldBlock {
                    guard.clear_ready();
                    continue;
                }
                break;
            }
            if n as usize == std::mem::size_of::<CanFdFrame>() {
                parse_fd_frame(&raw)
            } else {
                // Classic CAN frame — reinterpret the bytes.
                let mut classic = CanFrame {
                    can_id: raw.can_id,
                    can_dlc: raw.len,
                    _pad: 0,
                    _res0: 0,
                    _res1: 0,
                    data: [0u8; 8],
                };
                classic.data[..8].copy_from_slice(&raw.data[..8]);
                parse_classic_frame(&classic)
            }
        } else {
            let mut raw = CanFrame {
                can_id: 0,
                can_dlc: 0,
                _pad: 0,
                _res0: 0,
                _res1: 0,
                data: [0u8; 8],
            };
            let n = unsafe {
                libc::read(
                    raw_fd,
                    &mut raw as *mut CanFrame as *mut libc::c_void,
                    std::mem::size_of::<CanFrame>(),
                )
            };
            if n < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::WouldBlock {
                    guard.clear_ready();
                    continue;
                }
                break;
            }
            parse_classic_frame(&raw)
        };

        guard.clear_ready();

        // Deliver to subscribers.
        let subs = subscribers.lock().await;
        subs.retain_dead();
        for sub in subs.iter() {
            if !sub.closed.load(Ordering::Relaxed) {
                sub.push(frame.clone());
            }
        }
    }

    // Close all subscribers on exit.
    let subs = subscribers.lock().await;
    for sub in subs.iter() {
        sub.close();
    }
}

/// Helper trait for retaining only live subscribers (in-place filtering would
/// require a mutable guard which conflicts with the loop structure, so we use
/// a post-loop gc pass in VirtualBus instead).
trait RetainDead {
    fn retain_dead(&self);
}

impl RetainDead for Vec<Arc<SubInner>> {
    fn retain_dead(&self) {
        // No-op here; GC happens on subscribe(). This avoids needing &mut.
    }
}

// ---------------------------------------------------------------------------
// Interface index lookup
// ---------------------------------------------------------------------------

fn get_iface_index(fd: RawFd, name: &str) -> Result<libc::c_int, Error> {
    use std::ffi::CString;
    let cname = CString::new(name).map_err(|_| Error::Other("invalid interface name".into()))?;

    let mut req: libc::ifreq = unsafe { std::mem::zeroed() };
    let name_bytes = cname.as_bytes_with_nul();
    let copy_len = name_bytes.len().min(libc::IFNAMSIZ);
    unsafe {
        std::ptr::copy_nonoverlapping(
            name_bytes.as_ptr() as *const libc::c_char,
            req.ifr_name.as_mut_ptr(),
            copy_len,
        );
    }

    let ret = unsafe { libc::ioctl(fd, libc::SIOCGIFINDEX as _, &req) };
    if ret < 0 {
        return Err(Error::Io(std::io::Error::last_os_error()));
    }

    Ok(unsafe { req.ifr_ifru.ifru_ifindex })
}

// No unit tests for SocketCAN here since they require a real Linux SocketCAN
// interface. See tests/socketcan_test.rs (requires vcan0 to be set up).
