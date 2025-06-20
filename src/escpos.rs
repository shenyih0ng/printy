use std::fmt::Display;

macro_rules! def_cmd {
    ($fn_name:ident, $header:expr, $( $param_name:ident : $param_type:ty ),+) => {
        #[allow(non_snake_case)]
        pub(crate) fn $fn_name($( $param_name: $param_type ),+) -> Vec<u8> {
            let mut command = $header.to_vec();
            $( command.push($param_name as u8); )+
            command
        }
    };
}

const ESC: u8 = 0x1B;
const DLE: u8 = 0x10;
const EOT: u8 = 0x04;
const GS: u8 = 0x1D;

pub(crate) const CMD_INIT: &[u8] = &[ESC, b'@'];

const _CMD_RT_STATUS: &[u8] = &[DLE, EOT];
pub(crate) enum RtStatusReq {
    PrinterStatus = 1,
    OfflineCause = 2,
    ErrorCause = 3,
    PaperStatus = 4,
}
def_cmd!(CMD_RT_STATUS, _CMD_RT_STATUS, req: RtStatusReq);

#[derive(Debug)]
pub(crate) struct PrinterError {
    is_cutter_error: bool,
    is_unrecoverable_error: bool,
    is_auto_recoverable_error: bool,
}

#[derive(Debug)]
pub(crate) struct OfflineCause {
    is_cover_open: bool,
    is_paper_empty: bool,
    error: Option<PrinterError>,
}

#[derive(Debug)]
pub(crate) enum PaperStatus {
    Adequate,
    NearEnd,
    NotPresent,
}

#[derive(Debug)]
pub(crate) struct PrinterStatus {
    is_online: bool,
    offline_cause: Option<OfflineCause>,
    paper_status: PaperStatus,
}

impl PrinterStatus {
    pub(crate) fn from_bytes(bytes: &[u8; 4]) -> Option<Self> {
        if !bytes.iter().all(|&b| (b & 0b10010011) == 0b00010010) {
            // If the status bytes do not match the expected format, stop parsing and treat
            // it as an indeterminate status
            return None;
        }
        let [printer_status, offline_cause, error_cause, paper_status] = bytes;

        let is_online = (printer_status & 0b1000) == 0;
        // NOTE: Assume that end sensor and near-end sensor has a relationship where if
        // the end sensor detects NO paper, values do not matter for the near-end sensor.
        // On the other hand, we assume that if there is paper present (detected by the end sensor),
        // we only care about the value of the near-end sensor
        let paper_status = {
            let is_present = (paper_status & 0b1100000) == 0;
            let is_near_end = (paper_status & 0b1100) != 0;
            match (is_present, is_near_end) {
                (true, true) => PaperStatus::NearEnd,
                (true, false) => PaperStatus::Adequate,
                (false, _) => PaperStatus::NotPresent,
            }
        };

        if is_online {
            return Some(Self {
                is_online,
                offline_cause: None,
                paper_status,
            });
        }

        let is_cover_open = (offline_cause & 0b100) != 0;
        let is_paper_empty = (offline_cause & 0b100000) != 0;
        let is_error = (offline_cause & 0b1000000) != 0;

        if !is_error {
            return Some(Self {
                is_online,
                offline_cause: Some(OfflineCause {
                    is_cover_open,
                    is_paper_empty,
                    error: None,
                }),
                paper_status,
            });
        }

        let is_cutter_error = (error_cause & 0b1000) != 0;
        let is_unrecoverable_error = (error_cause & 0b100000) != 0;
        let is_auto_recoverable_error = (error_cause & 0b1000000) != 0;

        Some(Self {
            is_online,
            offline_cause: Some(OfflineCause {
                is_cover_open,
                is_paper_empty,
                error: Some(PrinterError {
                    is_cutter_error,
                    is_unrecoverable_error,
                    is_auto_recoverable_error,
                }),
            }),
            paper_status,
        })
    }
}

impl Display for PrinterStatus {
    #[rustfmt::skip]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Status: {}\nPaper: {}",
            if self.is_online { "\x1b[32;1mONLINE\x1b[0m" } else { "\x1b[31;1mOFFLINE\x1b[0m" },
            match self.paper_status {
                PaperStatus::NotPresent => "\x1b[31;1mNOT DETECTED\x1b[0m",
                PaperStatus::NearEnd => "\x1b[33;1mRUNNING OUT\x1b[0m",
                PaperStatus::Adequate => "\x1b[32;1mADEQUATE\x1b[0m",
            }
        ).unwrap();

        if !self.is_online && self.offline_cause.is_some() {
            let cause = self.offline_cause.as_ref().unwrap();
            let mut reasons = Vec::new();

            if cause.is_cover_open { reasons.push("Cover is open"); }
            if cause.is_paper_empty { reasons.push("Out of paper"); }

            if let Some(error) = &cause.error {
                if error.is_cutter_error { reasons.push("Cutter error"); }
                if error.is_unrecoverable_error { reasons.push("Unrecoverable error"); }
                if error.is_auto_recoverable_error { reasons.push("Auto-recoverable error"); }
            }

            writeln!(f).unwrap();
            reasons
                .iter()
                .for_each(|r| write!(f, "\n\x1b[31;1m[Error]\x1b[0m {}", r).unwrap());
        }

        Ok(())
    }
}

pub(crate) const CMD_PRINT_AND_FEED: &[u8] = &[0x0A]; // LF

const _CMD_PRINT_AND_FEED_N: &[u8] = &[ESC, b'd'];
def_cmd!(CMD_PRINT_AND_FEED_N, _CMD_PRINT_AND_FEED_N, n: u8);

pub(crate) const CMD_CUT: &[u8] = &[GS, b'V', b'1'];

pub(crate) const CMD_BOLD: &[u8] = &[ESC, b'E'];
pub(crate) const CMD_UNDERLINE: &[u8] = &[ESC, b'-'];
pub(crate) const CMD_CHAR_SIZE: &[u8] = &[ESC, b'!'];
