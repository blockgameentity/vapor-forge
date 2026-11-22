//! Print the identity and proxy forwarding plan for a Steam module on disk.
//!
//!   cargo run -p vapor-forge-host-windows --example proxy_plan -- <module.dll> [proxy-name]

use std::path::Path;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: proxy_plan <module.dll> [proxy-name]");
        std::process::exit(2);
    };
    let proxy = args.next().unwrap_or_else(|| "version.dll".to_owned());
    let path = Path::new(&path);

    let identity = match vapor_forge_host_windows::identify_path(path) {
        Ok(identity) => identity,
        Err(error) => {
            eprintln!("identify failed: {error}");
            std::process::exit(1);
        }
    };
    println!(
        "{} -> component {:?} {}bit sha256 {}",
        identity.name,
        vapor_forge_host_windows::module::corpus_component(&identity),
        identity.bits,
        identity.sha256
    );

    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("read failed: {error}");
            std::process::exit(1);
        }
    };
    match vapor_forge_host_windows::proxy::plan(&proxy, &bytes) {
        Ok(plan) => {
            println!("proxy {proxy} shadows {} exports:", plan.exports.len());
            for export in plan.exports.iter().take(8) {
                println!("  {export}");
            }
            if plan.exports.len() > 8 {
                println!("  ... {} more", plan.exports.len() - 8);
            }
        }
        Err(error) => println!("proxy plan unavailable: {error}"),
    }
}
