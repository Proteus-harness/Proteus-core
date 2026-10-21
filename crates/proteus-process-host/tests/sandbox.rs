#[cfg(target_os = "linux")]
#[path = "sandbox_linux/main.rs"]
mod linux;

fn main() {
    #[cfg(target_os = "linux")]
    if let Err(error) = linux::main() {
        eprintln!("{error:?}");
        std::process::exit(1);
    }
}
