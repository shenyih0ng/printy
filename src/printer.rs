use hexyl;
use rusb::{Context, DeviceHandle, Direction, TransferType, UsbContext};
use std::{
    fmt,
    io::{self},
    thread::sleep,
    time::Duration,
};

use crate::escpos::{
    CMD_BOLD, CMD_CHAR_SIZE, CMD_CUT, CMD_DISABLE_ASB, CMD_INIT, CMD_PROC_DELAY_MS, CMD_RT_STATUS,
    CMD_UNDERLINE, PrinterStatus, RtStatusReq,
};

use markdown::{mdast, to_mdast};

#[derive(Debug)]
pub enum DriverKind {
    Debug,
    Usb,
}

#[derive(Debug)]
pub enum PrintyError {
    Driver {
        kind: DriverKind,
        context: String,
        source: Option<Box<dyn std::error::Error>>,
    },
    Parse {
        context: String,
        source: Option<Box<dyn std::error::Error>>,
    },
}

impl fmt::Display for PrintyError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PrintyError::Driver {
                kind,
                context,
                source,
            } => {
                write!(
                    f,
                    "Driver ({}) error: {}",
                    match kind {
                        DriverKind::Debug => "Debug",
                        DriverKind::Usb => "USB",
                    },
                    context
                )?;

                if let Some(source_err) = source {
                    write!(f, " - {}", source_err)?;
                }

                Ok(())
            }
            PrintyError::Parse { context, source } => {
                write!(f, "Parse error: {}", context)?;
                if let Some(source_err) = source {
                    write!(f, " - {}", source_err)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for PrintyError {}

pub type PrintyResult<T> = Result<T, PrintyError>;

pub trait Driver {
    fn read(&mut self, buf: &mut [u8]) -> PrintyResult<usize>;

    fn write(&mut self, data: &[u8]) -> PrintyResult<usize>;

    fn drain(&mut self) -> PrintyResult<()>;
}

#[derive(Default)]
pub struct DebugDriver {
    write_count: usize,
    read_count: usize,
}

impl Driver for DebugDriver {
    fn read(&mut self, buf: &mut [u8]) -> PrintyResult<usize> {
        println!("P <- [{}]:", self.read_count);

        let mut input = String::new();
        io::stdin().read_line(&mut input).ok();

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
                self.read_count += 1;
                let bytes_to_copy = values.len().min(buf.len());
                buf[..bytes_to_copy].copy_from_slice(&values[..bytes_to_copy]);
                Ok(bytes_to_copy)
            }
            Err(_) => Err(PrintyError::Driver {
                kind: DriverKind::Debug,
                context: "Invalid hex format. Use format like: '0x41', '0x42' or '41', '42'"
                    .to_string(),
                source: None,
            }),
        }
    }

    fn write(&mut self, data: &[u8]) -> PrintyResult<usize> {
        println!("P -> [{}]:", self.write_count);

        let mut handle = io::stdout().lock();
        let mut hex_printer = hexyl::PrinterBuilder::new(&mut handle).build();
        hex_printer.print_all(data).unwrap();

        self.write_count += 1;
        Ok(data.len())
    }

    fn drain(&mut self) -> PrintyResult<()> {
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
    pub fn new(vid: u16, pid: u16) -> PrintyResult<Self> {
        let usb_ctx = Context::new().unwrap();
        let usb_devs = usb_ctx.devices().unwrap();

        let print_dev = usb_devs
            .iter()
            .find(|dev| match dev.device_descriptor() {
                Ok(dev_desc) => (dev_desc.vendor_id(), dev_desc.product_id()) == (vid, pid),
                _ => false,
            })
            .ok_or(PrintyError::Driver {
                kind: DriverKind::Usb,
                context: format!("Device (vid={vid:#04x}, pid={pid:#04x}) not found"),
                source: None,
            })?;

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
            .ok_or(PrintyError::Driver {
                kind: DriverKind::Usb,
                context: format!(
                    "No suitable bulk endpoints found for device with VID: {vid:#04x}, PID: {pid:#04x}"
                ),
                source: None
            })?;

        let print_dev_handle = print_dev.open().expect("Failed to open device!");
        print_dev_handle
            .claim_interface(if_num)
            .map_err(|e| PrintyError::Driver {
                kind: DriverKind::Usb,
                context: format!("Failed to claim USB interface {}", if_num),
                source: Some(Box::new(e)),
            })?;

        Ok(Self {
            dev: print_dev_handle,
            in_ept_addr,
            out_ept_addr,
            // NOTE: For now, default timeout seems sufficient, unless we need to allow user to configure it in the future
            io_timeout: Duration::from_secs(5),
        })
    }
}

impl UsbDriver {
    fn _io_with_retry<F, T>(&self, ept_addr: u8, mut io_func: F) -> PrintyResult<T>
    where
        F: FnMut() -> rusb::Result<T>,
    {
        io_func().or_else(|e: rusb::Error| match e {
            rusb::Error::Pipe => {
                self.dev
                    .clear_halt(ept_addr)
                    .map_err(|e| PrintyError::Driver {
                        kind: DriverKind::Usb,
                        context: format!("Failed to clear halt on endpoint {ept_addr:#04x}"),
                        source: Some(Box::new(e)),
                    })?;
                sleep(Duration::from_millis(CMD_PROC_DELAY_MS));
                io_func().map_err(|e| PrintyError::Driver {
                    kind: DriverKind::Usb,
                    context: format!("Failed to retry I/O operation on endpoint {ept_addr:#04x}"),
                    source: Some(Box::new(e)),
                })
            }
            _ => Err(PrintyError::Driver {
                kind: DriverKind::Usb,
                context: format!("I/O error on endpoint {ept_addr:#04x}: {}", e),
                source: Some(Box::new(e)),
            }),
        })
    }
}

impl Driver for UsbDriver {
    fn read(&mut self, buf: &mut [u8]) -> PrintyResult<usize> {
        self._io_with_retry(self.in_ept_addr, || {
            self.dev.read_bulk(self.in_ept_addr, buf, self.io_timeout)
        })
    }

    fn write(&mut self, data: &[u8]) -> PrintyResult<usize> {
        // TODO: chunk the payload if it exceeds the receive buffer size
        match self._io_with_retry(self.out_ept_addr, || {
            self.dev
                .write_bulk(self.out_ept_addr, data, self.io_timeout)
        })? {
            w_len if w_len == data.len() => Ok(w_len),
            w_len => Err(PrintyError::Driver {
                kind: DriverKind::Usb,
                context: format!(
                    "Partial write: expected {}, got {} - data: {:02x?}",
                    data.len(),
                    w_len,
                    &data[..w_len]
                ),
                source: None,
            }),
        }
    }

    fn drain(&mut self) -> PrintyResult<()> {
        let mut _buf = [0u8; 16];
        while self.read(&mut _buf)? != 0 {}
        Ok(())
    }
}

pub struct Printer<D> {
    pub driver: D,
}

impl Printer<Box<dyn Driver>> {
    pub fn usb(vid: u16, pid: u16) -> PrintyResult<Self> {
        Self::new(Box::new(UsbDriver::new(vid, pid)?))
    }

    pub fn debug() -> PrintyResult<Self> {
        Self::new(Box::new(DebugDriver::default()))
    }

    pub fn new(driver: Box<dyn Driver>) -> PrintyResult<Self> {
        let mut printer = Printer { driver };
        printer.init()?;
        Ok(printer)
    }

    fn init(&mut self) -> PrintyResult<&Self> {
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
        Ok(self)
    }

    pub fn status(&mut self) -> Option<PrinterStatus> {
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

    pub fn cut(&mut self) -> PrintyResult<&mut Self> {
        self.driver.write(CMD_CUT)?;
        Ok(self)
    }

    pub fn print(&mut self, data: &str) -> PrintyResult<&mut Self> {
        self.driver.write(data.as_bytes())?;
        Ok(self)
    }

    pub fn print_md(&mut self, data: &str) -> PrintyResult<&mut Self> {
        self.driver.write(&EscposMarkdown.compile(data)?)?;
        Ok(self)
    }
}

struct EscposMarkdown;

impl EscposMarkdown {
    pub fn compile(&self, md_str: &str) -> PrintyResult<Vec<u8>> {
        let md_root_node = to_mdast(md_str, &markdown::ParseOptions::default()).map_err(|e| {
            PrintyError::Parse {
                context: format!("Failed to parse markdown - {e}"),
                source: None,
            }
        })?;

        let mut compiled_cmds = Vec::<u8>::new();
        self.compile_node(&md_root_node, &mut compiled_cmds);
        Ok(compiled_cmds)
    }

    fn compile_node(&self, node: &mdast::Node, buf: &mut Vec<u8>) {
        match node {
            mdast::Node::Root(root) => root
                .children
                .iter()
                .for_each(|child| self.compile_node(child, buf)),
            mdast::Node::Paragraph(para) => {
                para.children
                    .iter()
                    .for_each(|child| self.compile_node(child, buf));
                buf.extend_from_slice(b"\n\n");
            }
            mdast::Node::Heading(header) => {
                let (style_cmds, reset_cmds) = match header.depth {
                    1 => (CMD_CHAR_SIZE(1, 0).to_vec(), CMD_CHAR_SIZE(0, 0).to_vec()),
                    2 => (
                        [CMD_UNDERLINE(true), CMD_BOLD(true)].concat(),
                        [CMD_UNDERLINE(false), CMD_BOLD(false)].concat(),
                    ),
                    3 => (CMD_BOLD(true).to_vec(), CMD_BOLD(false).to_vec()),
                    _ => (vec![], vec![]),
                };
                buf.extend_from_slice(&style_cmds);
                header
                    .children
                    .iter()
                    .for_each(|child| self.compile_node(child, buf));
                buf.extend_from_slice(&reset_cmds);
                buf.extend_from_slice(b"\n\n");
            }
            mdast::Node::Text(text) => buf.extend(text.value.as_bytes()),
            mdast::Node::Strong(bold) => {
                buf.extend(CMD_BOLD(true));
                bold.children
                    .iter()
                    .for_each(|child| self.compile_node(child, buf));
                buf.extend(CMD_BOLD(false));
            }
            _ => {}
        }
    }
}
