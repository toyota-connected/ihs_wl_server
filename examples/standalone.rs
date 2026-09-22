//! Run the server with no shell, for poking at it with real clients:
//!
//!   cargo run --example standalone -- [socket-name] [seconds]
//!   WAYLAND_DISPLAY=<socket-name> foot
//!
//! Clients connect and map; nothing is shown, since there is no shell to
//! host the views.

use std::time::Duration;

fn main() {
    let mut args = std::env::args().skip(1);
    let socket_name = args.next();
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30);
    ihs_wl_server::start(ihs_wl_server::Config { socket_name }).expect("start");
    println!("listening on {}", ihs_wl_server::socket_name().unwrap());
    std::thread::sleep(Duration::from_secs(seconds));
    ihs_wl_server::stop().expect("stop");
}
