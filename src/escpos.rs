use std::fmt::Display;

use derive_builder::Builder;

macro_rules! def_cmd {
    ($fn_name:ident, $header:expr, $( $param_name:ident : $param_type:ty ),+) => {
        #[allow(non_snake_case)]
        pub(crate) fn $fn_name($( $param_name: $param_type ),+) -> Vec<u8> {
            [$header, &[$($param_name as u8),+]].concat()
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

#[derive(Debug, Builder, Clone)]
pub(crate) struct PrinterError {
    is_cutter_err: bool,
    is_fatal_err: bool,
    is_recoverable_err: bool,
}

#[derive(Debug, Builder, Clone)]
pub(crate) struct OfflineCause {
    is_cover_open: bool,
    is_paper_empty: bool,
    error: Option<PrinterError>,
}

#[derive(Debug, Clone)]
pub(crate) enum PaperStatus {
    Adequate,
    NearEnd,
    NotPresent,
}

#[derive(Debug, Builder)]
pub(crate) struct PrinterStatus {
    is_online: bool,
    offline_cause: Option<OfflineCause>,
    paper_status: PaperStatus,
}

impl PrinterStatus {
    pub(crate) fn from_bytes(bytes: &[u8; 4]) -> Option<Self> {
        // All bit masks used below are based on the ESC/POS `DLE EOT` status response format.
        // Reference: https://download4.epson.biz/sec_pubs/pos/reference_en/escpos/dle_eot.html
        if !bytes.iter().all(|&b| (b & 0b10010011) == 0b00010010) {
            // If the status bytes do not match the expected format, stop parsing and treat
            // it as an indeterminate status
            return None;
        }
        let [dev_status_b, off_cause_b, err_cause_b, paper_status_b] = bytes;

        let is_online = (dev_status_b & 0b1000) == 0;

        let off_err = if (off_cause_b & 0b1000000) != 0 {
            PrinterErrorBuilder::default()
                .is_cutter_err((err_cause_b & 0b1000) != 0)
                .is_fatal_err((err_cause_b & 0b100000) != 0)
                .is_recoverable_err((err_cause_b & 0b1000000) != 0)
                .build()
                .ok()
        } else {
            None
        };

        let off_cause = if !is_online {
            OfflineCauseBuilder::default()
                .is_cover_open((off_cause_b & 0b100) != 0)
                .is_paper_empty((off_cause_b & 0b100000) != 0)
                .error(off_err)
                .build()
                .ok()
        } else {
            None
        };

        PrinterStatusBuilder::default()
            .is_online(is_online)
            // Paper sensor logic hierarchy (assumed):
            // 1. End sensor takes priority - if it detects no paper, status is `NotPresent` regardless
            //    of near-end sensor
            // 2. If end sensor detects paper is present, then check near-end sensor:
            .paper_status(match paper_status_b {
                paper_status_b if (paper_status_b & 0b1100000) != 0 => PaperStatus::NotPresent,
                paper_status_b if (paper_status_b & 0b1100) != 0 => PaperStatus::NearEnd,
                _ => PaperStatus::Adequate,
            })
            .offline_cause(off_cause)
            .build()
            .ok()
    }
}

impl Display for PrinterStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const GREEN: &str = "\x1b[32;1m";
        const RED: &str = "\x1b[31;1m";
        const YELLOW: &str = "\x1b[33;1m";
        const MAGENTA: &str = "\x1b[35m";
        const RESET: &str = "\x1b[0m";

        let status_text = if self.is_online {
            format!("{GREEN}ONLINE{RESET}")
        } else {
            format!("{RED}OFFLINE{RESET}")
        };

        let paper_text = match self.paper_status {
            PaperStatus::Adequate => format!("{GREEN}OK{RESET}"),
            PaperStatus::NotPresent => format!("{RED}EMPTY{RESET}"),
            PaperStatus::NearEnd => format!("{YELLOW}LOW{RESET}"),
        };

        write!(f, "Status: {status_text} - Paper: {paper_text}")?;

        if !self.is_online {
            if let Some(cause) = &self.offline_cause {
                let mut issues = Vec::new();

                if let Some(error) = &cause.error {
                    if error.is_fatal_err {
                        issues.push(format!("{RED}fatal-error{RESET}"));
                    }
                    if error.is_recoverable_err {
                        issues.push(format!("{YELLOW}auto-recovery{RESET}"));
                    }
                    if error.is_cutter_err {
                        issues.push(format!("{MAGENTA}cutter-error{RESET}"));
                    }
                }

                if cause.is_cover_open {
                    issues.push(format!("{MAGENTA}cover-open{RESET}"));
                }
                if cause.is_paper_empty {
                    issues.push(format!("{MAGENTA}no-paper{RESET}"));
                }

                if !issues.is_empty() {
                    write!(f, " - Issues: {}", issues.join(", "))?;
                }
            }
        }

        Ok(())
    }
}

pub(crate) const CMD_DISABLE_ASB: &[u8] = &[GS, b'a', 0];

// Feeds paper to `[cutting_position + n * vert_motion]` and cut
// n is set to 0, which means the printer will cut right after the last printed line
pub(crate) const CMD_CUT: &[u8] = &[GS, b'V', 66, 0];

pub(crate) const _CMD_BOLD: &[u8] = &[ESC, b'E'];
def_cmd!(CMD_BOLD, _CMD_BOLD, enable: bool);

pub(crate) const _CMD_UNDERLINE: &[u8] = &[ESC, b'-'];
def_cmd!(CMD_UNDERLINE, _CMD_UNDERLINE, enable: bool);

#[allow(non_snake_case)]
pub(crate) fn CMD_CHAR_SIZE(h_magnify: u8, w_magnify: u8) -> Vec<u8> {
    vec![
        GS,
        b'!',
        (w_magnify.clamp(0, 8) << 4) | h_magnify.clamp(0, 8),
    ]
}

pub(crate) const _CMD_JUSTIFY: &[u8] = &[ESC, b'a'];
pub(crate) enum JustifyReq {
    Left = 0,
    Center = 1,
    Right = 2,
}
def_cmd!(CMD_JUSTIFY, _CMD_JUSTIFY, req: JustifyReq);

pub(crate) const CMD_PROC_DELAY_MS: u64 = 500;
