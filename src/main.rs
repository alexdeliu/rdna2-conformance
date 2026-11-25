//! rdna2-conformance: runs every probe on every D3D12 adapter (hardware ones,
//! then WARP) and prints a Markdown report.
//!
//! ```text
//! rdna2-conformance                    all probes, all adapters
//! rdna2-conformance --only <id,..>     only these probes
//! rdna2-conformance --vectors <dir>    DXIL vectors from <dir> (default: ./vectors)
//! rdna2-conformance --out <file>       also write the report to a file
//! rdna2-conformance --dxil <id>        the DXIL disassembly of an HLSL probe's builds
//! ```

mod dxc;
mod gpu;
mod probes;
mod vectors;

use probes::{Outcome, Probe};
use std::collections::HashMap;
use std::fmt::Write as _;

struct Row {
    id: String,
    kind: &'static str,
    about: String,
    origin: String,
    outcomes: Vec<Outcome>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opt = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let all = probes::all();

    if let Some(id) = opt("--dxil") {
        let p = all
            .iter()
            .find(|p| p.id == id)
            .unwrap_or_else(|| panic!("no HLSL probe {id}"));
        for b in p.builds {
            println!("=== {} ({})", p.id, b.label);
            match dxc::disassemble(&probes::source(p), b.args) {
                Ok(d) => println!("{d}"),
                Err(e) => println!("{e}"),
            }
        }
        return;
    }

    let only: Option<Vec<String>> = opt("--only").map(|v| v.split(',').map(str::to_string).collect());
    let wanted = |id: &str| only.as_ref().is_none_or(|o| o.iter().any(|x| x == id));
    let hlsl: Vec<&Probe> = all.iter().filter(|p| wanted(p.id)).collect();
    let dir = opt("--vectors").unwrap_or_else(|| "vectors".into());
    let vecs: Vec<vectors::Vector> = match vectors::load(std::path::Path::new(&dir)) {
        Ok(v) => v.into_iter().filter(|v| wanted(&v.id)).collect(),
        Err(e) => {
            eprintln!("no DXIL vectors ({e})");
            Vec::new()
        }
    };

    // Compile each HLSL build once.
    let mut blobs: HashMap<(&str, &str), Result<Vec<u8>, String>> = HashMap::new();
    for p in &hlsl {
        for b in p.builds {
            blobs.insert((p.id, b.label), dxc::compile(&probes::source(p), b.args));
        }
    }

    let adapters = gpu::adapters();
    let mut rows: Vec<Row> = hlsl
        .iter()
        .map(|p| Row {
            id: p.id.into(),
            kind: "HLSL",
            about: p.about.into(),
            origin: p.origin.into(),
            outcomes: Vec::new(),
        })
        .chain(vecs.iter().map(|v| Row {
            id: v.id.clone(),
            kind: "DXIL",
            about: v.about.clone(),
            origin: v.origin.clone(),
            outcomes: Vec::new(),
        }))
        .collect();
    for a in &adapters {
        match gpu::Gpu::new(a) {
            Err(e) => rows
                .iter_mut()
                .for_each(|r| r.outcomes.push(Outcome::Skip(format!("no device: {e}")))),
            Ok(g) => {
                for (r, p) in rows.iter_mut().zip(&hlsl) {
                    r.outcomes.push(run(&g, p, &blobs));
                }
                for (r, v) in rows.iter_mut().skip(hlsl.len()).zip(&vecs) {
                    r.outcomes.push(vectors::run(&g, v));
                }
            }
        }
    }

    let report = report(&adapters, &rows);
    print!("{report}");
    if let Some(path) = opt("--out") {
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&path, &report).unwrap_or_else(|e| panic!("write {path}: {e}"));
    }
}

fn run(g: &gpu::Gpu, p: &Probe, blobs: &HashMap<(&str, &str), Result<Vec<u8>, String>>) -> Outcome {
    if g.shader_model() < 0x66 {
        return Outcome::Skip("below SM 6.6".into());
    }
    if p.needs_16bit && !g.native_16bit() {
        return Outcome::Skip("no native 16-bit ops".into());
    }
    let input = (p.input)();
    let mut outs = Vec::new();
    for b in p.builds {
        let blob = match &blobs[&(p.id, b.label)] {
            Ok(b) => b,
            Err(e) => return Outcome::Skip(format!("{}: {e}", b.label)),
        };
        match g.run(blob, 1, &input, probes::LANES * 4) {
            Ok(o) => outs.push(o),
            Err(e) => return Outcome::Skip(format!("{}: {e}", b.label)),
        }
    }
    (p.check)(&input, &outs)
}

fn cell(o: &Outcome) -> String {
    match o {
        Outcome::Pass => "pass".into(),
        Outcome::Fail { n, .. } => format!("**FAIL** ({n})"),
        Outcome::Info(_) => "info".into(),
        Outcome::Skip(_) => "skip".into(),
    }
}

fn report(adapters: &[gpu::AdapterInfo], rows: &[Row]) -> String {
    let mut md = String::new();
    let _ = writeln!(md, "# rdna2-conformance report\n");
    let _ = writeln!(
        md,
        "- rdna2-conformance {}, {}",
        env!("CARGO_PKG_VERSION"),
        dxc::version()
    );
    for a in adapters {
        let _ = writeln!(
            md,
            "- {}: vendor {:#06x}, device {:#06x}, driver {}",
            a.name, a.vendor, a.device, a.driver
        );
    }
    let names: Vec<&str> = adapters.iter().map(|a| a.name.as_str()).collect();
    let _ = writeln!(md, "\n| probe | kind | {} |", names.join(" | "));
    let _ = writeln!(md, "|---|---|{}", "---|".repeat(names.len()));
    for r in rows {
        let _ = writeln!(
            md,
            "| `{}` | {} | {} |",
            r.id,
            r.kind,
            r.outcomes.iter().map(cell).collect::<Vec<_>>().join(" | ")
        );
    }
    let _ = writeln!(
        md,
        "\n`FAIL (n)`: n lanes (HLSL) or output dwords (DXIL) wrong. `info`: measured, no single right answer.\n"
    );
    let _ = writeln!(md, "## Details\n");
    for r in rows {
        let _ = writeln!(
            md,
            "### `{}` ({})\n\n{}. Found in: {}.\n",
            r.id, r.kind, r.about, r.origin
        );
        for (a, o) in adapters.iter().zip(&r.outcomes) {
            let line = match o {
                Outcome::Pass => "pass".to_string(),
                Outcome::Fail { n, first } => format!("FAIL ({n} wrong); first: {first}"),
                Outcome::Info(s) => s.clone(),
                Outcome::Skip(s) => format!("skipped: {s}"),
            };
            let _ = writeln!(md, "- {}: {line}", a.name);
        }
        md.push('\n');
    }
    md
}
