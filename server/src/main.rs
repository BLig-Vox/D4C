use std::{fs, path::PathBuf};
use vox_d4c_server::{manager::Manager, runtime::Runtime, store::Store};
fn main() {
    if let Err(e) = run() {
        eprintln!("D4C: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 2 && matches!(args[1].as_str(), "--help" | "-h") {
        println!(
            "D4C server\nUsage: d4c-server [--config PATH] [--list-devices | --approve DEVICE_ID | --import-aliases MANIFEST | --migrate-codex-tools HOME | --refresh-codex-tools HOME]\n       d4c-server --mcp\n       d4c-server --native-screen SOCKET (Linux X11)\n       d4c-server --help | --version"
        );
        return Ok(());
    }
    if args.len() == 2 && matches!(args[1].as_str(), "--version" | "-V") {
        println!("d4c-server {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    if args.get(1).is_some_and(|arg| arg == "--native-screen") {
        if args.len() != 3 || args[2].starts_with("--") {
            return Err("--native-screen requires a Unix socket path".into());
        }
        return vox_d4c_server::native_screen::serve(std::path::Path::new(&args[2]))
            .map_err(|e| e.to_string());
    }
    let mut options = args.iter().skip(1);
    while let Some(arg) = options.next() {
        match arg.as_str() {
            "--mcp" | "--list-devices" => {}
            "--config"
            | "--approve"
            | "--import-aliases"
            | "--migrate-codex-tools"
            | "--refresh-codex-tools" => {
                if options.next().is_none_or(|v| v.starts_with("--")) {
                    return Err(format!("{arg} requires a value"));
                }
            }
            _ => return Err(format!("Unknown option {arg}; use --help")),
        }
    }
    let option = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|n| args.get(n + 1))
            .cloned()
    };
    if args.iter().any(|a| a == "--mcp") {
        return vox_d4c_server::mcp::serve(
            &std::env::var("D4C_SOCKET").unwrap_or_else(|_| "/run/d4c/manager.sock".into()),
            &std::env::var("D4C_SESSION").map_err(|_| "D4C_SESSION required")?,
        );
    }
    if let Some(home) = option("--refresh-codex-tools") {
        let count = vox_d4c_server::codex_tools::refresh_tools(std::path::Path::new(&home))?;
        println!("Refreshed tool registrations on {count} D4C threads");
        return Ok(());
    }
    if let Some(home) = option("--migrate-codex-tools") {
        let count = vox_d4c_server::codex_tools::migrate_tools(std::path::Path::new(&home))?;
        println!("Updated Codex tools on {count} threads");
        return Ok(());
    }
    let path = option("--config").unwrap_or_else(|| "/etc/d4c/manager.json".into());
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let root = PathBuf::from(config["dataDirectory"].as_str().unwrap_or("/var/lib/d4c"));
    let mut db = Store::open(&root.join("workspace.sqlite"))?;
    if let Some(path) = option("--import-aliases") {
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        for (key, row) in manifest["sessions"]
            .as_object()
            .ok_or("Missing session map")?
        {
            if let Some(tid) = row["threadId"].as_str() {
                db.rekey(key, tid)?;
            }
        }
        println!("Session aliases imported");
        return Ok(());
    }
    if args.iter().any(|a| a == "--list-devices") {
        println!(
            "{}",
            serde_json::to_string_pretty(&db.devices()?).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if let Some(id) = option("--approve") {
        db.approve_device(&id)?;
        println!("Approved {id}");
        return Ok(());
    }
    let runtime = Runtime::new(root)?;
    vox_d4c_server::transport::serve(Manager::new(db, runtime), &config)
}
