use std::{
    io::{self, Error, Read, Write},
    time::Duration,
};

use rusb::{Context, DeviceHandle, Direction, TransferType, UsbContext};

use crate::escpos::{CMD_INIT, CMD_RT_STATUS, PrinterStatus, RtStatusReq};

pub trait Driver {
    fn read(&self, buf: &mut [u8]) -> io::Result<usize>;

    fn write(&self, data: &[u8]) -> io::Result<()>;
}

pub struct DebugDriver;

impl Driver for DebugDriver {
    fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        print!("Read (hex): ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        let hex_values: Result<Vec<u8>, _> = input
            .trim()
            .split_whitespace()
            .map(|s| {
                if s.starts_with("0x") || s.starts_with("0X") {
                    u8::from_str_radix(&s[2..], 16)
                } else {
                    u8::from_str_radix(s, 16)
                }
            })
            .collect();

        match hex_values {
            Ok(values) => {
                let bytes_to_copy = values.len().min(buf.len());
                buf[..bytes_to_copy].copy_from_slice(&values[..bytes_to_copy]);
                Ok(bytes_to_copy)
            }
            Err(_) => Err(Error::new(
                io::ErrorKind::InvalidData,
                "Invalid hex format. Use format like: 0x41 0x42 or 41 42",
            )),
        }
    }

    fn write(&self, data: &[u8]) -> io::Result<()> {
        println!(
            "Write: [{}]",
            data.iter()
                .map(|b| format!("0x{:02x}", b))
                .collect::<Vec<_>>()
                .join(", ")
        );
        Ok(())
    }
}

pub struct UsbDriver {
    dev: DeviceHandle<Context>,
    in_ept_addr: u8,
    out_ept_addr: u8,
    io_timeout: Duration,
}

impl UsbDriver {
    pub fn new(vid: u16, pid: u16) -> Self {
        let usb_ctx = Context::new().unwrap();
        let usb_devs = usb_ctx.devices().expect("Failed to get USB devices!");

        let print_dev = usb_devs
            .iter()
            .find(|dev| match dev.device_descriptor() {
                Ok(dev_desc) => (dev_desc.vendor_id(), dev_desc.product_id()) == (vid, pid),
                _ => false,
            })
            .expect(&format!("Device ({}, {}) not found!", vid, pid));

        let (in_ept_addr, out_ept_addr, if_num) = print_dev
            .active_config_descriptor()
            .unwrap()
            .interfaces()
            .flat_map(|inf| inf.descriptors())
            .flat_map(|if_desc| {
                let mut in_ept = None;
                let mut out_ept = None;
                for ept in if_desc.endpoint_descriptors() {
                    match (ept.direction(), ept.transfer_type()) {
                        (Direction::In, TransferType::Bulk) => {
                            in_ept = Some(ept.address());
                        }
                        (Direction::Out, TransferType::Bulk) => {
                            out_ept = Some(ept.address());
                        }
                        _ => {}
                    }
                }

                match (in_ept, out_ept) {
                    (Some(in_ept), Some(out_ept)) => {
                        Some((in_ept, out_ept, if_desc.interface_number()))
                    }
                    _ => None,
                }
            })
            .next()
            .expect("Failed to find bulk endpoints for the device!");

        let print_dev_handle = print_dev.open().expect("Failed to open device!");
        print_dev_handle
            .claim_interface(if_num)
            .expect("Failed to claim device USB interface!");

        Self {
            dev: print_dev_handle,
            in_ept_addr,
            out_ept_addr,
            // NOTE: For now, default timeout seems sufficient, unless we need to allow user to configure it in the future
            io_timeout: Duration::from_secs(5),
        }
    }
}

impl Driver for UsbDriver {
    fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        match self.dev.read_bulk(self.in_ept_addr, buf, self.io_timeout) {
            Ok(len) => Ok(len),
            Err(e) => Err(Error::new(io::ErrorKind::Other, e)),
        }
    }

    fn write(&self, data: &[u8]) -> io::Result<()> {
        match self
            .dev
            .write_bulk(self.out_ept_addr, data, self.io_timeout)
        {
            Ok(len) if len == data.len() => Ok(()),
            Ok(_) => Err(Error::new(io::ErrorKind::Other, "Incomplete write")),
            Err(e) => Err(Error::new(io::ErrorKind::Other, e)),
        }
    }
}

pub struct Printer<D> {
    pub driver: D,
}

impl Printer<UsbDriver> {
    pub fn usb(vid: u16, pid: u16) -> Self {
        Self::new(UsbDriver::new(vid, pid))
    }
}

impl Printer<DebugDriver> {
    pub fn debug() -> Self {
        Self::new(DebugDriver)
    }
}

impl<D: Driver> Printer<D> {
    pub fn new(driver: D) -> Self {
        let printer = Printer { driver };
        printer.init().unwrap();
        printer
    }

    fn init(&self) -> io::Result<()> {
        self.driver.write(CMD_INIT)
    }

    pub fn status(&self) -> Option<PrinterStatus> {
        let batched_status_cmds = [
            CMD_RT_STATUS(RtStatusReq::PrinterStatus),
            CMD_RT_STATUS(RtStatusReq::OfflineCause),
            CMD_RT_STATUS(RtStatusReq::ErrorCause),
            CMD_RT_STATUS(RtStatusReq::PaperStatus),
        ]
        .concat();
        self.driver.write(batched_status_cmds.as_slice()).unwrap();

        let mut buf = [0u8; 4];
        match self.driver.read(&mut buf) {
            Ok(len) if len == 4 => PrinterStatus::from_bytes(&buf),
            Ok(_) => {
                eprintln!("Received incomplete status response from printer.");
                None
            }
            Err(e) => {
                eprintln!("Failed to read printer status: {}", e);
                None
            }
        }
    }
}
