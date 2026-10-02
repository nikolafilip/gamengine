//! Hold a text on the clipboard for a while: what the screens gate pastes from
//! (`scripts/check-screens.sh`), with no tool that a machine may not have.
//!
//!   cargo run -p gm-client --example clipboard -- TEXT [SECS]

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let mut args = std::env::args().skip(1);
    let Some(text) = args.next() else {
        eprintln!("clipboard TEXT [SECS]");
        std::process::exit(2);
    };
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30);
    let held = arboard::Clipboard::new().and_then(|mut clipboard| {
        clipboard.set_text(text)?;
        Ok(clipboard)
    });
    match held {
        // The text is there for as long as its owner is.
        Ok(_clipboard) => {
            println!("clipboard: holding");
            std::thread::sleep(std::time::Duration::from_secs(secs));
        }
        Err(e) => {
            eprintln!("clipboard: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {}
