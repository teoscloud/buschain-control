fn main() {
    if let Err(e) = buschain_tools::daemon::run() {
        eprintln!("buschain-daemon: {e:#}");
        std::process::exit(1);
    }
}
