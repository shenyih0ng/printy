use crate::printer::Printer;

mod escpos;
mod printer;

const TM_T88IV_USB_ID: (u16, u16) = (0x4b8, 0x202);

fn main() {
    let printer = Printer::usb(TM_T88IV_USB_ID.0, TM_T88IV_USB_ID.1);
    // let printer = Printer::debug();

    match printer.status() {
        Some(status) => println!("{status}"),
        None => println!("Failed to retrieve printer status."),
    }
}
