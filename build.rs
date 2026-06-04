// build.rs — SemVer version embedding + Windows icon resource.
//
// ── SemVer Versioning Policy (MAJOR.MINOR.PATCH, single-digit roll-over) ──────
//
//   Adapted from a standardized Rust versioning policy, with no BUILD counter
//   and no build-metadata segment: the version is exactly MAJOR.MINOR.PATCH.
//   MINOR and PATCH are single digits (0–9) and roll over like an odometer;
//   MAJOR is uncapped.
//   There is no auto-increment and no external shell script: the version changes
//   only on an explicit `BUMP=` invocation, handled entirely here.
//
//     BUMP=patch cargo build   PATCH + 1; at 9 → 0, carry into MINOR
//     BUMP=minor cargo build   MINOR + 1 (PATCH → 0); at 9 → 0, carry into MAJOR
//     BUMP=major cargo build   MAJOR + 1 (MINOR, PATCH → 0)
//
//   e.g.  1.2.9 --BUMP=patch--> 1.3.0      1.9.4 --BUMP=minor--> 2.0.0
//
//   build.rs rewrites `version` under [package] in Cargo.toml and emits
//   WEEK_CLOCK_VERSION for the crate to read via env!().
//
//   When to bump (binding policy — review should reject violations):
//     MAJOR  backward-incompatible change (removed/renamed public API, changed
//            wire/on-disk format, removed/redefined CLI flags, dropped platform
//            or MSRV support).
//     MINOR  backward-compatible new functionality (new API, new optional
//            fields, new flags with safe defaults, opt-in features).
//     PATCH  backward-compatible bug fixes (security fixes, logic corrections,
//            doc/help/error-message fixes, behavior-preserving dep bumps).
//
//   `BUMP` is one-shot: unset it after the bump, or the next rebuild (triggered
//   by the Cargo.toml rewrite) will bump again.
//       sh:    BUMP=patch cargo build && unset BUMP
//       pwsh:  $env:BUMP='patch'; cargo build; Remove-Item Env:BUMP
// ─────────────────────────────────────────────────────────────────────────────

use std::fs;

const CARGO_TOML_PATH: &str = "Cargo.toml";

fn main() {
    println!("cargo::rerun-if-changed=Cargo.toml");
    println!("cargo::rerun-if-env-changed=BUMP");

    let crate_name = std::env::var("CARGO_PKG_NAME").unwrap();
    let env_prefix: String = crate_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
        .collect();

    // Cargo.toml is authoritative; CARGO_PKG_VERSION is its parsed value.
    let mut version = std::env::var("CARGO_PKG_VERSION").unwrap();

    let bump = std::env::var("BUMP")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if let Some(level) = bump {
        let (maj, min, pat) = parse_semver(&version);
        let (m, n, p) = match level.as_str() {
            "major" => (maj + 1, 0, 0),
            "minor" => roll(maj, min + 1, 0),
            "patch" => roll(maj, min, pat + 1),
            other => {
                // Plain eprintln + exit(1) fails the build cleanly on any Cargo
                // version (avoids the MSRV that `cargo::error=` would impose).
                eprintln!("BUMP must be one of: major, minor, patch (case-sensitive). Got {other:?}");
                std::process::exit(1);
            }
        };
        let new_version = format!("{m}.{n}.{p}");
        println!("cargo::warning=BUMP={level}: {version} → {new_version}");
        rewrite_cargo_toml(&new_version);
        version = new_version;
    }

    // CARGO_PKG_VERSION is stale after a rewrite, so emit the value we just
    // computed; the crate reads WEEK_CLOCK_VERSION, not CARGO_PKG_VERSION.
    println!("cargo::rustc-env={env_prefix}_VERSION={version}");

    // Windows: embed the multi-size icon into the .exe (Explorer / taskbar).
    #[cfg(windows)]
    {
        println!("cargo::rerun-if-changed=assets/icon.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            // Don't fail the build if the resource compiler is unavailable.
            println!("cargo::warning=failed to embed icon resource: {e}");
        }
    }
}

/// Apply 0–9 odometer carry to MINOR/PATCH (MAJOR is uncapped). An incremented
/// component is at most 10 here, so a single carry per level suffices.
fn roll(mut maj: u64, mut min: u64, mut pat: u64) -> (u64, u64, u64) {
    if pat > 9 {
        min += pat / 10;
        pat %= 10;
    }
    if min > 9 {
        maj += min / 10;
        min %= 10;
    }
    (maj, min, pat)
}

/// Parse a clean "MAJOR.MINOR.PATCH" core. Any pre-release or build-metadata
/// suffix is rejected — Cargo.toml's package.version is authoritative and must
/// be a clean SemVer core under this policy.
fn parse_semver(s: &str) -> (u64, u64, u64) {
    fn fail(s: &str) -> ! {
        eprintln!(
            "Cargo.toml package.version must be a clean \"MAJOR.MINOR.PATCH\" \
             (no pre-release, no build metadata). Got {s:?}"
        );
        std::process::exit(1);
    }
    let mut parts = s.split('.');
    let mut take = || -> u64 {
        match parts.next().and_then(|p| p.parse().ok()) {
            Some(v) => v,
            None => fail(s),
        }
    };
    let (m, n, p) = (take(), take(), take());
    if parts.next().is_some() {
        fail(s);
    }
    (m, n, p)
}

/// Rewrite the `version = "..."` line under `[package]` in Cargo.toml, preserving
/// all other formatting. Best-effort atomic (write temp + rename).
fn rewrite_cargo_toml(new_version: &str) {
    let original = fs::read_to_string(CARGO_TOML_PATH).expect("read Cargo.toml");

    let mut out = String::with_capacity(original.len());
    let mut in_package = false;
    let mut replaced = false;
    for line in original.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            // Match exactly `[package]`, not `[package.metadata.*]`.
            let header = trimmed.split('#').next().unwrap_or("").trim_end();
            in_package = header == "[package]";
        }
        if in_package && !replaced {
            if let Some(after) = trimmed.strip_prefix("version") {
                if let Some(rest) = after.trim_start().strip_prefix('=') {
                    if rest.trim_start().starts_with('"') {
                        let lead = &line[..line.len() - trimmed.len()];
                        let newline = if line.ends_with("\r\n") {
                            "\r\n"
                        } else if line.ends_with('\n') {
                            "\n"
                        } else {
                            ""
                        };
                        out.push_str(&format!("{lead}version = \"{new_version}\"{newline}"));
                        replaced = true;
                        continue;
                    }
                }
            }
        }
        out.push_str(line);
    }

    if !replaced {
        eprintln!("Could not find a `version = \"...\"` line under [package] in Cargo.toml");
        std::process::exit(1);
    }

    let tmp = format!("{CARGO_TOML_PATH}.tmp");
    fs::write(&tmp, out).expect("write Cargo.toml.tmp");
    fs::rename(&tmp, CARGO_TOML_PATH).expect("rename Cargo.toml.tmp → Cargo.toml");
}
