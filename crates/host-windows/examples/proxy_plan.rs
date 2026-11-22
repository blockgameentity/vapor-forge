//! Print the identity and proxy forwarding plan for a Steam module on disk.
//!
//!   cargo run -p vapor-forge-host-windows --example proxy_plan -- <module.dll> [proxy-name] [--emit-def]
//!
//! `--emit-def` prints the forwarding `.def` body, which is how the static
//! export list in `crates/host-windows/proxy/version.def` gets regenerated when
//! a client update adds an export. The body forwarders name the module
//! unqualified; the shipped `.def` qualifies the target with
//! `C:/Windows/System32/` because a bare module name resolves to the proxy
//! itself and would recurse, so add that prefix when you paste it in.

use std::path::Path;

fn main() {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    let emit_def = args.iter().any(|arg| arg == "--emit-def");
    args.retain(|arg| arg != "--emit-def");

    let mut args = args.into_iter();
    let Some(path) = args.next() else {
        eprintln!("usage: proxy_plan <module.dll> [proxy-name] [--emit-def]");
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
            if emit_def {
                println!("{}", plan.def_file());
                return;
            }
            for export in plan.exports.iter().take(8) {
                println!("  {export}");
            }
            if plan.exports.len() > 8 {
                println!("  ... {} more", plan.exports.len() - 8);
            }
            println!("  (--emit-def prints the forwarding .def body)");
        }
        Err(error) => println!("proxy plan unavailable: {error}"),
    }
}
