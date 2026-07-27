//! Diagnostic: prove that WARDEN binds no socket until the user clicks Share.
//!
//! "Fully local at rest" is a product claim, and the unit test for it can only check that
//! the Rust-side endpoint handle is absent. This asks the OS instead: it counts this
//! process's own UDP sockets via lsof before and after `start()`, so the claim is measured
//! rather than asserted. Single-threaded on purpose, so no other test can bind a socket
//! underneath the measurement.
//!
//! Run: `cargo run --example observe_socket_probe`

use std::sync::Arc;
use warden_lib::observe::grants::GrantStore;
use warden_lib::observe::projection::ObservedState;
use warden_lib::observe::transport::{BindMode, EventSink, FrameSource, HostShare};
use warden_lib::store::Store;

fn udp_sockets() -> usize {
    let pid = std::process::id();
    let out = std::process::Command::new("lsof")
        .args(["-p", &pid.to_string(), "-a", "-i", "UDP", "-n", "-P"])
        .output()
        .expect("lsof should run on macOS");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .skip(1) // header
        .filter(|l| !l.trim().is_empty())
        .count()
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let dir = std::env::temp_dir().join(format!("warden-socket-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");

    let frames: FrameSource = Arc::new(|_salt: &[u8; 16]| None::<ObservedState>);
    let events: EventSink = Arc::new(|_| {});
    let host = HostShare::new(
        GrantStore::new(Store::memory().expect("memory store")),
        frames,
        events,
        "probe".to_string(),
        dir.join("observer_key"),
        // Public is the shipping configuration: relays plus DNS address lookup. Probing
        // that one is the point, since it is the mode that could contact a third party.
        BindMode::Public,
    );

    let before = udp_sockets();
    println!("UDP sockets after constructing the sharing runtime: {before}");
    println!("  endpoint bound: {:?}", host.endpoint_id().await.is_some());
    println!(
        "  key file written: {}",
        dir.join("observer_key").exists()
    );

    host.start().await.expect("start sharing");
    let after = udp_sockets();
    println!("UDP sockets after observe_start_sharing:            {after}");
    println!("  endpoint bound: {:?}", host.endpoint_id().await.is_some());

    host.stop().await.expect("stop sharing");
    let _ = std::fs::remove_dir_all(&dir);

    println!();
    if before == 0 && after > 0 {
        println!("PASS: nothing was bound at rest; sharing is what opened the socket.");
    } else {
        println!("FAIL: expected 0 sockets at rest and more than 0 after start.");
        std::process::exit(1);
    }
}
