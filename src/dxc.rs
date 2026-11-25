//! HLSL to signed DXIL with `dxc.exe`: `DXC` (a dxc.exe path) if set, else the
//! newest Windows SDK's.

use std::path::PathBuf;
use std::process::Command;

pub fn dxc_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DXC") {
        return Some(PathBuf::from(p));
    }
    let kits = PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&kits)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path().join(r"x64\dxc.exe")))
        .filter(|p| p.exists())
        .collect();
    v.sort();
    v.pop()
}

/// `dxc --version`'s first line.
pub fn version() -> String {
    dxc_path()
        .and_then(|p| Command::new(p).arg("--version").output().ok())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string()
        })
        .unwrap_or_else(|| "dxc.exe not found".into())
}

/// Compiles `src` (entry `main`, profile `cs_6_6`) with `extra` arguments.
pub fn compile(src: &str, extra: &[&str]) -> Result<Vec<u8>, String> {
    let dxc = dxc_path().ok_or("dxc.exe not found (set DXC)")?;
    let dir = std::env::temp_dir().join(format!("rdna2-conformance-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let hlsl = dir.join(format!("p{n}.hlsl"));
    let out = dir.join(format!("p{n}.dxil"));
    std::fs::write(&hlsl, src).map_err(|e| e.to_string())?;
    let o = Command::new(dxc)
        .args(["-T", "cs_6_6", "-E", "main", "-HV", "2021"])
        .args(extra)
        .arg("-Fo")
        .arg(&out)
        .arg(&hlsl)
        .output()
        .map_err(|e| e.to_string())?;
    if !o.status.success() {
        return Err(format!("dxc: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }
    std::fs::read(&out).map_err(|e| e.to_string())
}

/// `dxc -dumpbin`'s disassembly of `src` (for checking which DXIL a probe gets).
pub fn disassemble(src: &str, extra: &[&str]) -> Result<String, String> {
    let dxc = dxc_path().ok_or("dxc.exe not found (set DXC)")?;
    let dir = std::env::temp_dir().join(format!("rdna2-conformance-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let hlsl = dir.join(format!("d{n}.hlsl"));
    std::fs::write(&hlsl, src).map_err(|e| e.to_string())?;
    let o = Command::new(dxc)
        .args(["-T", "cs_6_6", "-E", "main", "-HV", "2021"])
        .args(extra)
        .arg(&hlsl)
        .output()
        .map_err(|e| e.to_string())?;
    if !o.status.success() {
        return Err(format!("dxc: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
