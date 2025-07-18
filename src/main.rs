use std::path::PathBuf;

use clap::{Parser, Subcommand};
use printer::PrintyResult;

use crate::printer::Printer;

mod escpos;
mod printer;

#[derive(Parser)]
#[command(about = r"
    .--.
   |    |
   | uwu|
  .'----'.
 /________\
|  (· ‿ ·) |
|__________|
'----------'")]
struct Cli {
    #[arg(
        long = "vid",
        default_value_t = 0x4b8,
        help = "Defaults to VID of Espon TM-T88IV"
    )]
    usb_vendor_id: u16,
    #[arg(
        long = "pid",
        default_value_t = 0x202,
        help = "Defaults to PID of Espon TM-T88IV"
    )]
    usb_product_id: u16,
    #[arg(long, short, default_value_t = false)]
    debug: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Status,
    Print { file: PathBuf },
}

fn main() -> PrintyResult<()> {
    let args = Cli::parse();

    let mut printer = match if args.debug {
        Printer::debug()
    } else {
        Printer::usb(args.usb_vendor_id, args.usb_product_id)
    } {
        Ok(printer) => printer,
        Err(e) => {
            println!(
                r"
    .--.
   |    |
   | SOS|
  .'----'.
 /________\
|  (T ᴖ T) |
|__________|
'----------'"
            );
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    match args.command {
        Commands::Print { file } => {
            let content = std::fs::read_to_string(&file).unwrap_or_else(|_| {
                eprintln!("Failed to read file: {}", file.display());
                std::process::exit(1);
            });

            match file.extension() {
                Some(ext) if ext == "md" => {
                    printer.print_md(&content)?.cut()?;
                }
                _ => {
                    printer.print(&content)?.cut()?;
                }
            }
        }
        Commands::Status => match printer.status() {
            Some(status) => println!("{status}"),
            None => println!("Unable to determine printer status!"),
        },
    }

    Ok(())
}
