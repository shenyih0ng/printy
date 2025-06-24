use rusb::{Context, DeviceHandle, Direction, TransferType, UsbContext};
use std::{
    fmt,
    io::{self, Write},
    thread::sleep,
    time::Duration,
};

use crate::escpos::{
    CMD_DISABLE_ASB, CMD_INIT, CMD_PROC_DELAY_MS, CMD_RT_STATUS, PrinterStatus, RtStatusReq,
};

#[derive(Debug)]
pub enum PrinterError {
    Driver(String),
}

impl fmt::Display for PrinterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrinterError::Driver(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for PrinterError {}

impl From<io::Error> for PrinterError {
    fn from(err: io::Error) -> Self {
        PrinterError::Driver(format!("[I/O]: {}", err))
    }
}

impl From<rusb::Error> for PrinterError {
    fn from(err: rusb::Error) -> Self {
        PrinterError::Driver(format!("[USB]: {}", err))
    }
}

pub type PrinterResult<T> = Result<T, PrinterError>;

pub trait Driver {
    fn read(&self, buf: &mut [u8]) -> PrinterResult<usize>;

    fn write(&self, data: &[u8]) -> PrinterResult<()>;

    fn drain(&self) -> PrinterResult<()>;
}

pub struct DebugDriver;

impl Driver for DebugDriver {
    fn read(&self, buf: &mut [u8]) -> PrinterResult<usize> {
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
            Err(_) => Err(PrinterError::Driver(
                "Invalid hex format. Use format like: 0x41 0x42 or 41 42".to_owned(),
            )),
        }
    }

    fn write(&self, data: &[u8]) -> PrinterResult<()> {
        Ok(println!(
            "Write: [{}]",
            data.iter()
                .map(|b| format!("0x{:02x}", b))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }

    fn drain(&self) -> PrinterResult<()> {
        unimplemented!();
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

impl UsbDriver {
    fn _io_with_retry<F, T>(&self, ept_addr: u8, mut io_func: F) -> PrinterResult<T>
    where
        F: FnMut() -> rusb::Result<T>,
    {
        io_func().or_else(|e: rusb::Error| {
            if e == rusb::Error::Pipe {
                self.dev.clear_halt(ept_addr)?;
                sleep(Duration::from_millis(CMD_PROC_DELAY_MS));
                io_func().map_err(PrinterError::from)
            } else {
                Err(e.into())
            }
        })
    }
}

impl Driver for UsbDriver {
    fn read(&self, buf: &mut [u8]) -> PrinterResult<usize> {
        self._io_with_retry(self.in_ept_addr, || {
            self.dev.read_bulk(self.in_ept_addr, buf, self.io_timeout)
        })
    }

    fn write(&self, data: &[u8]) -> PrinterResult<()> {
        match self._io_with_retry(self.out_ept_addr, || {
            self.dev
                .write_bulk(self.out_ept_addr, data, self.io_timeout)
        })? {
            w_len if w_len == data.len() => Ok(()),
            w_len => Err(PrinterError::Driver(format!(
                "Partial write: expected {}, got {} - data: {:02x?}",
                data.len(),
                w_len,
                &data[..w_len]
            ))),
        }
    }

    fn drain(&self) -> PrinterResult<()> {
        let mut _buf = [0u8; 16];
        loop {
            let read_len = self.read(&mut _buf)?;
            if read_len == 0 {
                break;
            }
        }
        Ok(())
    }
}

pub struct Printer<D> {
    pub driver: D,
}

impl Printer<UsbDriver> {
    pub fn usb(vid: u16, pid: u16) -> PrinterResult<Self> {
        Self::new(UsbDriver::new(vid, pid))
    }
}

impl Printer<DebugDriver> {
    pub fn debug() -> PrinterResult<Self> {
        Self::new(DebugDriver)
    }
}

impl<D: Driver> Printer<D> {
    pub fn new(driver: D) -> PrinterResult<Self> {
        let printer = Printer { driver };
        printer.init()?;
        Ok(printer)
    }

    fn init(&self) -> PrinterResult<()> {
        /*
          The printer (`TM-T88IV`) seems to transmit a 7-byte long data sequence (via the BULK endpoint)
          upon powering on. This is not explicitly documented in the manual.

          The sequence is as follows: [0x3B, 0x31, 0x0, 0x14, 0x0, 0x0, 0x0F]
            - The first three bytes [0x3B, 0x31, 0x0] corresponds the closest to the transmitted
              response of a power-off command (`DLE DC4 (fn=2)`).
              - However, the identifier (`0x31`) does not match the expected value (`0x30`) according
                to the specification.
              - `TM-T88IV` does NOT support the power-off command (according to the `ESC/POS` manual),
                which could a possible explanation for the mismatch.
            - The remaining four bytes is the mysterious part, which seems to be a mixture of message
              terminators and undocumented start-up/initialization sequences.

          If the printer is powered on with an OFFLINE state (e.g. cover is open, paper is out), there
          will be additional 4 bytes of `ASB` (Automatic Status Back) message (which seems to also
          suggest that `ASB` is enabled by default).

          As such, we will drain these initial bytes to avoid any issues with message backlogging
          (transmitted data is only cleared after host reads it).
        */
        self.driver.drain()?;
        self.driver.write(CMD_INIT)?;
        // NOTE: Only works (reliably) if the printer (`TM-T88IV`) is powered on with an ONLINE state
        // Else, `ASB` sequences will still be transmitted
        self.driver.write(CMD_DISABLE_ASB)?;
        Ok(())
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

        sleep(Duration::from_millis(CMD_PROC_DELAY_MS));

        let mut buf = [0u8; 4];
        match self.driver.read(&mut buf) {
            Ok(len) if len == buf.len() => PrinterStatus::from_bytes(&buf),
            _ => None,
        }
    }
}
