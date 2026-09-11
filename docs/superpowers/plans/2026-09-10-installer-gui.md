# Installer & Tray GUI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a zero-command-line installer (`gdut-net-setup.exe`) with a Chinese GUI wizard, install into `C:\Program Files\gdut-net` with Start Menu integration, add a Chinese daily-driver tray GUI (left-click opens it), and migrate the author's machine off the Desktop kit.

**Architecture:** Same crate, two new bins (`gdut-net-setup` GUI/silent, `gdut-net-pack` host-side packer). The release artifact is a single `gdut-net-setup.exe` with the main exe + scripts appended as a checksummed payload container. Install/uninstall logic is extracted from `service.rs` into reusable cores shared by CLI and setup. The tray keeps its Win32 message pump and gains: named-mutex single instance, a wake event for "show GUI", left-click = open, and one persistent eframe window that hides on close instead of exiting.

**Tech Stack:** Rust 1.95+, windows 0.62 (+`Win32_System_Com`), eframe/egui 0.36 (glow + default_fonts, ADR-0006), tray-icon 0.24, sha2 0.10 (payload checksums), tempfile 3 (dev only).

**Spec:** `docs/superpowers/specs/2026-09-10-installer-gui-design.md`. Read it plus `CONTEXT.md` rules before starting.

## Global Constraints

- Console/CLI/log/bat/script output: **English only** (GBK consoles garble Chinese). GUI text: **Chinese**. Source comments: either language (house style).
- CLI behavior is frozen: subcommands, prompts, exit codes, `--password-stdin` piping (`switch-v4.ps1` depends on it). New flags only add.
- Pure logic (payload, args parsing, script inventory) must be `#[cfg]`-free and Linux-testable; Win32/egui code is `#[cfg(windows)]` and verified with `cargo check --target x86_64-pc-windows-msvc`.
- Every Windows task ends with: `cargo check --target x86_64-pc-windows-msvc && cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings && cargo fmt`.
- Every Linux-testable task ends with: `cargo test && cargo clippy -- -D warnings && cargo fmt`.
- windows 0.62 API names below were verified against `~/.cargo/registry/src/index.crates.io-*/windows-0.62.2/`; vendored source is the source of truth when `cargo check` disagrees.
- GUI Chinese renders only with a system CJK font loaded (egui default fonts have no CJK glyphs). Every egui entry point calls `fonts::install_cjk_fonts` before showing.
- Secrets: never log/print the password or a portal URL with query; `--keep-password` reuses the DPAPI blob (`crypto::unprotect`) and must never touch plaintext on disk.
- Commit style: `feat:/fix:/test:/docs:` like existing history.
- TDD: Linux tasks write the failing test first and run it. Windows-only tasks use the cross-compile gates above; real-machine checks are named in the step.

## File Map

- Create `src/payload.rs` — payload container pack/unpack + name validation (pure).
- Create `src/packaging.rs` — fs-level payload collection and packing (pure, used by `gdut-net-pack`).
- Create `src/bin/gdut-net-pack.rs` — host-side packer CLI.
- Create `src/bin/gdut-net-setup.rs` — setup entry (Windows GUI subsystem).
- Create `src/setup/mod.rs` — constants, elevation, entry dispatch.
- Create `src/setup/args.rs` → top-level `src/setup_args.rs` (pure, Linux-tested) — `SetupArgs::parse`.
- Create `src/setup/ui.rs` — egui wizard + maintenance pages.
- Create `src/setup/work.rs` — install/uninstall/start-service workflows with step events and rollback.
- Create `src/setup/silent.rs` — `--silent` English-output mode.
- Create `src/shell.rs` — Start Menu shortcuts (IShellLink), uninstall registry key, delayed dir removal.
- Create `src/fonts.rs` — CJK font loading for egui.
- Create `src/tray/gui.rs` — persistent daily GUI window + `GuiShared` wake plumbing.
- Delete `src/tray/panel.rs` (replaced by `gui.rs`).
- Create `packaging/payload/` — shipped files: 7 bats + `说明.txt`.
- Create `packaging/personal/` — author ops files (not shipped): `switch-v4.ps1`, `rollback.bat`, `rollback-v4.ps1`, `一键切换.bat`, `tun-watch.ps1`.
- Create `packaging/dev/capture-window.ps1` — dev-only window screenshot helper (not shipped).
- Modify `src/service.rs`, `src/tray/mod.rs`, `src/cli.rs`, `src/lib.rs`, `src/logging.rs`, `Cargo.toml`, `.github/workflows/{ci,release}.yml`, `README.md`, `AGENTS.md`, `CONTEXT.md`, `docs/{desktop-kit,acceptance}.md`, `PRODUCT.md`.
- Create `tests/payload.rs`, `tests/packaging.rs`, `tests/setup_args.rs`, `tests/payload_inventory.rs`.

---

### Task 1: Visual direction for both GUIs (impeccable, user-interactive)

**Files:**
- Read: `PRODUCT.md`, `docs/superpowers/specs/2026-09-10-installer-gui-design.md`
- Create: surface briefs via the impeccable CLI (its own storage), later `DESIGN.md`

**Interfaces:**
- Consumes: PRODUCT.md (exists), the spec.
- Produces: the locked visual world + two surface briefs with `## Direction contract` blocks. Tasks 7–11 read these before writing UI code.

This task is user-interactive (decision page in the browser): execute it in the main session, never via a subagent.

- [ ] **Step 1: Load the skill and context.** Use the `impeccable` skill; run `impeccable context` once. Read `reference/new-work.md` (world discovery) and `reference/craft-floor.md` only right before UI edits.

- [ ] **Step 2: Do new-work §3 world discovery.** Name the product's mechanism, the audience's real scene, its cultural home, and what these surfaces must prove. List 7 grounded visual systems from that world (spanning ≥3 material families). Both surfaces share ONE world: installer = Operate, daily GUI = Operate.

- [ ] **Step 3: Run the direction roll.** `.opencode/skills/impeccable/scripts/impeccable concept-seed --scope direction --mode operate` and follow exactly what it prints (assignment, challengers, verdicts, raises, decision page command). Serve the decision page; the user locks one card.

- [ ] **Step 4: Record surface briefs.** For each surface write the brief with all six contract blocks (THESIS / OWN-WORLD / STORY / FIRST VIEWPORT / FORM + seed key / FINISH):
  - daily GUI primary target `src/tray/gui.rs`
  - installer primary target `src/setup/ui.rs`
  Use `impeccable surface-brief write <target> <body-file>`; verify all six blocks are present before building.

- [ ] **Step 5: Commit the briefs** (whatever path the skill stores them under) with `docs: lock visual direction for setup + daily GUI`.

---

### Task 2: Payload container (pure)

**Files:**
- Create: `src/payload.rs`
- Modify: `src/lib.rs` (add `pub mod payload;`)
- Modify: `Cargo.toml` (add `sha2 = "0.10"`, `[dev-dependencies] tempfile = "3"`)
- Test: `tests/payload.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
```rust
pub const FOOTER_LEN: usize = 24;
pub fn pack(setup: &[u8], entries: &[(String, Vec<u8>)]) -> anyhow::Result<Vec<u8>>;
pub fn unpack(exe: &[u8]) -> anyhow::Result<Option<Vec<Entry>>>;
pub fn validate_name(name: &str) -> anyhow::Result<()>;
pub struct Entry { pub name: String, pub data: Vec<u8> }
```

Layout (all integers little-endian): `[original setup bytes][entry data...][TOC][footer]`.
TOC entry: `u16 name_len | name (UTF-8) | u64 offset | u64 len | [u8;32] sha256`.
Footer (last 24 bytes): `"GDUTPAK1" | u32 version=1 | u32 count | u64 toc_offset` (absolute file offset).

- [ ] **Step 1: Write failing tests** — `tests/payload.rs`:

```rust
use gdut_net::payload::{pack, unpack, validate_name, Entry, FOOTER_LEN, MAGIC};

fn entries() -> Vec<(String, Vec<u8>)> {
    vec![
        ("gdut-net.exe".to_string(), b"MZ fake exe".to_vec()),
        ("说明.txt".to_string(), "中文说明".as_bytes().to_vec()),
        ("status.bat".to_string(), b"@echo off\r\n".to_vec()),
    ]
}

#[test]
fn round_trip_keeps_setup_prefix_and_data() {
    let setup = b"MZ this is the setup exe".to_vec();
    let packed = pack(&setup, &entries()).unwrap();
    assert_eq!(&packed[..setup.len()], &setup[..]);
    assert!(packed.len() > setup.len() + FOOTER_LEN);
    let got = unpack(&packed).unwrap().expect("payload present");
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].data, b"MZ fake exe");
    assert_eq!(got[1].name, "说明.txt");
    assert_eq!(got[2].data, b"@echo off\r\n");
}

#[test]
fn unpack_without_magic_returns_none() {
    assert!(unpack(b"just a plain exe").unwrap().is_none());
    assert!(unpack(&[0u8; 8]).unwrap().is_none());
}

#[test]
fn corrupted_data_fails_checksum() {
    let setup = b"MZ setup".to_vec();
    let mut packed = pack(&setup, &entries()).unwrap();
    let victim = setup.len(); // first data byte of entry 0
    packed[victim] ^= 0xff;
    let err = unpack(&packed).unwrap_err();
    assert!(format!("{err:#}").contains("checksum"), "got: {err:#}");
}

#[test]
fn magic_in_footer_but_truncated_toc_is_error() {
    let setup = b"MZ setup".to_vec();
    let mut packed = pack(&setup, &entries()).unwrap();
    packed.truncate(setup.len() + 4); // magic bytes survive? no: crop from end
    // rebuild: keep magic at end manually
    let mut fake = setup.clone();
    fake.extend_from_slice(MAGIC);
    fake.extend_from_slice(&[0u8; FOOTER_LEN - 8]);
    assert!(unpack(&fake).is_err());
    let _ = packed;
}

#[test]
fn validate_name_rejects_separators_and_dotdot() {
    for bad in ["", "a/b", "a\\b", "..", "../x", "C:evil"] {
        assert!(validate_name(bad).is_err(), "accepted {bad:?}");
    }
    for good in ["status.bat", "说明.txt", "a-b_c.d"] {
        assert!(validate_name(good).is_ok(), "rejected {good:?}");
    }
}

#[test]
fn duplicate_names_rejected() {
    let e = vec![
        ("a.txt".to_string(), vec![1]),
        ("a.txt".to_string(), vec![2]),
    ];
    assert!(pack(b"MZ", &e).is_err());
}

#[test]
fn empty_entries_are_allowed() {
    let packed = pack(b"MZ setup bytes", &[]).unwrap();
    let got = unpack(&packed).unwrap().expect("payload present");
    assert!(got.is_empty());
    let e: Vec<Entry> = got;
    assert!(e.is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail** — `cargo test --test payload` → compile error: `gdut_net::payload` missing.

- [ ] **Step 3: Implement `src/payload.rs`:**

```rust
//! 单文件发布载荷容器（纯逻辑）：[setup][data...][TOC][footer]。
//! footer 24B: "GDUTPAK1" | u32 version | u32 count | u64 toc_offset。
//! TOC 条目: u16 name_len | name(UTF-8) | u64 offset | u64 len | sha256[32]。

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

pub const MAGIC: &[u8; 8] = b"GDUTPAK1";
const VERSION: u32 = 1;
pub const FOOTER_LEN: usize = 24;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub data: Vec<u8>,
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

/// 文件名白名单：非空、无路径分隔符、无 `..`、无 ASCII 控制字符。
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("payload entry name is empty");
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") || name.contains(':') {
        bail!("payload entry name {name:?} must not contain path separators or \"..\"");
    }
    if name.chars().any(|c| c.is_control()) {
        bail!("payload entry name {name:?} contains control characters");
    }
    Ok(())
}

pub fn pack(setup: &[u8], entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut seen = std::collections::HashSet::new();
    for (name, _) in entries {
        validate_name(name)?;
        if !seen.insert(name.clone()) {
            bail!("duplicate payload entry name {name:?}");
        }
    }
    let mut out = setup.to_vec();
    let mut toc: Vec<u8> = Vec::new();
    for (name, data) in entries {
        let offset = out.len() as u64;
        out.extend_from_slice(data);
        let name_bytes = name.as_bytes();
        let name_len = u16::try_from(name_bytes.len()).context("payload name too long")?;
        toc.extend_from_slice(&name_len.to_le_bytes());
        toc.extend_from_slice(name_bytes);
        toc.extend_from_slice(&offset.to_le_bytes());
        toc.extend_from_slice(&(data.len() as u64).to_le_bytes());
        toc.extend_from_slice(&sha256(data));
    }
    let toc_offset = out.len() as u64;
    out.extend_from_slice(&toc);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend_from_slice(&toc_offset.to_le_bytes());
    Ok(out)
}

/// 读 payload；无 footer / magic 不符 → Ok(None)（开发态未打包）。
/// magic 相符但结构损坏 → Err（拒绝半安装）。
pub fn unpack(exe: &[u8]) -> Result<Option<Vec<Entry>>> {
    if exe.len() < FOOTER_LEN {
        return Ok(None);
    }
    let footer = &exe[exe.len() - FOOTER_LEN..];
    if &footer[..8] != MAGIC {
        return Ok(None);
    }
    let version = u32::from_le_bytes(footer[8..12].try_into().unwrap());
    if version != VERSION {
        bail!("unsupported payload version {version}");
    }
    let count = u32::from_le_bytes(footer[12..16].try_into().unwrap()) as usize;
    let toc_offset = u64::from_le_bytes(footer[16..24].try_into().unwrap()) as usize;
    let toc_end = exe.len() - FOOTER_LEN;
    if toc_offset > toc_end {
        bail!("payload TOC offset {toc_offset} out of range (len {toc_end})");
    }
    let toc = &exe[toc_offset..toc_end];
    let mut pos = 0usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        if pos + 2 > toc.len() {
            bail!("payload TOC truncated");
        }
        let name_len = u16::from_le_bytes(toc[pos..pos + 2].try_into().unwrap()) as usize;
        pos += 2;
        if pos + name_len + 8 + 8 + 32 > toc.len() {
            bail!("payload TOC entry truncated");
        }
        let name = std::str::from_utf8(&toc[pos..pos + name_len])
            .context("payload entry name is not UTF-8")?
            .to_string();
        validate_name(&name)?;
        pos += name_len;
        let offset = u64::from_le_bytes(toc[pos..pos + 8].try_into().unwrap()) as usize;
        pos += 8;
        let len = u64::from_le_bytes(toc[pos..pos + 8].try_into().unwrap()) as usize;
        pos += 8;
        let want: [u8; 32] = toc[pos..pos + 32].try_into().unwrap();
        pos += 32;
        if offset.checked_add(len).map_or(true, |end| end > toc_offset) {
            bail!("payload entry {name:?} out of data range");
        }
        let data = &exe[offset..offset + len];
        if sha256(data) != want {
            bail!("payload checksum mismatch for {name:?}");
        }
        entries.push(Entry { name, data: data.to_vec() });
    }
    if pos != toc.len() {
        bail!("payload TOC has trailing bytes");
    }
    Ok(Some(entries))
}
```

- [ ] **Step 4: Run tests** — `cargo test --test payload` → all pass.

- [ ] **Step 5: Gates + commit**

```bash
cargo test && cargo clippy -- -D warnings && cargo fmt
git add Cargo.toml Cargo.lock src/payload.rs src/lib.rs tests/payload.rs
git commit -m "feat(payload): single-file payload container for the setup exe"
```

---

### Task 3: Install/uninstall cores + `--keep-password`

**Files:**
- Modify: `src/service.rs` (windows `mod win`)
- Modify: `src/tray/mod.rs` (`register_autostart(exe: &Path)`)
- Modify: `src/cli.rs` (`Install { keep_password }`, thin shells, no-arg double-click path added in Task 10 — not here)
- Test: cross-compile + real-machine CLI checks (Windows-only code has no Linux unit tests)

**Interfaces:**
- Consumes: `crate::crypto::{protect, unprotect}`.
- Produces (used by setup in Tasks 7–9):
```rust
pub enum Credential { Plain(String), KeepExisting }
pub struct InstallRequest {
    pub cfg_path: std::path::PathBuf,
    pub student_id: Option<String>,
    pub credential: Credential,
    pub service_exe: std::path::PathBuf,
    pub tray_exe: std::path::PathBuf,
}
pub struct InstallOutcome { pub student_id: String, pub cfg_path: std::path::PathBuf }
pub enum InstallState { NotInstalled, Installed { service_exe: std::path::PathBuf, version: Option<String> } }
pub fn install_core(req: InstallRequest) -> anyhow::Result<InstallOutcome>;
pub fn uninstall_core(cfg_path: &Path, purge: bool) -> anyhow::Result<()>;
pub fn install_state() -> InstallState;
pub fn existing_account(cfg_path: &Path) -> Option<String>;   // student_id if config + blob exist
pub fn stop_service(timeout: std::time::Duration) -> anyhow::Result<()>;  // no-op if absent
pub fn start_service() -> anyhow::Result<()>;
```

- [ ] **Step 1: Refactor `install` into `install_core` + thin CLI shell.** In `src/service.rs`:

```rust
pub enum Credential { Plain(String), KeepExisting }
pub struct InstallRequest {
    pub cfg_path: PathBuf,
    pub student_id: Option<String>,
    pub credential: Credential,
    pub service_exe: PathBuf,
    pub tray_exe: PathBuf,
}
pub struct InstallOutcome { pub student_id: String, pub cfg_path: PathBuf }
pub enum InstallState { NotInstalled, Installed { service_exe: PathBuf, version: Option<String> } }

/// 安装核心：显式接收 exe 路径与凭据；不打印、不提示（CLI 外壳与 setup 共用）。
pub fn install_core(req: InstallRequest) -> Result<InstallOutcome> {
    let mut cfg = if req.cfg_path.exists() { Config::load(&req.cfg_path)? } else { Config::default() };
    let password = match req.credential {
        Credential::Plain(p) => {
            if p.is_empty() { bail!("Password must not be empty"); }
            p
        }
        Credential::KeepExisting => {
            if cfg.account.password_blob.is_empty() {
                bail!("No stored password to keep (config has no password_blob)");
            }
            crate::crypto::unprotect(&cfg.account.password_blob)
                .context("Stored password cannot be decrypted (entropy/config mismatch)")?
        }
    };
    if let Some(id) = req.student_id {
        if !id.trim().is_empty() { cfg.account.student_id = id; }
    }
    if cfg.account.student_id.trim().is_empty() {
        bail!("Student ID must not be empty");
    }
    // 存量迁移规则与旧 install 相同（9.9.9.9 被墙）。
    if cfg.dial.http_probe_url == "http://9.9.9.9" {
        cfg.dial.http_probe_url = "http://223.5.5.5".into();
        log::info!("Auto-migrated http_probe_url: 9.9.9.9 -> 223.5.5.5");
    }
    match req.credential {
        // Plain 才重写密文；KeepExisting 原样保留（重加密无意义且多一次 DPAPI 调用）。
        Credential::Plain(_) => cfg.account.password_blob = crate::crypto::protect(&password)?,
        Credential::KeepExisting => {}
    }
    cfg.save(&req.cfg_path)?;

    let pbk_path = PathBuf::from(&cfg.dial.pbk_path);
    if let Some(parent) = pbk_path.parent() { std::fs::create_dir_all(parent)?; }
    crate::ras::ensure_entry(&cfg.dial.pbk_path, &cfg.dial.entry_name)?;
    crate::ras::set_credentials(&cfg.dial.pbk_path, &cfg.dial.entry_name, &cfg.account.student_id, &password)?;

    create_service(&req.cfg_path, &req.service_exe).context("Failed to create/update service")?;
    if let Err(e) = set_recovery_actions() { log::warn!("Failed to set service recovery actions (ignored): {e:#}"); }
    if let Err(e) = eventlog::register_source() { log::warn!("Failed to register event source (ignored): {e:#}"); }
    crate::tray::register_autostart(&req.tray_exe)?;
    Ok(InstallOutcome { student_id: cfg.account.student_id.clone(), cfg_path: req.cfg_path })
}
```

`create_service(cfg_path: &Path, service_exe: &Path)`: replace the `service_binary()?` call with `service_exe`; delete `service_binary()` if unused after Task 10.

Also extend the re-export at the bottom of `src/service.rs` so the new core API is reachable from setup/cli (ruling R1):

```rust
#[cfg(windows)]
pub use win::{
    delete_service, existing_account, install, install_core, install_state, restore_service_path,
    service_main, start_service, stop_service, uninstall, uninstall_core, Credential,
    InstallOutcome, InstallRequest, InstallState,
};
```

CLI shell keeps exact prints:

```rust
pub fn install(cfg_path: &Path, password_stdin: bool, keep_password: bool) -> Result<()> {
    require_admin()?;
    if keep_password && password_stdin { bail!("--keep-password cannot be combined with --password-stdin"); }
    let password = if keep_password {
        // 明文不落盘、不打印；复用密文也要先验证可解密（错配早报错）。
        Credential::KeepExisting
    } else if password_stdin {
        Credential::Plain(read_stdin_password()?)
    } else {
        Credential::Plain(rpassword::prompt_password("Enter password: ")?)
    };
    let outcome = install_core(InstallRequest {
        cfg_path: cfg_path.to_path_buf(),
        student_id: None,               // CLI 沿用旧逻辑：仅当配置为空时再提示
        credential: password,
        service_exe: std::env::current_exe()?,
        tray_exe: std::env::current_exe()?,
    })?;
    // ... print the existing "Install complete:" block verbatim ...
    Ok(())
}
```

Note the CLI never prompts for student ID now — `install_core` errors when missing. Keep the old interactive prompt in the shell: before calling, if `Config::load(cfg_path)` has empty/absent student_id and not keep_password? Old behavior: `install` prompts for ID after password when config empty. Preserve exactly:

```rust
let sid = if Config::load(cfg_path).map(|c| c.account.student_id.trim().is_empty()).unwrap_or(true) {
    Some(prompt_nonempty("Enter student ID: ")?)
} else { None };
```

`uninstall_core(cfg_path, purge)`: current `uninstall()` body minus `println!`s, plus (Task 8) shell cleanup. `require_admin` stays in the CLI shell, not the core (setup already elevated).

- [ ] **Step 2: Add `--keep-password` to the CLI.** `src/cli.rs`:

```rust
Install {
    /// Reuse the stored DPAPI password (no prompt, no plaintext)
    #[arg(long)]
    keep_password: bool,
},
```
`dispatch` arm → `crate::service::install(&cli.config, cli.password_stdin, keep_password)`.

- [ ] **Step 3: `install_state` + service start/stop helpers.**

```rust
pub fn install_state() -> InstallState {
    let Ok(mgr) = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT) else {
        return InstallState::NotInstalled;
    };
    let access = ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG;
    match mgr.open_service(SERVICE_NAME, access) {
        Ok(svc) => match svc.query_config() {
            Ok(c) => InstallState::Installed { service_exe: c.executable_path, version: None },
            Err(_) => InstallState::Installed { service_exe: PathBuf::new(), version: None },
        },
        Err(_) => InstallState::NotInstalled,
    }
}

pub fn existing_account(cfg_path: &Path) -> Option<String> {
    let cfg = Config::load(cfg_path).ok()?;
    let id = cfg.account.student_id.trim().to_string();
    if id.is_empty() || cfg.account.password_blob.is_empty() { None } else { Some(id) }
}

pub fn stop_service(timeout: Duration) -> Result<()> {
    let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("Failed to connect to service manager")?;
    let svc = match mgr.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP) {
        Ok(s) => s,
        Err(_) => return Ok(()), // 未安装
    };
    if svc.query_status()?.current_state == ServiceState::Stopped { return Ok(()); }
    let _ = svc.stop();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if svc.query_status()?.current_state == ServiceState::Stopped { return Ok(()); }
        sleep(Duration::from_millis(250));
    }
    bail!("Service did not stop within {:?}", timeout)
}

pub fn start_service() -> Result<()> {
    let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("Failed to connect to service manager")?;
    let svc = mgr.open_service(SERVICE_NAME, ServiceAccess::START | ServiceAccess::QUERY_STATUS)
        .context("gdut-net service is not installed")?;
    if svc.query_status()?.current_state != ServiceState::Running {
        svc.start(&Vec::<OsString>::new())?;
    }
    Ok(())
}
```

`src/tray/mod.rs`:

```rust
pub fn register_autostart(tray_exe: &std::path::Path) -> Result<()> {
    let value = format!("\"{}\" tray", tray_exe.display());
    // ... rest unchanged ...
}
```

`crate::shell::installed_version()` does not exist yet (shell.rs lands in Task 7). Keep `version: None` here; Task 7 step 2 swaps in `crate::shell::installed_version()`. Do not create cross-task stubs.

- [ ] **Step 4: Cross-compile gates.**

Run: `cargo check --target x86_64-pc-windows-msvc && cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings && cargo fmt`
Expected: clean. Fix any drift against the vendored `windows-0.62.2` source, never the plan.

- [ ] **Step 5: Real-machine CLI contract checks (safe, no install).** Stage the cross-built exe to Windows and run from cmd:

```
gdut-net-new.exe install --help          → shows --keep-password
gdut-net-new.exe install --keep-password --password-stdin   → "cannot be combined" error, exit != 0
gdut-net-new.exe status                  → unchanged output
```
Full install-path regression happens in Task 12 (migration) and Task 9 (setup silent).

- [ ] **Step 6: Commit**

```bash
git add src/service.rs src/cli.rs src/tray/mod.rs
git commit -m "feat(install): extract install/uninstall cores, add --keep-password"
```

---

### Task 4: `gdut-net-pack` host packer (pure fs + CLI)

**Files:**
- Create: `src/packaging.rs`; modify `src/lib.rs`
- Create: `src/bin/gdut-net-pack.rs`
- Test: `tests/packaging.rs`

**Interfaces:**
- Consumes: `crate::payload::{pack, Entry}`.
- Produces:
```rust
pub fn collect_dir(dir: &std::path::Path) -> anyhow::Result<Vec<crate::payload::Entry>>; // sorted by name, flat files only
pub fn pack_into_file(setup: &std::path::Path, extras: &[std::path::PathBuf], payload_dir: &std::path::Path, out: &std::path::Path) -> anyhow::Result<()>;
```

- [ ] **Step 1: Write failing tests** — `tests/packaging.rs`:

```rust
use std::fs;
use gdut_net::packaging::pack_into_file;
use gdut_net::payload::unpack;

#[test]
fn packs_dir_and_extras_into_single_exe() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let payload = dir.join("payload");
    fs::create_dir(&payload).unwrap();
    fs::write(payload.join("status.bat"), b"@echo off\r\n").unwrap();
    fs::write(payload.join("说明.txt"), "你好".as_bytes()).unwrap();
    let exe = dir.join("gdut-net.exe");
    fs::write(&exe, b"MZ main exe").unwrap();
    let setup = dir.join("setup.exe");
    fs::write(&setup, b"MZ setup bytes").unwrap();
    let out = dir.join("dist.exe");

    pack_into_file(&setup, &[exe.clone()], &payload, &out).unwrap();

    let bytes = fs::read(&out).unwrap();
    assert_eq!(&bytes[..12], b"MZ setup byt");
    let entries = unpack(&bytes).unwrap().expect("payload");
    let names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
    assert_eq!(names, vec!["gdut-net.exe", "status.bat", "说明.txt"]);
    assert_eq!(entries[0].data, b"MZ main exe");
}

#[test]
fn missing_payload_dir_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let setup = tmp.path().join("s.exe");
    fs::write(&setup, b"MZ").unwrap();
    let out = tmp.path().join("o.exe");
    assert!(pack_into_file(&setup, &[], &tmp.path().join("nope"), &out).is_err());
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test --test packaging` → module missing.

- [ ] **Step 3: Implement `src/packaging.rs`:**

```rust
//! 打包文件级逻辑（纯 std）：收集 payload 目录 + 额外文件，写单文件发布物。

use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::payload::{pack, validate_name, Entry};

/// 读目录下所有常规文件（不递归），按名称排序保证产物可复现。
pub fn collect_dir(dir: &Path) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir(dir).with_context(|| format!("Failed to read {}", dir.display()))? {
        let item = item?;
        let path = item.path();
        if !item.file_type()?.is_file() { continue; }
        let name = path.file_name().and_then(|n| n.to_str()).context("Non-UTF-8 file name")?.to_string();
        validate_name(&name)?;
        let data = std::fs::read(&path).with_context(|| format!("Failed to read {}", path.display()))?;
        entries.push(Entry { name, data });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// setup 原文件 + extras（按 basename 收编）+ payload 目录 → out。
pub fn pack_into_file(setup: &Path, extras: &[std::path::PathBuf], payload_dir: &Path, out: &Path) -> Result<()> {
    let setup_bytes = std::fs::read(setup).with_context(|| format!("Failed to read {}", setup.display()))?;
    let mut entries = collect_dir(payload_dir)?;
    for extra in extras {
        let name = extra.file_name().and_then(|n| n.to_str()).context("Non-UTF-8 extra file name")?.to_string();
        validate_name(&name)?;
        if entries.iter().any(|e| e.name == name) { bail!("duplicate payload entry {name:?}"); }
        let data = std::fs::read(extra).with_context(|| format!("Failed to read {}", extra.display()))?;
        entries.push(Entry { name, data });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let bytes = pack(&setup_bytes, &entries.iter().map(|e| (e.name.clone(), e.data.clone())).collect::<Vec<_>>())?;
    if let Some(parent) = out.parent() { std::fs::create_dir_all(parent)?; }
    std::fs::write(out, bytes).with_context(|| format!("Failed to write {}", out.display()))?;
    Ok(())
}
```

- [ ] **Step 4: Create `src/bin/gdut-net-pack.rs`:**

```rust
//! gdut-net-pack — 把主程序与脚本追加到 setup.exe 尾部，产出单文件发布物。
//! Host 工具：不依赖 Windows，CI 与本地 xwin 构建后均可运行。
//! Usage: gdut-net-pack --setup <setup.exe> --payload-dir <dir> [--file <f>]... --out <dist.exe>

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut setup: Option<std::path::PathBuf> = None;
    let mut payload_dir: Option<std::path::PathBuf> = None;
    let mut extras: Vec<std::path::PathBuf> = Vec::new();
    let mut out: Option<std::path::PathBuf> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--setup" => setup = args.next().map(Into::into),
            "--payload-dir" => payload_dir = args.next().map(Into::into),
            "--file" => extras.push(args.next().context("--file needs a value")?.into()),
            "--out" => out = args.next().map(Into::into),
            other => anyhow::bail!("unknown argument {other:?}"),
        }
    }
    let setup = setup.context("--setup is required")?;
    let payload_dir = payload_dir.context("--payload-dir is required")?;
    let out = out.context("--out is required")?;
    gdut_net::packaging::pack_into_file(&setup, &extras, &payload_dir, &out)?;
    println!("Packed {} (+{} extras) -> {}", setup.display(), extras.len(), out.display());
    Ok(())
}
```
Add `use anyhow::Context as _;` at the top.

- [ ] **Step 5: Run tests + gates + commit**

```bash
cargo test --test packaging && cargo test && cargo clippy -- -D warnings && cargo fmt
git add src/packaging.rs src/bin/gdut-net-pack.rs src/lib.rs tests/packaging.rs Cargo.lock
git commit -m "feat(pack): single-file packer bin + fs-level packaging"
```

---

### Task 5: Product scripts + `说明.txt` + inventory test

**Files:**
- Create: `packaging/payload/status.bat`, `campus.bat`, `home.bat`, `tray.bat`, `wireless-test.bat`, `open-logs.bat`, `proxy-check.bat`, `说明.txt`
- Test: `tests/payload_inventory.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: the exact shipped file set Task 12's packer consumes; Task 9's installer copies.

- [ ] **Step 1: Write failing test** — `tests/payload_inventory.rs`:

```rust
use std::fs;
use std::path::Path;

const EXPECTED: &[&str] = &[
    "status.bat", "campus.bat", "home.bat", "tray.bat",
    "wireless-test.bat", "open-logs.bat", "proxy-check.bat", "说明.txt",
];

fn payload_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/packaging/payload"))
}

#[test]
fn payload_dir_is_exactly_the_expected_set() {
    let mut got: Vec<String> = fs::read_dir(payload_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    got.sort();
    let mut want: Vec<String> = EXPECTED.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(got, want, "payload dir diverged from the shipped list");
}

#[test]
fn bats_are_ascii_only() {
    for name in EXPECTED.iter().filter(|n| n.ends_with(".bat")) {
        let bytes = fs::read(payload_dir().join(name)).unwrap();
        assert!(bytes.iter().all(|b| *b < 0x80), "{name} contains non-ASCII bytes");
    }
}

#[test]
fn scripts_never_hardcode_desktop_paths_or_plaintext_passwords() {
    for name in EXPECTED {
        let text = fs::read_to_string(payload_dir().join(name)).unwrap();
        assert!(!text.contains("Lemonawa"), "{name} hardcodes a user path");
        assert!(!text.contains("pw.txt"), "{name} references the plaintext password file");
        assert!(!text.contains("--password-stdin"), "{name} should not pipe plaintext");
    }
}

#[test]
fn bats_use_script_relative_paths() {
    for name in ["status.bat", "tray.bat", "campus.bat", "wireless-test.bat"] {
        let text = fs::read_to_string(payload_dir().join(name)).unwrap();
        assert!(text.contains("%~dp0"), "{name} must be location-independent (%~dp0)");
    }
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test --test payload_inventory` → dir empty/missing.

- [ ] **Step 3: Write the payload files.** Exact contents:

`packaging/payload/status.bat`
```bat
@echo off
rem Show gdut-net service status (reads the live snapshot over the named pipe).
"%~dp0gdut-net.exe" status
pause
```

`packaging/payload/tray.bat`
```bat
@echo off
rem Start the gdut-net tray (left-click the icon to open the status window).
start "" "%~dp0gdut-net.exe" tray
```

`packaging/payload/open-logs.bat`
```bat
@echo off
rem Open the gdut-net log directory (config + dial book + logs live under ProgramData).
if not exist "C:\ProgramData\gdut-net\logs" mkdir "C:\ProgramData\gdut-net\logs"
explorer.exe "C:\ProgramData\gdut-net\logs"
```

`packaging/payload/proxy-check.bat`
```bat
@echo off
rem Show Windows system-proxy state. Expect ProxyEnable=0 (off).
powershell -NoProfile -Command "$p=Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings'; Write-Host ('ProxyEnable=' + $p.ProxyEnable + '  ProxyServer=' + $p.ProxyServer)"
pause
```

`packaging/payload/campus.bat`
```bat
@echo off
rem Campus mode: set the gdut-net service to Automatic, start it, wait for
rem dial success (max 180s), then run a 30s stability check.
rem Run as Administrator. Self-contained rollback at the bottom.

net session >nul 2>&1
if errorlevel 1 (
  echo NOT ADMIN: right-click and run as Administrator.
  pause
  exit /b 1
)

sc config gdut-net start=auto >nul
net start gdut-net >nul 2>&1

echo Waiting for dial success (max 180s)...
for /L %%i in (1,1,90) do (
  timeout /t 2 /nobreak >nul
  powershell -NoProfile -Command "Get-Content 'C:\ProgramData\gdut-net\logs\gdut-net_rCURRENT.log' -Tail 5 -Encoding UTF8 | Select-String 'Dial succeeded'" | findstr Dial >nul
  if not errorlevel 1 goto :dialed
)
goto :rollback

:dialed
echo DIAL OK, stability check 30s...
timeout /t 30 /nobreak >nul
powershell -NoProfile -Command "Get-Content 'C:\ProgramData\gdut-net\logs\gdut-net_rCURRENT.log' -Tail 10 -Encoding UTF8 | Select-String 'considered dropped|Probe failed'" | findstr "dropped failed" >nul
if not errorlevel 1 goto :rollback
echo CAMPUS MODE OK:
"%~dp0gdut-net.exe" status
pause
exit /b 0

:rollback
echo DIAL FAILED, rolling back...
net stop gdut-net >nul 2>&1
sc config gdut-net start=demand >nul
echo ROLLBACK DONE: service stopped and set to Manual.
echo If the campus wire needs the official client, use the full switch as Administrator.
pause
exit /b 1
```

`packaging/payload/home.bat`
```bat
@echo off
rem Home mode: campus PPPoE does not exist at home, so stop gdut-net
rem (avoids endless redial + toast spam) and set the service to Manual.
rem Proxy is left untouched (you may need Clash at home).
rem Run as Administrator. Self-contained rollback at the bottom.

net session >nul 2>&1
if errorlevel 1 (
  echo NOT ADMIN: right-click and run as Administrator.
  pause
  exit /b 1
)

net stop gdut-net >nul 2>&1
sc config gdut-net start=demand >nul
taskkill /F /IM gdut-net.exe >nul 2>&1

sc query gdut-net | findstr /C:"STOPPED" >nul
if errorlevel 1 goto :rollback
echo HOME MODE OK: service stopped and set to Manual.
echo At home your normal network works, gdut-net stays quiet.
pause
exit /b 0

:rollback
echo HOME SWITCH FAILED, rolling back to campus state...
sc config gdut-net start=auto >nul
net start gdut-net >nul 2>&1
echo ROLLBACK DONE: service restored to Automatic and started.
pause
exit /b 1
```

`packaging/payload/wireless-test.bat`
```bat
@echo off
rem Field-check the campus portal login (one-shot; auto-disconnects WLAN when done).
rem Needs admin: the temporary /32 route add fails otherwise.
rem Usage: right-click this file -> "Run as administrator".
setlocal
cd /d "%~dp0"

net session >nul 2>&1
if %errorlevel% neq 0 (
  echo [!] Not elevated - the /32 route cannot be added and the test will fail.
  echo     Right-click this file and choose "Run as administrator".
  pause
  exit /b 1
)

echo Running wireless test (joins gdut, one portal login, then disconnects)...
echo This takes about 20-40 seconds. Do not close this window.
echo.

rem Capture via PowerShell: a windows-subsystem exe prints nothing to a plain
rem cmd invocation, but PowerShell pipeline capture is verified to work.
powershell -NoProfile -Command "$o = & '.\gdut-net.exe' wireless test 2>&1 | Out-String; [Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Write-Output $o"

echo.
echo Expected: "RESULT: SUCCESS". Anything else - see the README troubleshooting section.
pause
```

`packaging/payload/说明.txt` (UTF-8, no BOM):
```
gdut-net 使用说明
================================

装在哪里
  C:\Program Files\gdut-net\     程序与脚本
  C:\ProgramData\gdut-net\       配置、拨号书、日志
  开始菜单 "GDUT Net" 文件夹     所有常用入口都在这里

【每天用的】
  GDUT Net          打开状态界面（左键点右下角托盘图标也是它）
  状态查看          命令行看一次状态；Connected + IP 就是在线
  代理检查          看系统代理，期望 ProxyEnable=0（关着）

【换地方】
  回校模式（管理员）  在学校：启动拨号，失败自动回滚
  回家模式（管理员）  在家里：停掉拨号防打扰，代理不动
  启动托盘            托盘被杀掉后用它重新拉起来

【排障用的】
  无线体检（管理员）  一次性校园 WiFi 认证实测，约 40s，自动断开不留状态
  打开日志            打开日志目录，报障先看这里

【无线接管】
  - 默认"有线优先，自动接管"：拔网线自动连校园 WiFi 并认证，
    插回网线 1~2 秒拨上有线、约 10 秒让位。
  - 界面里可切"有线+无线备用"：WiFi 常连，断线瞬间接替，
    但会一直占一个无线设备位。
  - 无线认证 = 校园 gdut 信号 + 和有线同一套学号密码。

【铁律】
  1. 拔线期间程序不拨号（防 PPPoE 端口卡死）；插回网线自动拨。
  2. 改学号密码：打开 GDUT Net -> 修改账号密码，或重跑开始菜单里的安装器。
  3. 代理只信"代理检查"，不信任何软件界面开关。
  4. 不要开 FlClash；Verge 的代理开关是坏的，别碰。
  5. 改过 Verge 的 Merge.yaml 后必须完整退出并重启 Verge 才生效；
     永远不要给 Mihomo 加 interface-name。
```

- [ ] **Step 4: Run tests + gates + commit**

```bash
cargo test --test payload_inventory && cargo test && cargo clippy -- -D warnings && cargo fmt
git add packaging/payload tests/payload_inventory.rs
git commit -m "feat(payload): shipped scripts + 说明.txt with inventory tests"
```

---

### Task 6: Setup skeleton — bin, args, elevation, fonts, UI shell

**Files:**
- Create: `src/bin/gdut-net-setup.rs`
- Create: `src/setup/mod.rs`, `src/setup/ui.rs`
- Create: `src/setup_args.rs` (pure), `src/fonts.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `crate::service::install_state()` from Task 3.
- Produces:
```rust
// src/setup_args.rs (pure, Linux-tested)
pub struct SetupArgs { pub silent: bool, pub uninstall: bool, pub repair: bool, pub start_service: bool, pub keep_password: bool, pub purge: bool }
pub enum Mode { Gui, Repair, Uninstall, StartService, SilentInstall, SilentUninstall }
impl SetupArgs { pub fn parse<I: IntoIterator<Item = String>>(args: I) -> anyhow::Result<Self>; pub fn mode(&self) -> anyhow::Result<Mode>; }
// src/fonts.rs (windows)
pub fn install_cjk_fonts(ctx: &egui::Context) -> anyhow::Result<String>;
// src/setup/mod.rs (windows)
pub fn entry() -> anyhow::Result<()>;
pub const START_MENU_FOLDER: &str = "GDUT Net";
pub const DATA_DIR: &str = r"C:\ProgramData\gdut-net";
pub fn install_dir() -> std::path::PathBuf;
pub fn is_admin() -> bool;
```

- [ ] **Step 1: Write failing args tests** — `tests/setup_args.rs`:

```rust
use gdut_net::setup_args::{Mode, SetupArgs};

fn parse(args: &[&str]) -> anyhow::Result<SetupArgs> {
    SetupArgs::parse(args.iter().map(|s| s.to_string()))
}

#[test]
fn defaults_to_gui() {
    let a = parse(&[]).unwrap();
    assert_eq!(a.mode().unwrap(), Mode::Gui);
}

#[test]
fn modes_parse() {
    assert_eq!(parse(&["--repair"]).unwrap().mode().unwrap(), Mode::Repair);
    assert_eq!(parse(&["--uninstall"]).unwrap().mode().unwrap(), Mode::Uninstall);
    assert_eq!(parse(&["--start-service"]).unwrap().mode().unwrap(), Mode::StartService);
    assert_eq!(parse(&["--silent"]).unwrap().mode().unwrap(), Mode::SilentInstall);
    assert_eq!(parse(&["--silent", "--uninstall"]).unwrap().mode().unwrap(), Mode::SilentUninstall);
}

#[test]
fn keep_password_requires_silent() {
    assert!(parse(&["--keep-password"]).is_err());
    let a = parse(&["--silent", "--keep-password"]).unwrap();
    assert!(a.keep_password);
}

#[test]
fn purge_requires_uninstall() {
    assert!(parse(&["--purge"]).is_err());
    assert!(parse(&["--silent", "--uninstall", "--purge"]).is_ok());
}

#[test]
fn conflicting_modes_rejected() {
    assert!(parse(&["--uninstall", "--repair"]).is_err());
    assert!(parse(&["--start-service", "--repair"]).is_err());
    assert!(parse(&["--silent", "--start-service"]).is_err());
}

#[test]
fn unknown_flag_rejected() {
    assert!(parse(&["--wat"]).is_err());
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test --test setup_args` → module missing.

- [ ] **Step 3: Implement `src/setup_args.rs`:**

```rust
//! setup 命令行参数（纯逻辑，Linux 可测）。GUI 默认；其余模式见 Mode。

use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Gui,
    Repair,
    Uninstall,
    StartService,
    SilentInstall,
    SilentUninstall,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetupArgs {
    pub silent: bool,
    pub uninstall: bool,
    pub repair: bool,
    pub start_service: bool,
    pub keep_password: bool,
    pub purge: bool,
}

impl SetupArgs {
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self> {
        let mut out = Self::default();
        for a in args {
            match a.as_str() {
                "--silent" => out.silent = true,
                "--uninstall" => out.uninstall = true,
                "--repair" => out.repair = true,
                "--start-service" => out.start_service = true,
                "--keep-password" => out.keep_password = true,
                "--purge" => out.purge = true,
                other => bail!("unknown argument {other:?}"),
            }
        }
        Ok(out)
    }

    pub fn mode(&self) -> Result<Mode> {
        if self.keep_password && !self.silent {
            bail!("--keep-password requires --silent");
        }
        if self.purge && !self.uninstall {
            bail!("--purge requires --uninstall");
        }
        let exclusive = [self.uninstall, self.repair, self.start_service]
            .iter()
            .filter(|b| **b)
            .count();
        if exclusive > 1 {
            bail!("--uninstall/--repair/--start-service are mutually exclusive");
        }
        if self.silent && self.start_service {
            bail!("--silent cannot combine with --start-service");
        }
        Ok(match (self.silent, self.uninstall, self.repair, self.start_service) {
            (true, true, _, _) => Mode::SilentUninstall,
            (true, false, _, _) => Mode::SilentInstall,
            (false, true, _, _) => Mode::Uninstall,
            (false, _, true, _) => Mode::Repair,
            (false, _, _, true) => Mode::StartService,
            _ => Mode::Gui,
        })
    }
}
```

- [ ] **Step 4: Run tests** — `cargo test --test setup_args` → pass.

- [ ] **Step 5: Implement `src/bin/gdut-net-setup.rs`:**

```rust
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        gdut_net::setup::entry()
    }
    #[cfg(not(windows))]
    {
        anyhow::bail!("gdut-net-setup is Windows-only")
    }
}
```

- [ ] **Step 6: Implement `src/fonts.rs`:**

```rust
//! egui 中文字体：从系统字体目录加载一个 CJK 字体并注入字体表。
//! epaint 0.36 用 skrifa 解析，支持 .ttc（index 0）；候选顺序先单文件 TTF。

use anyhow::{bail, Context, Result};

const CANDIDATES: [&str; 4] = ["Deng.ttf", "simhei.ttf", "msyh.ttc", "simsun.ttc"];

pub fn install_cjk_fonts(ctx: &egui::Context) -> Result<String> {
    let fonts_dir = std::path::PathBuf::from(
        std::env::var_os("SystemRoot").context("SystemRoot is not set")?,
    )
    .join("Fonts");
    for name in CANDIDATES {
        let path = fonts_dir.join(name);
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let mut defs = egui::FontDefinitions::default();
        defs.font_data.insert(
            "cjk".to_string(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            defs.families.entry(family).or_default().push("cjk".to_string());
        }
        ctx.set_fonts(defs);
        log::info!("Loaded CJK font {name} from {}", fonts_dir.display());
        return Ok(name.to_string());
    }
    bail!("No CJK font found in {} (tried {CANDIDATES:?})", fonts_dir.display())
}
```
(egui `FontDefinitions::font_data` is `BTreeMap<String, Arc<FontData>>` in epaint 0.36 — verified against vendored source.)

- [ ] **Step 7: Implement `src/setup/mod.rs`:**

```rust
//! 安装器入口（Windows）：参数解析 → 自提权 → silent 或 GUI。
//! 常量与安装布局见 spec §4；工作流在 work.rs，页面在 ui.rs。

use std::path::PathBuf;

use anyhow::{bail, Context, Result};

pub mod silent;
pub mod ui;
// `pub mod work;` lands here in Task 8.

pub use crate::setup_args::{Mode, SetupArgs};

pub const START_MENU_FOLDER: &str = "GDUT Net";
pub const DATA_DIR: &str = r"C:\ProgramData\gdut-net";

/// 安装目录：%ProgramFiles%\gdut-net。
pub fn install_dir() -> PathBuf {
    let base = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    base.join("gdut-net")
}

pub fn is_admin() -> bool {
    unsafe { windows::Win32::UI::Shell::IsUserAnAdmin() }.as_bool()
}

pub fn entry() -> Result<()> {
    let raw: Vec<String> = std::env::args().collect();
    let args = SetupArgs::parse(raw.iter().skip(1).cloned())?;
    let mode = args.mode()?;
    if !is_admin() {
        let wait = matches!(mode, Mode::SilentInstall | Mode::SilentUninstall);
        return elevate_self_and_maybe_wait(&raw, wait);
    }
    match mode {
        Mode::Gui | Mode::Repair | Mode::Uninstall | Mode::StartService => ui::run(args),
        Mode::SilentInstall | Mode::SilentUninstall => {
            crate::logging::init_cli_logging();
            crate::setup::silent::run(&args, mode)
        }
    }
}

/// ShellExecuteW "runas" 重启自身；silent 等非 GUI 模式等待并透传退出码。
fn elevate_self_and_maybe_wait(raw: &[String], wait: bool) -> Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let exe = std::env::current_exe()?;
    let params = quote_args(&raw[1..]);
    let exe_w: Vec<u16> = exe.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let verb = wide("runas");
    let params_w = wide(&params);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(exe_w.as_ptr()),
        lpParameters: PCWSTR(params_w.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    if !unsafe { ShellExecuteExW(&mut info) }.as_bool() {
        bail!("Elevation was cancelled (UAC declined?)");
    }
    if wait {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
        unsafe { WaitForSingleObject(info.hProcess, INFINITE) };
        let mut code = 1u32;
        unsafe { GetExitCodeProcess(info.hProcess, &mut code) }.ok()?;
        unsafe { let _ = CloseHandle(info.hProcess); }
        if code != 0 {
            bail!("Elevated setup failed with exit code {code}");
        }
    }
    Ok(())
}

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }

/// 参数带空格时加引号（安装路径可能含空格）。
fn quote_args(args: &[String]) -> String {
    args.iter().map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.clone() }).collect::<Vec<_>>().join(" ")
}
```
(`use std::os::windows::ffi::OsStrExt as _;` needed for `encode_wide`.)

- [ ] **Step 8: Implement UI shell `src/setup/ui.rs`** — minimal runnable app with fonts + pages that show and a disabled 下一步, wiring done in Task 8/9:

```rust
//! 安装器 GUI（中文）：向导 + 维护页。页面内容在后续任务补全。

use anyhow::Result;
use eframe::egui::{self, ViewportBuilder};

use crate::service::InstallState;

use super::{install_dir, SetupArgs};

pub fn run(args: SetupArgs) -> Result<()> {
    let state = crate::service::install_state();
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("GDUT Net 安装程序")
            .with_inner_size(egui::vec2(520.0, 460.0))
            .with_resizable(false),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "gdut-net-setup",
        options,
        Box::new(move |cc| {
            let font_ok = crate::fonts::install_cjk_fonts(&cc.egui_ctx).is_ok();
            Ok(Box::new(SetupApp::new(args, state, font_ok)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe failed: {e}"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page { Welcome, Account, Progress, Done, Maintenance, UninstallConfirm, StartService }

pub(crate) struct SetupApp {
    pub(crate) args: SetupArgs,
    pub(crate) state: InstallState,
    pub(crate) font_ok: bool,
    pub(crate) page: Page,
    pub(crate) error: Option<String>,
}

impl SetupApp {
    fn new(args: SetupArgs, state: InstallState, font_ok: bool) -> Self {
        let page = match state {
            InstallState::Installed { .. } => Page::Maintenance,
            InstallState::NotInstalled => Page::Welcome,
        };
        Self { args, state, font_ok, page, error: None }
    }
}

impl eframe::App for SetupApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            if !self.font_ok {
                ui.colored_label(egui::Color32::RED, "无法加载中文字体：请确认 C:\\Windows\\Fonts 下有 msyh.ttc / simhei.ttf。");
                return;
            }
            ui.heading("GDUT Net 安装程序");
            ui.add_space(8.0);
            match self.page {
                Page::Welcome => {
                    ui.label("gdut-net 会在后台自动完成校园网拨号，并在拔线时接管校园 WiFi。");
                    ui.add_space(4.0);
                    ui.label("安装过程不会改动你的代理、VPN 或其他网卡设置。");
                    ui.add_space(12.0);
                    if ui.button("开始安装").clicked() { self.page = Page::Account; }
                }
                Page::Account => {
                    ui.label("账号页将在下一个任务实现。");
                    if ui.button("返回").clicked() { self.page = Page::Welcome; }
                }
                Page::Maintenance => {
                    ui.label(format!("已安装位置：{}", install_dir().display()));
                    ui.add_space(8.0);
                    if ui.button("关闭").clicked() { ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close); }
                }
                _ => { ui.label("页面开发中"); }
            }
        });
    }
}
```
Also `pub mod silent;` in `src/setup/mod.rs` (empty stub is NOT allowed — create `src/setup/silent.rs` in Task 9; for Task 6 declare `pub mod silent;` only when the file exists. Declare it now and create a minimal `silent.rs` with `pub fn run(args: &SetupArgs, mode: Mode) -> Result<()> { bail!("silent mode lands in a later task") }` so the module compiles; Task 9 replaces the body.)

- [ ] **Step 9: Register modules in `src/lib.rs`:**

```rust
pub mod payload;
pub mod packaging;
pub mod setup_args;
#[cfg(windows)]
pub mod fonts;
#[cfg(windows)]
pub mod setup;
```

- [ ] **Step 10: Cross-compile gates** — same three commands as Task 3.

- [ ] **Step 11: Real-machine smoke.** Stage `gdut-net-setup.exe` (xwin build) to Windows and double-click it: UAC prompt appears, window opens, Chinese renders (no tofu). Note: this machine already has a service, so the app opens on the Maintenance page; verify it renders and the window closes cleanly. The Welcome→Account path is exercised once the service is removed (Task 9 checks) or on a clean machine. Screenshot if useful.

- [ ] **Step 12: Commit**

```bash
git add src/bin/gdut-net-setup.rs src/setup src/setup_args.rs src/fonts.rs src/lib.rs tests/setup_args.rs Cargo.lock
git commit -m "feat(setup): bin, args, elevation, CJK fonts, GUI shell"
```

---

### Task 7: Shell integration — shortcuts, uninstall key, cleanup

**Files:**
- Create: `src/shell.rs`; modify `src/lib.rs`
- Modify: `src/service.rs` (`uninstall_core` calls shell cleanup; `install_state` version)
- Modify: `Cargo.toml` (windows features `Win32_System_Com`)

**Interfaces:**
- Consumes: `crate::setup::{install_dir, START_MENU_FOLDER}` (constants only; no cycle: setup/work depends on shell, shell does not depend on setup).
  Correction to avoid a module cycle: put `START_MENU_FOLDER` in `shell.rs`; `setup/mod.rs` re-exports it. Define `shell::START_MENU_FOLDER` and have setup use it.
- Produces:
```rust
pub fn install_shell_integration(install_dir: &std::path::Path, version: &str) -> anyhow::Result<()>;
pub fn remove_shell_integration() -> anyhow::Result<()>;   // idempotent
pub fn installed_version() -> Option<String>;               // DisplayVersion or None
pub fn schedule_install_dir_removal(dir: &std::path::Path) -> anyhow::Result<()>;
```

- [ ] **Step 1: Implement `src/shell.rs`** (full code; COM sequence verified against vendored windows-0.62.2):

```rust
//! 安装态 shell 集成（Windows）：开始菜单快捷方式（IShellLink COM）、
//! "应用和功能"卸载项（HKLM）、卸载后延迟删除安装目录（cmd 助手）。

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

pub const START_MENU_FOLDER: &str = "GDUT Net";
const UNINSTALL_SUBKEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\gdut-net";

struct Shortcut {
    name: &'static str,
    target: &'static str,
    args: &'static str,
    run_as_admin: bool,
}

/// 开始菜单 10 项（spec §4）。target 相对安装目录。
const SHORTCUTS: &[Shortcut] = &[
    Shortcut { name: "GDUT Net", target: "gdut-net.exe", args: "", run_as_admin: false },
    Shortcut { name: "状态查看", target: "status.bat", args: "", run_as_admin: false },
    Shortcut { name: "回校模式", target: "campus.bat", args: "", run_as_admin: true },
    Shortcut { name: "回家模式", target: "home.bat", args: "", run_as_admin: true },
    Shortcut { name: "启动托盘", target: "gdut-net.exe", args: "tray", run_as_admin: false },
    Shortcut { name: "无线体检", target: "wireless-test.bat", args: "", run_as_admin: true },
    Shortcut { name: "打开日志", target: "open-logs.bat", args: "", run_as_admin: false },
    Shortcut { name: "代理检查", target: "proxy-check.bat", args: "", run_as_admin: false },
    Shortcut { name: "说明", target: "说明.txt", args: "", run_as_admin: false },
    Shortcut { name: "卸载 GDUT Net", target: "gdut-net-setup.exe", args: "--uninstall", run_as_admin: true },
];

fn start_menu_dir() -> Result<PathBuf> {
    let base = std::env::var_os("ProgramData").context("ProgramData is not set")?;
    Ok(PathBuf::from(base)
        .join(r"Microsoft\Windows\Start Menu\Programs")
        .join(START_MENU_FOLDER))
}

pub fn install_shell_integration(install_dir: &Path, version: &str) -> Result<()> {
    let dir = start_menu_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    for s in SHORTCUTS {
        let target = install_dir.join(s.target);
        if !target.exists() {
            bail!("Shortcut target missing: {}", target.display());
        }
        create_shortcut(&dir.join(format!("{}.lnk", s.name)), &target, s.args, s.run_as_admin, s.name)?;
    }
    write_uninstall_key(install_dir, version)
}

pub fn remove_shell_integration() -> Result<()> {
    if let Ok(dir) = start_menu_dir() {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => log::info!("Removed Start Menu folder {}", dir.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("Failed to remove {}: {e}", dir.display()),
        }
    }
    if let Err(e) = delete_uninstall_key() { log::warn!("Failed to remove uninstall registry key: {e:#}"); }
    Ok(())
}

/// 延迟删除安装目录：启动后直接退出的 setup 进程无法删掉自己所在的目录，
/// 交给 cmd 等待数秒后 rmdir（CREATE_NO_WINDOW | DETACHED_PROCESS）。
pub fn schedule_install_dir_removal(dir: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt as _;
    let dir = dir.to_path_buf();
    std::process::Command::new("cmd")
        .args(["/c", &format!(r#"ping -n 4 127.0.0.1 >nul & rmdir /s /q "{}""#, dir.display())])
        .creation_flags(0x0800_0000 | 0x0000_0008) // CREATE_NO_WINDOW | DETACHED_PROCESS
        .spawn()
        .context("Failed to spawn delayed directory removal")?;
    Ok(())
}

pub fn installed_version() -> Option<String> {
    read_uninstall_string("DisplayVersion")
}

// ---- COM / registry 胶水 ----

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() }

fn create_shortcut(lnk: &Path, target: &Path, args: &str, run_as_admin: bool, description: &str) -> Result<()> {
    use windows::core::Interface as _;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{IShellLinkDataList, IShellLinkW, ShellLink, SLDF_RUNAS_USER};

    unsafe {
        // S_FALSE(已初始化) 与 S_OK 均可；其他失败退出。
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if hr.is_err() { bail!("CoInitializeEx failed: {hr:?}"); }
        let result = (|| -> Result<()> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            let target_w = wide(&target.to_string_lossy());
            let args_w = wide(args);
            let desc_w = wide(description);
            unsafe { link.SetPath(windows::core::PCWSTR(target_w.as_ptr()))? };
            unsafe { link.SetArguments(windows::core::PCWSTR(args_w.as_ptr()))? };
            unsafe { link.SetDescription(windows::core::PCWSTR(desc_w.as_ptr()))? };
            if let Some(parent) = target.parent() {
                let dir_w = wide(&parent.to_string_lossy());
                unsafe { link.SetWorkingDirectory(windows::core::PCWSTR(dir_w.as_ptr()))? };
            }
            if run_as_admin {
                let dl: IShellLinkDataList = link.cast()?;
                let flags = unsafe { dl.GetFlags()? };
                unsafe { dl.SetFlags(flags | SLDF_RUNAS_USER.0 as u32)? };
            }
            let pf: IPersistFile = link.cast()?;
            let lnk_w = wide(&lnk.to_string_lossy());
            unsafe { pf.Save(windows::core::PCWSTR(lnk_w.as_ptr()), true)? };
            Ok(())
        })();
        CoUninitialize();
        result.with_context(|| format!("Failed to create shortcut {}", lnk.display()))?;
    }
    Ok(())
}

fn write_uninstall_key(install_dir: &Path, version: &str) -> Result<()> {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_WRITE,
        REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
    };
    let mut hkey = HKEY::default();
    let subkey = wide(UNINSTALL_SUBKEY);
    let ret = unsafe {
        RegCreateKeyExW(HKEY_LOCAL_MACHINE, windows::core::PCWSTR(subkey.as_ptr()), None,
            windows::core::PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut hkey, None)
    };
    if ret != windows::Win32::Foundation::ERROR_SUCCESS { bail!("RegCreateKeyExW failed: {}", ret.0); }
    let set_sz = |name: &str, value: &str| -> Result<()> {
        let n = wide(name);
        let v = wide(value);
        let bytes: Vec<u8> = v.iter().flat_map(|c| c.to_le_bytes()).collect();
        let ret = unsafe { RegSetValueExW(hkey, windows::core::PCWSTR(n.as_ptr()), None, REG_SZ, Some(&bytes)) };
        if ret != windows::Win32::Foundation::ERROR_SUCCESS { bail!("RegSetValueExW({name}) failed: {}", ret.0); }
        Ok(())
    };
    set_sz("DisplayName", "GDUT Net")?;
    set_sz("DisplayVersion", version)?;
    set_sz("InstallLocation", &install_dir.to_string_lossy())?;
    set_sz("DisplayIcon", &install_dir.join("gdut-net.exe").to_string_lossy())?;
    set_sz("UninstallString", &format!("\"{}\" --uninstall", install_dir.join("gdut-net-setup.exe").display()))?;
    set_sz("QuietUninstallString", &format!("\"{}\" --silent --uninstall", install_dir.join("gdut-net-setup.exe").display()))?;
    let one: u32 = 1;
    for name in ["NoModify", "NoRepair"] {
        let n = wide(name);
        let bytes = one.to_le_bytes();
        let ret = unsafe { RegSetValueExW(hkey, windows::core::PCWSTR(n.as_ptr()), None, REG_DWORD, Some(&bytes)) };
        if ret != windows::Win32::Foundation::ERROR_SUCCESS { bail!("RegSetValueExW({name}) failed: {}", ret.0); }
    }
    let closed = unsafe { RegCloseKey(hkey) };
    if closed != windows::Win32::Foundation::ERROR_SUCCESS { bail!("RegCloseKey failed: {}", closed.0); }
    Ok(())
}

fn delete_uninstall_key() -> Result<()> {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{RegDeleteTreeW, HKEY_LOCAL_MACHINE};
    let subkey = wide(UNINSTALL_SUBKEY);
    let ret = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, windows::core::PCWSTR(subkey.as_ptr())) };
    if ret != ERROR_SUCCESS && ret != ERROR_FILE_NOT_FOUND { bail!("RegDeleteTreeW failed: {}", ret.0); }
    Ok(())
}

fn read_uninstall_string(name: &str) -> Option<String> {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let subkey = wide(UNINSTALL_SUBKEY);
    let value = wide(name);
    let mut buf = [0u16; 256];
    let mut size = (buf.len() * 2) as u32;
    let ret = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, windows::core::PCWSTR(subkey.as_ptr()),
            windows::core::PCWSTR(value.as_ptr()), RRF_RT_REG_SZ, None,
            Some(buf.as_mut_ptr().cast()), Some(&mut size))
    };
    if ret != windows::Win32::Foundation::ERROR_SUCCESS { return None; }
    let len = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..len.min(buf.len())]))
}
```

Cargo.toml: add `"Win32_System_Com"` to the windows features list.

- [ ] **Step 2: Wire cleanup + version into `service.rs`:**

- `install_state` version: `crate::shell::installed_version()`.
- `uninstall_core`: after the existing autostart removal, add
```rust
if let Err(e) = crate::shell::remove_shell_integration() {
    log::warn!("Failed to remove shell integration (ignored): {e:#}");
}
```
- `install_state` and uninstall must tolerate `shell` failing on a non-installed machine (idempotent).

- [ ] **Step 3: Register `pub mod shell;` (cfg windows) in `src/lib.rs`.** Remove the `START_MENU_FOLDER` const from `setup/mod.rs` and re-export: `pub use crate::shell::START_MENU_FOLDER;` (setup UI uses it).

- [ ] **Step 4: Cross-compile gates** — same three commands.

- [ ] **Step 5: Real-machine check (safe subset).** Temporarily run a one-off `cargo xwin` build and, from an elevated PowerShell on Windows, call the shortcut+registry code through the setup wizard path once Task 8 lands. Until then verify only by cross-compile; the full shortcut check is in Task 12 (migration installs them for real).

- [ ] **Step 6 (added by ruling R6, spec §10): shortcut inventory as pure logic + consistency test.** Extract the `SHORTCUTS` table into a cfg-free `src/shell_shortcuts.rs` (`pub struct Shortcut { name, target, args, run_as_admin }`, `pub const SHORTCUTS`, `pub const EXTRA_TARGETS: &[&str] = &["gdut-net.exe", "gdut-net-setup.exe"]`), consumed by `shell.rs`. Add `tests/shell_shortcuts.rs`: exactly 10 entries with unique names; admin set == {回校模式, 回家模式, 无线体检, 卸载 GDUT Net}; 卸载 args == "--uninstall"; every target ASCII, no path separators, and either in `EXTRA_TARGETS` or an existing file in `packaging/payload/`. This closes the "快捷方式清单与 payload 一致性" test spec §10 promises.

- [ ] **Step 7: Commit**

```bash
git add src/shell.rs src/shell_shortcuts.rs src/service.rs src/setup/mod.rs src/lib.rs tests/shell_shortcuts.rs Cargo.toml
git commit -m "feat(shell): Start Menu shortcuts, uninstall key, delayed dir cleanup"
```

---

### Task 8: Install / repair / start-service workflow + wizard pages

**Files:**
- Create: `src/setup/work.rs`
- Modify: `src/setup/ui.rs` (Account/Progress/Done/StartService pages)
- Modify: `src/service.rs` (add `restore_service_path`, `delete_service` for rollback)

**Interfaces:**
- Consumes: `service::{install_core, existing_account, stop_service, start_service, InstallRequest, Credential, InstallState}`, `shell::install_shell_integration`, `payload::unpack`, `packaging::collect_dir`.
- Produces:
```rust
// src/setup/work.rs
pub enum Ev { Step(String), StepDone(String), Done(Result<(), String>) }
pub fn spawn_install(ui_tx: std::sync::mpsc::Sender<Ev>, args: SetupArgs, ui_student_id: String, ui_password: Option<String>); // worker thread
pub fn query_status_once() -> anyhow::Result<crate::ipc::protocol::StateSnapshot>;
pub fn spawn_start_service(ui_tx: std::sync::mpsc::Sender<Ev>);
```

- [ ] **Step 1: Add rollback helpers to `service.rs`:**

```rust
/// 安装失败回滚：把服务重新指回旧 exe（不重写配置）。
pub fn restore_service_path(cfg_path: &Path, service_exe: &Path) -> Result<()> {
    create_service(cfg_path, service_exe)
}

/// 回滚新建失败的服务：删除（不存在视为成功）。
pub fn delete_service() -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    match manager.open_service(SERVICE_NAME, ServiceAccess::DELETE) {
        Ok(svc) => svc.delete().context("Failed to delete service"),
        Err(_) => Ok(()),
    }
}
```

- [ ] **Step 2: Implement `src/setup/work.rs`:**

> **Ruling R7 (post-review):** `install_state` returns the raw service command line (exe + args) in `service_exe`; rollback must NOT pass it to `create_service` blindly. Add a pure, Linux-tested `src/cmdline.rs::first_token(line) -> &str` (quote-aware first-token split), have `install_state` return the parsed exe path, and on query_config failure mark the service as "existed but unknown path" so rollback never blindly `delete_service()`; after restore/delete, best-effort `start_service()`. `Ev::Done` must carry the rollback outcome so the UI never claims a rollback that did not happen.

```rust
//! 安装/修复/启动服务的工作流（后台线程 + 步骤事件 + 失败回滚）。

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};

use crate::service::{self, Credential, InstallRequest};
use crate::setup::{install_dir, SetupArgs};

pub enum Ev { Step(String), StepDone(String), Done(Result<(), String>) }

fn emit(tx: &Sender<Ev>, ev: Ev) { let _ = tx.send(ev); }

/// 从 exe 尾读 payload；未打包时回退到 exe 旁 payload/ 目录（开发态）。
fn load_payload() -> Result<Vec<crate::payload::Entry>> {
    let exe = std::env::current_exe()?;
    let bytes = std::fs::read(&exe)?;
    match crate::payload::unpack(&bytes)? {
        Some(entries) => Ok(entries),
        None => {
            let dir = exe.parent().unwrap_or(std::path::Path::new(".")).join("payload");
            let entries = crate::packaging::collect_dir(&dir)
                .with_context(|| format!("Not packed and no dev payload dir at {}", dir.display()))?;
            Ok(entries)
        }
    }
}

fn kill_tray() {
    use std::os::windows::process::CommandExt as _;
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/IM", "gdut-net.exe"])
        .creation_flags(0x0800_0000)
        .status();
}

/// 安装/修复：停旧服务 → 解包 → install_core → shell 集成 → 起服务。
/// 任一步失败：恢复旧服务路径（或删除新建服务）。
pub fn spawn_install(tx: Sender<Ev>, args: SetupArgs, student_id: String, password: Option<String>) {
    std::thread::Builder::new()
        .name("gdut-net-setup-install".into())
        .spawn(move || run_install(tx, args, student_id, password))
        .expect("Failed to spawn install thread");
}

fn run_install(tx: Sender<Ev>, args: SetupArgs, student_id: String, password: Option<String>) {
    let rollback = || -> Result<()> {
        let prev = match service::install_state() {
            service::InstallState::Installed { service_exe, .. } if !service_exe.as_os_str().is_empty() => Some(service_exe),
            _ => None,
        };
        match prev {
            Some(p) => service::restore_service_path(std::path::Path::new(crate::setup::DATA_DIR).join("config.toml").as_path(), &p),
            None => service::delete_service(),
        }
    };
    // 先记下旧路径（在解包覆盖之前）。
    let prev_exe = match service::install_state() {
        service::InstallState::Installed { service_exe, .. } if !service_exe.as_os_str().is_empty() => Some(service_exe),
        _ => None,
    };

    let result: Result<()> = (|| {
        emit(&tx, Ev::Step("停止旧服务".into()));
        service::stop_service(Duration::from_secs(16))?;
        kill_tray();
        emit(&tx, Ev::StepDone("停止旧服务".into()));

        emit(&tx, Ev::Step("解包文件".into()));
        let dir = install_dir();
        std::fs::create_dir_all(&dir)?;
        for entry in load_payload()? {
            let dest = dir.join(&entry.name);
            std::fs::write(&dest, &entry.data).with_context(|| format!("Failed to write {}", dest.display()))?;
        }
        let self_exe = std::env::current_exe()?;
        std::fs::copy(&self_exe, dir.join("gdut-net-setup.exe"))?;
        emit(&tx, Ev::StepDone("解包文件".into()));

        emit(&tx, Ev::Step("写入配置并注册服务".into()));
        let cfg_path = std::path::PathBuf::from(crate::setup::DATA_DIR).join("config.toml");
        let credential = match password {
            Some(p) => Credential::Plain(p),
            None => Credential::KeepExisting,
        };
        let outcome = service::install_core(InstallRequest {
            cfg_path: cfg_path.clone(),
            student_id: Some(student_id),
            credential,
            service_exe: dir.join("gdut-net.exe"),
            tray_exe: dir.join("gdut-net.exe"),
        })?;
        emit(&tx, Ev::StepDone("写入配置并注册服务".into()));

        emit(&tx, Ev::Step("创建开始菜单快捷方式".into()));
        crate::shell::install_shell_integration(&dir, env!("CARGO_PKG_VERSION"))?;
        emit(&tx, Ev::StepDone("创建开始菜单快捷方式".into()));

        emit(&tx, Ev::Step("启动服务".into()));
        service::start_service()?;
        emit(&tx, Ev::StepDone("启动服务".into()));
        let _ = outcome;
        Ok(())
    })();

    match result {
        Ok(()) => {
            let _ = args;
            emit(&tx, Ev::Done(Ok(())));
        }
        Err(e) => {
            log::error!("Install failed (rolling back): {e:#}");
            if let Err(rb) = rollback_for(prev_exe.as_deref()) {
                log::error!("Rollback failed: {rb:#}");
            }
            emit(&tx, Ev::Done(Err(format!("{e:#}"))));
        }
    }
}

fn rollback_for(prev_exe: Option<&std::path::Path>) -> Result<()> {
    let cfg = std::path::PathBuf::from(crate::setup::DATA_DIR).join("config.toml");
    match prev_exe {
        Some(p) => service::restore_service_path(&cfg, p),
        None => service::delete_service(),
    }
}

/// 读一次服务快照（Done 页显示"已连接 / 重拨中"）。
pub fn query_status_once() -> Result<crate::ipc::protocol::StateSnapshot> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async {
        let mut client = crate::ipc::client::PipeClient::connect()?;
        client.next_state().await
    })
}

/// 启动服务并等待连接（≤25s），结果经 Ev 回报。
pub fn spawn_start_service(tx: Sender<Ev>) {
    std::thread::Builder::new()
        .name("gdut-net-setup-start".into())
        .spawn(move || {
            emit(&tx, Ev::Step("启动 gdut-net 服务".into()));
            let result: Result<()> = (|| {
                service::start_service()?;
                emit(&tx, Ev::StepDone("启动 gdut-net 服务".into()));
                emit(&tx, Ev::Step("等待拨号结果".into()));
                let deadline = Instant::now() + Duration::from_secs(25);
                loop {
                    if let Ok(s) = query_status_once() {
                        use crate::ipc::protocol::SessionStatus::*;
                        if matches!(s.status, Connected | Backoff | AuthFail) {
                            emit(&tx, Ev::StepDone(format!("等待拨号结果（{}）", s.status_text())));
                            return Ok(());
                        }
                    }
                    if Instant::now() >= deadline {
                        bail!("25s 内未读到服务状态");
                    }
                    std::thread::sleep(Duration::from_millis(800));
                }
            })();
            match result {
                Ok(()) => emit(&tx, Ev::Done(Ok(()))),
                Err(e) => emit(&tx, Ev::Done(Err(format!("{e:#}")))),
            }
        })
        .expect("Failed to spawn start-service thread");
}
```
Clean up the duplicated `rollback` closure (keep only `prev_exe` + `rollback_for`); the plan shows both deliberately — the executor keeps `rollback_for` and deletes the unused closure.

- [ ] **Step 3: Extend `src/setup/ui.rs` pages.** Replace the placeholder match arms:

```rust
// SetupApp 新增字段
pub(crate) student_id: String,
pub(crate) password: String,
pub(crate) keep_existing: bool,
pub(crate) has_existing: Option<String>,
pub(crate) rx: Option<std::sync::mpsc::Receiver<work::Ev>>,
pub(crate) steps: Vec<(String, bool)>,
pub(crate) result: Option<Result<(), String>>,
pub(crate) status_line: Option<String>,
```
Account page behavior (Chinese):
- 学号 `TextEdit::singleline`; if `has_existing` is Some, prefill 学号 and show `Checkbox` "使用现有密码"（默认勾选），勾选时密码框置灰。
- 校验：学号非空；未勾选时密码非空。点击"开始安装"→ 创建 channel → `work::spawn_install(tx, args, student_id, if keep { None } else { Some(password) })` → `page = Progress`。
- `SetupApp::new` 里 `has_existing = service::existing_account(&cfg_path)`；`keep_existing = has_existing.is_some()`。
Progress page each frame:
```rust
if let Some(rx) = &self.rx {
    while let Ok(ev) = rx.try_recv() {
        match ev {
            work::Ev::Step(label) => self.steps.push((label, false)),
            work::Ev::StepDone(label) => {
                // 标记同名步骤完成；带括号补充文本的按前缀匹配
                for (l, done) in self.steps.iter_mut().rev() {
                    if label.starts_with(l.as_str()) { *done = true; *l = label.clone(); break; }
                }
            }
            work::Ev::Done(result) => {
                self.result = Some(result);
                self.page = Page::Done;
            }
        }
    }
}
```
Done page: Ok → spawn a thread polling `work::query_status_once()` ≤20s into a channel / or simply show "服务已启动" with buttons: "打开 GDUT Net"（`Command::new(install_dir().join("gdut-net.exe")).arg("tray").spawn()`）and "关闭". Err → red error text + "打开日志" button + "重试"（back to Account）。
Maintenance page buttons: "修复安装"（→ Account with `keep_existing = has_existing.is_some()`）、"卸载"（→ UninstallConfirm, Task 9）、位置与版本展示、`egui::Button` 关闭。
StartService page: auto `work::spawn_start_service` on first frame; steps like Progress; Done → 成功/失败文本 + 关闭。
Welcome → Account.

- [ ] **Step 4: Cross-compile gates** — three commands.

- [ ] **Step 5: Real-machine UI smoke (no install).** With the user told, stage `gdut-net-setup.exe` + a `payload/` dir (cross-built `gdut-net.exe`, the 7 bats, `说明.txt`) and run it: UAC prompt appears, window opens on the Maintenance page (service already present), Chinese renders (no tofu), "修复安装" reaches the account page with 学号 prefilled and 使用现有密码 checked, inputs work; close without installing. **Do not complete the install here** (ruling R3): the first real install happens in Task 9's silent check, and the full machine migration is Task 12.

- [ ] **Step 6: Commit**

```bash
git add src/setup/work.rs src/setup/ui.rs src/service.rs
git commit -m "feat(setup): install/repair/start-service workflow + wizard pages"
```

---

### Task 9: Uninstall page + `--silent` mode

**Files:**
- Modify: `src/setup/ui.rs` (UninstallConfirm page + uninstall worker)
- Replace: `src/setup/silent.rs` (real implementation)
- Modify: `src/setup/work.rs` (spawn_uninstall)

**Interfaces:**
- Produces:
```rust
pub fn spawn_uninstall(tx: std::sync::mpsc::Sender<Ev>, purge: bool, remove_dir: bool);
pub fn run(args: &SetupArgs, mode: Mode) -> anyhow::Result<()>;  // silent, English output
```

- [ ] **Step 1: `work::spawn_uninstall`:** worker: step "停止服务"（`stop_service(16s)` + kill_tray）→ "移除服务与集成"（`service::uninstall_core(&cfg, purge)`）→ "移除安装目录"（`shell::schedule_install_dir_removal(&install_dir())`，仅 remove_dir=true）→ `Done`。Uninstall 失败不回滚（幂等可重试），日志记录。R7 形状：`Ev::Done { result, rollback: RollbackOutcome::NotNeeded }`（卸载无回滚概念）；UI/silent 的 Done 匹配沿用 Task 8 的 `{ result, rollback }` 形状。

- [ ] **Step 2: `silent.rs`:**

```rust
//! --silent（英文输出，供迁移脚本/高级用户）：安装走 KeepExisting；卸载幂等。

use anyhow::{bail, Result};

use super::{install_dir, work, Mode, SetupArgs};

pub fn run(args: &SetupArgs, mode: Mode) -> Result<()> {
    match mode {
        Mode::SilentInstall => {
            if !args.keep_password {
                bail!("Silent install requires --keep-password (no interactive input available)");
            }
            let (tx, rx) = std::sync::mpsc::channel();
            work::spawn_install(tx, args.clone(), String::new(), None);
            let mut ok = false;
            while let Ok(ev) = rx.recv() {
                match ev {
                    work::Ev::Step(s) => println!("== {s}"),
                    work::Ev::StepDone(s) => println!("ok {s}"),
                    // R7：Ev::Done 携带回滚实情，silent 必须照实打印（英文）。
                    work::Ev::Done { result: Err(e), rollback } => {
                        eprintln!("FAILED: {e}");
                        match rollback {
                            work::RollbackOutcome::NotNeeded => {}
                            work::RollbackOutcome::Restored => eprintln!("Rolled back to the previous service."),
                            work::RollbackOutcome::RestoredUnknown => eprintln!("Service existed but its path was unreadable; left untouched."),
                            work::RollbackOutcome::Failed(r) => eprintln!("ROLLBACK FAILED: {r}"),
                        }
                        std::process::exit(1);
                    }
                    work::Ev::Done { result: Ok(()), .. } => { ok = true; break; }
                }
            }
            if ok { println!("Install complete: {}", install_dir().display()); }
            Ok(())
        }
        Mode::SilentUninstall => {
            let cfg = std::path::PathBuf::from(super::DATA_DIR).join("config.toml");
            crate::service::stop_service(std::time::Duration::from_secs(16))?;
            crate::service::uninstall_core(&cfg, args.purge)?;
            crate::shell::schedule_install_dir_removal(&install_dir())?;
            println!("Uninstall complete (purge={})", args.purge);
            Ok(())
        }
        _ => unreachable!("silent::run called with mode {mode:?}"),
    }
}
```
Note: with empty `student_id` in silent install, `install_core` must keep the existing ID — `student_id: Some(String::new())` is ignored by `install_core` (only non-empty replaces), and if config has no ID it bails. That matches migration semantics.

- [ ] **Step 3: UninstallConfirm page UI:** purge checkbox "同时删除配置与日志（含学号密码、拨号记录）", warning text, red "卸载" button → `work::spawn_uninstall(tx, purge, true)` + Progress page; "取消" → back to Maintenance / close.

- [ ] **Step 4: Cross-compile gates + real-machine silent checks** (this is the first real install onto the dev box; service moves from the Desktop kit to Program Files — the Desktop kit stays as fallback):
```
gdut-net-setup.exe --silent --keep-password      (elevated) → "Install complete", exit 0
gdut-net-setup.exe --silent --uninstall          (elevated) → exit 0, service gone, shortcuts gone
gdut-net-setup.exe --silent --keep-password      (elevated) → reinstall so the machine stays in the new layout
```
**Never** run `--purge` on this machine: it deletes config + logs and the DPAPI password blob would be gone, requiring the plaintext password again. Silent uninstall testing is always without `--purge`.

- [ ] **Step 5: Commit**

```bash
git add src/setup/silent.rs src/setup/work.rs src/setup/ui.rs
git commit -m "feat(setup): uninstall page + silent mode for migration scripts"
```

---

### Task 10: Tray single-instance, left-click opens GUI, persistent window

**Files:**
- Modify: `src/logging.rs` (add `init_tray_logging`)
- Modify: `src/tray/mod.rs` (singleton mutex, wake event, left-click, menu-on-left-click off)
- Modify: `src/tray/panel.rs` → rename to `src/tray/gui.rs`; rewrite for persistence
- Modify: `src/cli.rs` (no-arg double-click entry; `run_tray(false)`)
- Modify: `src/setup/ui.rs` (init setup file logging)

**Interfaces:**
- Produces:
```rust
// src/logging.rs
pub fn init_tray_logging(log_dir: &str, basename: &str);
// src/tray/mod.rs
pub fn run_tray(show_gui_at_start: bool) -> anyhow::Result<()>;
pub fn has_console() -> bool;
pub fn double_click_entry() -> anyhow::Result<()>;
// src/tray/gui.rs
pub(crate) type GuiShared = Arc<Mutex<Option<egui::Context>>>;
pub(crate) fn show_or_focus(GuiShared, SharedSnapshot, Sender<()>, Sender<NetMode>);
```

**Verified API facts** (against vendored sources): `CreateMutexW -> Result<HANDLE>`; "already exists" detection via `GetLastError() == ERROR_ALREADY_EXISTS` (183); `CreateEventW(None, false, false, name)` auto-reset; `OpenEventW(EVENT_MODIFY_STATE, false, name)`; `GetConsoleWindow()` in `Win32_System_Console`; `egui::ViewportCommand::{Focus, CancelClose, Visible(bool)}` and `ViewportInfo::close_requested()` all exist in 0.36.2.

- [ ] **Step 1: Add `init_tray_logging` to `src/logging.rs`:**

```rust
/// 托盘 / 安装器 GUI 进程无 stderr：写 ProgramData 文件日志。
/// 失败静默（最坏情况无日志，不阻断 UI 启动）。
pub fn init_tray_logging(log_dir: &str, basename: &str) {
    let _ = std::fs::create_dir_all(log_dir);
    let result = Logger::try_with_str("info").map(|logger| {
        logger
            .log_to_file(FileSpec::default().directory(log_dir).basename(basename))
            .append()
            .rotate(
                Criterion::Size(5 * 1024 * 1024),
                Naming::Timestamps,
                Cleanup::KeepLogFiles(2),
            )
            .start()
    });
    if let Err(e) = result {
        eprintln!("Failed to init {basename} logging (continuing): {e:#}");
    }
}
```
(Naming/Cleanup/Criterion already imported in logging.rs.)

- [ ] **Step 2: Tray singleton + wake + left-click in `src/tray/mod.rs`.** Add near the top of the module:

```rust
const SINGLETON_NAME: &str = "gdut-net-tray-singleton";
const SHOW_EVENT_NAME: &str = "gdut-net-tray-show";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

enum Singleton {
    /// 首个实例：持有内核 mutex，进程退出自动释放。
    Primary(windows::Win32::Foundation::HANDLE),
    /// 已有托盘在跑：唤醒它的 GUI 后本进程退出。
    Secondary,
}

/// 单实例守卫；mutex 创建失败时降级为 Primary（无保护，不阻断托盘启动）。
fn acquire_singleton() -> Singleton {
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
    use windows::Win32::System::Threading::CreateMutexW;

    let name = wide(SINGLETON_NAME);
    match unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) } {
        Ok(h) => {
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                let _ = unsafe { CloseHandle(h) };
                Singleton::Secondary
            } else {
                Singleton::Primary(h)
            }
        }
        Err(e) => {
            log::warn!("Tray singleton mutex failed (continuing unguarded): {e}");
            Singleton::Primary(HANDLE::default())
        }
    }
}

/// 二次启动：置位命名事件，跨进程唤醒主实例显示 GUI。
fn signal_show() {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{CreateEventW, SetEvent};

    let name = wide(SHOW_EVENT_NAME);
    if let Ok(h) = unsafe { CreateEventW(None, false, false, PCWSTR(name.as_ptr())) } {
        let _ = unsafe { SetEvent(h) };
        let _ = unsafe { CloseHandle(h) };
    }
}

/// 从 cmd/PowerShell 启动时有控制台窗口；双击（windows 子系统）没有。
pub fn has_console() -> bool {
    !unsafe { windows::Win32::System::Console::GetConsoleWindow() }
        .0
        .is_null()
}

/// 双击 exe（无参数、无控制台）入口：已安装 → 托盘 + 弹 GUI；未安装 → 中文提示。
pub fn double_click_entry() -> Result<()> {
    match crate::service::install_state() {
        crate::service::InstallState::Installed { .. } => run_tray(true),
        crate::service::InstallState::NotInstalled => {
            message_box_install_hint();
            Ok(())
        }
    }
}

fn message_box_install_hint() {
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};

    let text = wide("gdut-net 尚未安装。\n\n请运行安装包 gdut-net-setup.exe，或从开始菜单打开安装程序。");
    let title = wide("GDUT Net");
    unsafe {
        MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONINFORMATION);
    }
}

fn show_gui(
    gui: &GuiShared,
    snapshot: &SharedSnapshot,
    redial_tx: &mpsc::Sender<()>,
    setmode_tx: &mpsc::Sender<NetMode>,
) {
    gui::show_or_focus(
        Arc::clone(gui),
        Arc::clone(snapshot),
        redial_tx.clone(),
        setmode_tx.clone(),
    );
}
```

Change `run_tray` head and body:

```rust
pub fn run_tray(show_gui_at_start: bool) -> Result<()> {
    let _singleton = match acquire_singleton() {
        Singleton::Primary(h) => h,
        Singleton::Secondary => {
            signal_show();
            return Ok(());
        }
    };
    crate::logging::init_tray_logging(r"C:\ProgramData\gdut-net\logs", "tray");
    register_aumid();

    let snapshot: SharedSnapshot = Arc::new(Mutex::new(None));
    // ... 菜单/图标构建与现状完全一致，仅两处改动：
    let panel_item = MenuItem::new("Details", true, None);   // Task 11 改中文
    // tray 构建：.with_menu_on_left_click(false)（左键留给 GUI，右键弹菜单）
    let tray = tray_icon::TrayIconBuilder::new()
        .with_tooltip("gdut-net — Wired: Disconnected / WiFi: Off")
        .with_icon(...)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .map_err(|e| anyhow!("Failed to create tray icon: {e}"))?;
    // ... 通道与 IPC 线程不变，其后：
    let wake_event = {
        let name = wide(SHOW_EVENT_NAME);
        unsafe { CreateEventW(None, false, false, PCWSTR(name.as_ptr())) }
            .context("Failed to create tray show event")?
    };
    let gui: GuiShared = Arc::new(Mutex::new(None));
    if show_gui_at_start {
        show_gui(&gui, &snapshot, &panel_redial_tx, &panel_setmode_tx);
    }

    let menu_rx = MenuEvent::receiver();
    let tray_rx = tray_icon::TrayIconEvent::receiver();

    loop {
        let pump = pump_once(Some(Duration::from_millis(200)), Some(&wake_event))?;
        if pump.processed {
            continue;
        }
        if pump.woke {
            log::info!("Show-GUI request received");
            show_gui(&gui, &snapshot, &panel_redial_tx, &panel_setmode_tx);
        }
        // 左键单击托盘 → 打开日常 GUI（菜单已改为只右键弹）。
        while let Ok(ev) = tray_rx.try_recv() {
            if let tray_icon::TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Up,
                ..
            } = ev
            {
                show_gui(&gui, &snapshot, &panel_redial_tx, &panel_setmode_tx);
            }
        }
        while let Ok(event) = menu_rx.try_recv() {
            if event.id == *mode_exclusive.id() {
                send_set_mode(NetMode::WiredExclusive);
            } else if event.id == *mode_standby.id() {
                send_set_mode(NetMode::WiredPlusStandby);
            } else if event.id == *redial_item.id() {
                send_redial();
            } else if event.id == *panel_item.id() {
                show_gui(&gui, &snapshot, &panel_redial_tx, &panel_setmode_tx);
            } else if event.id == *quit_item.id() {
                std::process::exit(0);
            }
        }
        // ... 其余（面板通道 / 状态文本 / 图标刷新）保持不变 ...
    }
}
```

`pump_once` signature change (call sites: this loop only):

```rust
/// 一拍泵结果：`processed` = 处理过消息（应 continue）；`woke` = 命名事件触发。
struct Pump {
    processed: bool,
    woke: bool,
}

/// 跑一拍 win32 消息泵。`wake` 须为自动复位事件句柄。
fn pump_once(timeout: Option<Duration>, wake: Option<&HANDLE>) -> Result<Pump> {
    use windows::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage,
        MSG, PM_REMOVE, QS_ALLINPUT,
    };

    let mut processed = false;
    loop {
        let mut msg = MSG::default();
        let has_msg = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool();
        if !has_msg {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        processed = true;
    }
    if processed {
        return Ok(Pump { processed: true, woke: false });
    }
    match timeout {
        None => {
            // ... 与现状相同：GetMessageW 阻塞，处理一条消息后
            Ok(Pump { processed: true, woke: false })
        }
        Some(d) => {
            let timeout_ms = u32::try_from(d.as_millis()).unwrap_or(INFINITE - 1);
            let handles = wake.map(std::slice::from_ref);
            let res = unsafe { MsgWaitForMultipleObjects(handles, false, timeout_ms, QS_ALLINPUT) };
            if res == WAIT_TIMEOUT {
                return Ok(Pump { processed: false, woke: false });
            }
            // 句柄下标 0 = wake 事件（自动复位）；其余为输入待排空。
            Ok(Pump { processed: false, woke: res == WAIT_OBJECT_0 })
        }
    }
}
```
Add `use windows::Win32::Foundation::HANDLE;` and `use windows::Win32::System::Threading::CreateEventW;` to the module imports (pump signature uses `HANDLE`).

- [ ] **Step 3: Create `src/tray/gui.rs`** — move `panel.rs` here and make it persistent. Delete `src/tray/panel.rs`, change `mod panel;` → `mod gui;` in `src/tray/mod.rs`. Header and structure:

```rust
//! 日常状态窗口（常驻）：首次点开创建；关窗 = 隐藏；左键托盘唤出。
//! egui/eframe glow（ADR-0006）；快照每帧拉取，操作经 mpsc 回泵线程发 IPC。

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use eframe::egui::{CentralPanel, Grid, ScrollArea, ViewportBuilder};
use eframe::{NativeOptions, Renderer};

use crate::ipc::protocol::{NetMode, StateSnapshot};

use super::SharedSnapshot;

/// GUI 的 egui 上下文句柄：Some = 窗口线程活着（可能处于隐藏）。
pub(crate) type GuiShared = Arc<Mutex<Option<egui::Context>>>;

/// 显示或创建窗口；已存在则显示 + 聚焦。
pub(crate) fn show_or_focus(
    shared: GuiShared,
    snapshot: SharedSnapshot,
    redial_tx: Sender<()>,
    setmode_tx: Sender<NetMode>,
) {
    if let Some(ctx) = shared.lock().ok().and_then(|g| g.clone()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
        return;
    }
    let shared2 = Arc::clone(&shared);
    let _ = std::thread::Builder::new()
        .name("gdut-net-gui".into())
        .spawn(move || {
            if let Err(e) = run_window(Arc::clone(&shared2), snapshot, redial_tx, setmode_tx) {
                log::error!("GUI window exited: {e:#}");
            }
            if let Ok(mut g) = shared2.lock() {
                *g = None; // 线程退出后允许下次点击重建
            }
        });
}

fn run_window(
    shared: GuiShared,
    snapshot: SharedSnapshot,
    redial_tx: Sender<()>,
    setmode_tx: Sender<NetMode>,
) -> anyhow::Result<()> {
    let mut options = NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("GDUT Net")
            .with_inner_size(egui::vec2(460.0, 540.0))
            .with_resizable(false),
        renderer: Renderer::Glow,
        ..Default::default()
    };
    options.event_loop_builder = Some(Box::new(|builder| {
        use winit::platform::windows::EventLoopBuilderExtWindows as _;
        builder.with_any_thread(true);
    }));
    eframe::run_native(
        "gdut-net-gui",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::light());
            if let Ok(mut g) = shared.lock() {
                *g = Some(cc.egui_ctx.clone());
            }
            Ok(Box::new(Gui { snapshot, redial_tx, setmode_tx }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe failed: {e}"))
}

struct Gui {
    snapshot: SharedSnapshot,
    redial_tx: Sender<()>,
    setmode_tx: Sender<NetMode>,
}

impl eframe::App for Gui {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // 关窗 = 隐藏：取消关闭、窗口留活；托盘左键再唤出。
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        ctx.request_repaint_after(Duration::from_millis(500));
        CentralPanel::default().show(ui, |ui| {
            // Task 10 先原样保留旧面板内容（自 panel.rs 搬迁，字段相同）；
            // Task 11 换成锁定视觉方向的中文界面。
        });
    }
}
```
Move the old panel body inside `CentralPanel` verbatim (reads `self.snapshot`, sends via `self.redial_tx` / `self.setmode_tx`).

- [ ] **Step 4: `src/cli.rs`** — no-arg entry before clap parse, and tray call sites:

```rust
pub fn dispatch() -> Result<()> {
    // 双击 exe（无参数、无控制台）：已安装 → 托盘 + GUI；未安装 → 中文提示。
    #[cfg(windows)]
    if std::env::args().len() == 1 && !crate::tray::has_console() {
        return crate::tray::double_click_entry();
    }
    // ... 现有 SetConsoleOutputCP / clap / logger 保持不变 ...
    #[cfg(windows)]
    Cmd::Tray => crate::tray::run_tray(false),
```
(`Cmd::Run` 的 `--arg` 解析不受影响：服务由 SCM 以参数启动，args.len() > 1。)

- [ ] **Step 5: `src/setup/ui.rs`** — first line of `run()`:

```rust
    crate::logging::init_tray_logging(r"C:\ProgramData\gdut-net\logs", "setup");
```

- [ ] **Step 6: Cross-compile gates** — `cargo check --target x86_64-pc-windows-msvc && cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings && cargo fmt`.

- [ ] **Step 7: Real-machine checks** (stage the new exe; the machine is already installed in Program Files after Task 8):
  - 双击 `gdut-net.exe` → 托盘出现 + GUI 窗口弹出（暂无控制台黑窗）。
  - 点窗口 X → 窗口隐藏、托盘仍在；左键托盘 → 窗口复现。
  - 右键托盘 → 右键菜单仍弹出；菜单 "Details" → 窗口复现。
  - 托盘已在跑时再双击 exe → 不出现第二个图标，窗口被唤到前台。
  - `C:\ProgramData\gdut-net\logs\tray_r*.log` 有日志行。
  - 退出托盘（右键退出）后双击 exe 能重新起托盘。

- [ ] **Step 8: Commit**

```bash
git add src/logging.rs src/tray src/cli.rs src/setup/ui.rs
git commit -m "feat(tray): single instance, left-click opens persistent GUI window, file logging"
```

---

### Task 11: Daily GUI content (Chinese, locked direction) + tray menu

**Files:**
- Modify: `src/tray/gui.rs` (replace placeholder body with the full Chinese GUI)
- Modify: `src/tray/mod.rs` (Chinese menu labels + Chinese status line/tooltip)

**Interfaces:**
- Consumes: Task 1 surface brief for `src/tray/gui.rs` (direction contract), `crate::fonts::install_cjk_fonts`, `crate::setup::install_dir`, `crate::config::Config`, `crate::ipc::protocol::{HeartbeatStatus, SessionStatus, WPhase}`.

- [ ] **Step 1: Read the locked surface brief** for `src/tray/gui.rs` and `reference/craft-floor.md`, then implement its FIRST VIEWPORT composition. The code below is the functional skeleton; the brief's OWN-WORLD (palette/components) wins wherever they disagree on presentation, never on behavior.

- [ ] **Step 2: Replace `gui.rs` content** (keep `show_or_focus` / `run_window` plumbing from Task 10):

```rust
const CFG_PATH: &str = r"C:\ProgramData\gdut-net\config.toml";
const LOG_DIR: &str = r"C:\ProgramData\gdut-net\logs";

fn run_window(...) -> anyhow::Result<()> {
    // ... NativeOptions 同 Task 10 ...
    Box::new(move |cc| {
        cc.egui_ctx.set_visuals(egui::Visuals::light());
        if let Err(e) = crate::fonts::install_cjk_fonts(&cc.egui_ctx) {
            log::error!("Failed to load CJK fonts: {e:#}");
        }
        if let Ok(mut g) = shared.lock() { *g = Some(cc.egui_ctx.clone()); }
        Ok(Box::new(Gui {
            snapshot,
            redial_tx,
            setmode_tx,
            student_id: load_student_id(),
            cfg_mtime: config_mtime(),
        }))
    })
}

/// 学号：空值显示 "—"。
fn load_student_id() -> Option<String> {
    crate::config::Config::load(std::path::Path::new(CFG_PATH))
        .ok()
        .map(|c| c.account.student_id)
        .filter(|s| !s.trim().is_empty())
}

fn config_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(CFG_PATH).and_then(|m| m.modified()).ok()
}

struct Gui {
    snapshot: SharedSnapshot,
    redial_tx: Sender<()>,
    setmode_tx: Sender<NetMode>,
    student_id: Option<String>,
    cfg_mtime: Option<std::time::SystemTime>,
}

impl eframe::App for Gui {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        ctx.request_repaint_after(Duration::from_millis(500));
        // 修改账号密码后配置变了：mtime 变化才重读（ruling R4）。
        let mtime = config_mtime();
        if mtime != self.cfg_mtime {
            self.student_id = load_student_id();
            self.cfg_mtime = mtime;
        }
        CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("GDUT Net");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(format!("v{}", env!("CARGO_PKG_VERSION")));
                });
            });
            let snap = self.snapshot.lock().ok().and_then(|g| g.clone());
            match snap.as_ref() {
                None => service_down_ui(ui),
                Some(s) => status_ui(ui, s, &self.redial_tx, &self.setmode_tx, self.student_id.as_deref()),
            }
        });
    }
}

fn service_down_ui(ui: &mut egui::Ui) {
    ui.add_space(28.0);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new("服务未运行").size(28.0).strong());
        ui.add_space(4.0);
        ui.label("后台服务没有运行，网络不会自动拨号。");
        ui.add_space(12.0);
        if ui.button("启动服务").clicked() { launch_setup(&["--start-service"]); }
        if ui.button("打开日志目录").clicked() { open_logs(); }
    });
}

fn status_ui(
    ui: &mut egui::Ui,
    s: &StateSnapshot,
    redial_tx: &Sender<()>,
    setmode_tx: &Sender<NetMode>,
    student_id: Option<&str>,
) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(status_word(s)).size(26.0).strong());
    ui.label(format!("出口：{}", egress_word(s)));
    ui.add_space(8.0);
    egui::Grid::new("status").num_columns(2).spacing([18.0, 6.0]).show(ui, |ui| {
        ui.strong("IP"); ui.label(s.ip.clone().unwrap_or_else(|| "—".into())); ui.end_row();
        ui.strong("在线时长"); ui.label(s.uptime_text()); ui.end_row();
        ui.strong("上次掉线"); ui.label(s.last_drop_reason.clone().unwrap_or_else(|| "—".into())); ui.end_row();
        ui.strong("心跳"); ui.label(match &s.heartbeat {
            HeartbeatStatus::Off => "关闭".to_string(),
            HeartbeatStatus::Running => "运行中".to_string(),
            HeartbeatStatus::Error(e) => format!("错误（{e}）"),
        }); ui.end_row();
        ui.strong("无线"); ui.label(wireless_zh(s)); ui.end_row();
    });
    ui.add_space(10.0);
    if ui.button("立即重拨").clicked() { let _ = redial_tx.send(()); }
    ui.add_space(6.0);
    ui.label(egui::RichText::new("网络模式").strong());
    if ui.radio(s.mode == NetMode::WiredExclusive, "有线优先（拔线自动无线接管）").clicked() {
        let _ = setmode_tx.send(NetMode::WiredExclusive);
    }
    if ui.radio(s.mode == NetMode::WiredPlusStandby, "有线 + 无线备用").clicked() {
        let _ = setmode_tx.send(NetMode::WiredPlusStandby);
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("账号").strong());
        ui.label(format!("学号：{}", student_id.unwrap_or("—")));
        if ui.button("修改账号密码").clicked() { launch_setup(&["--repair"]); }
    });
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("最近事件").strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("打开日志目录").clicked() { open_logs(); }
        });
    });
    ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
        if s.events.is_empty() { ui.weak("（暂无事件）"); }
        for line in s.events.iter().rev() { ui.monospace(line); }
    });
}

fn status_word(s: &StateSnapshot) -> &'static str {
    match s.status {
        SessionStatus::Connected => "已连接",
        SessionStatus::Dialing => "正在拨号",
        SessionStatus::Backoff => "重拨中",
        SessionStatus::AuthFail => "认证失败",
        SessionStatus::Idle => "空闲",
    }
}

fn egress_word(s: &StateSnapshot) -> &'static str {
    if s.status == SessionStatus::Connected { "有线" }
    else if s.wireless.phase == WPhase::Online { "无线" }
    else { "—" }
}

fn wireless_zh(s: &StateSnapshot) -> String {
    let phase = match s.wireless.phase {
        WPhase::Off => "关闭", WPhase::Joining => "连接中", WPhase::Authing => "认证中",
        WPhase::Online => "已接管", WPhase::Error => "错误",
    };
    match (&s.wireless.ip, &s.wireless.last_error) {
        (Some(ip), _) => format!("{phase} {ip}"),
        (None, Some(e)) => format!("{phase}（{e}）"),
        _ => phase.to_string(),
    }
}

/// 改密码 / 启动服务：拉起安装目录的 setup（它自提权，弹一次 UAC）。
fn launch_setup(args: &[&str]) {
    let exe = crate::setup::install_dir().join("gdut-net-setup.exe");
    match std::process::Command::new(&exe).args(args).spawn() {
        Ok(_) => log::info!("Launched setup {args:?}"),
        Err(e) => log::error!("Failed to launch setup {}: {e}", exe.display()),
    }
}

fn open_logs() {
    let dir = std::path::PathBuf::from(LOG_DIR);
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::process::Command::new("explorer.exe").arg(&dir).spawn();
}
```

- [ ] **Step 3: Chinese tray menu** (`src/tray/mod.rs`): menu items become `打开主界面`（原 Details）/ `立即重拨` / `有线优先（自动无线接管）` / `有线 + 无线备用` / `退出托盘`；`status_line()` 改为中文（菜单首行 + tooltip 共用）:

```rust
fn status_line(s: Option<&StateSnapshot>) -> String {
    let Some(s) = s else { return "服务未运行".to_string() };
    let wired = match s.status {
        SessionStatus::Connected => "已连接", SessionStatus::Dialing => "拨号中",
        SessionStatus::Backoff => "重拨中", SessionStatus::AuthFail => "认证失败",
        SessionStatus::Idle => "空闲",
    };
    let wifi = match s.wireless.phase {
        WPhase::Off => "关闭", WPhase::Joining => "连接中", WPhase::Authing => "认证中",
        WPhase::Online => "已接管", WPhase::Error => "错误",
    };
    format!("有线：{wired} · WiFi：{wifi}")
}
```
Tooltip 同步用该函数；CLI 输出（`status` 子命令）继续保持英文，不动 `protocol.rs`。

- [ ] **Step 4: Cross-compile gates** — 同 Task 3 三条命令。

- [ ] **Step 5: Real-machine visual checks.** GUI 中文无方块；每个按钮点一遍：立即重拨（日志出现拨号行为）、模式切 standby（托盘快照回显）、修改账号密码（安装器维护页弹出，UAC 一次）、打开日志目录（explorer 打开）。截图交给 Task 13。**本机注意**：本机 UAC 策略（`ConsentPromptBehaviorAdmin=0` + `EnableLUA=1`）可能让 setup 自提权静默失败；若"修改账号密码"无反应，改从开始菜单的安装程序/计划任务通道验证，并在 CONTEXT 陷阱里补记。

- [ ] **Step 6: Commit**

```bash
git add src/tray
git commit -m "feat(gui): Chinese daily-driver window + tray menu (locked direction)"
```

---

### Task 12: Packaging pipeline, CI/release, author migration

**Files:**
- Modify: `.github/workflows/ci.yml`, `.github/workflows/release.yml`
- Create: `packaging/personal/switch-v4.ps1`, `packaging/personal/rollback.bat`, `packaging/personal/rollback-v4.ps1`, `packaging/personal/一键切换.bat`, `packaging/personal/tun-watch.ps1`
- Create: `packaging/dev/capture-window.ps1` (dev-only; not shipped)

**Interfaces:**
- Consumes: `gdut-net-pack` (Task 4), payload dir (Task 5), setup silent mode (Task 9).

- [ ] **Step 1: `release.yml`** — replace the build/zip/release steps:

```yaml
      - run: cargo build --release
      - run: ./target/release/gdut-net-pack.exe --setup target/release/gdut-net-setup.exe --file target/release/gdut-net.exe --payload-dir packaging/payload --out gdut-net-setup.exe
        shell: pwsh
      - uses: softprops/action-gh-release@v2
        with:
          files: gdut-net-setup.exe
          generate_release_notes: true
```

- [ ] **Step 2: `ci.yml` windows-build** — after `cargo build --release`, add the same pack command and upload `gdut-net-setup.exe` as the artifact (drop the zip step).

- [ ] **Step 3: Personal scripts.** `switch-v4.ps1` is rewritten as migration-aware v5 (runs elevated via the existing `gdut-switch` task, so no UAC on this machine):

> **Ruling R8 (pre-migration, real-machine parse test):** the plan's original `\"`-escaped lines in `switch-v4.ps1` are invalid PowerShell (ParserError → whole script never runs). Committed fix (84f07e9): task repoint via `New-ScheduledTaskAction` + `Set-ScheduledTask` with `schtasks /Query` verification; `RollbackToDesktop` restores by copying the Desktop exe over the installed one (no binPath mutation — the field-proven desktop-kit pattern); silent setup output captured via `& $setup ... 2>&1 | Out-String` + `$LASTEXITCODE` (the wireless-test.bat pattern). The plan snippet below is superseded by `packaging/personal/switch-v4.ps1` as committed.

```powershell
# gdut-net switch v5 -- migrate Desktop kit into Program Files + full switch.
# Runs elevated via the pre-authorized scheduled task gdut-switch.
# Phases: A. silent install (keep password) + personal scripts;
#         B. repoint task to the installed script; C. dial + 75s stability;
#         D. any failure -> service back to the Desktop kit.

$ErrorActionPreference = 'Continue'
$desktop = 'C:\Users\Lemonawa\Desktop\gdut-net'
$install = 'C:\Program Files\gdut-net'
$log     = 'C:\ProgramData\gdut-net\logs\switch-v4.log'
$gdutLog = 'C:\ProgramData\gdut-net\logs\gdut-net_rCURRENT.log'
function Log($m) { "$(Get-Date -Format 'MM-dd HH:mm:ss') $m" | Out-File $log -Append }
function RollbackToDesktop() {
  Log ">>> Rollback: service back to the Desktop kit"
  Stop-Service gdut-net -Force -ErrorAction SilentlyContinue
  Stop-Process -Name gdut-net -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 2
  sc.exe config gdut-net binPath= "\"$desktop\gdut-net.exe\" --config C:\ProgramData\gdut-net\config.toml run" | Out-Null
  sc.exe start gdut-net | Out-Null
  Log "Rollback done"
}

"=== migrate+switch $(Get-Date) ===" | Out-File $log

Log "A. Stop service + tray"
Stop-Service gdut-net -Force -ErrorAction SilentlyContinue
Stop-Process -Name gdut-net -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

Log "A. Silent install into $install (keep existing password)"
$setup = Join-Path $desktop 'gdut-net-setup.exe'
if (-not (Test-Path $setup)) { Log "missing $setup"; exit 1 }
$p = Start-Process -FilePath $setup -ArgumentList '--silent','--keep-password' -Wait -PassThru
Log "setup exit=$($p.ExitCode)"
if ($p.ExitCode -ne 0) { Log "install failed"; RollbackToDesktop; exit 1 }

Log "A. Copy personal ops scripts + backup exe"
$personal = Join-Path $desktop 'personal'
if (Test-Path $personal) { Copy-Item (Join-Path $personal '*') $install -Force }
if (Test-Path (Join-Path $desktop 'gdut-net-bak.exe')) { Copy-Item (Join-Path $desktop 'gdut-net-bak.exe') (Join-Path $install 'gdut-net-bak.exe') -Force }

Log "B. Repoint the gdut-switch task to the installed script"
schtasks /Change /TN gdut-switch /TR "powershell.exe -NoProfile -ExecutionPolicy Bypass -File \"$install\switch-v4.ps1\"" | Out-Null

Log "C. Wait for dial success (max 180s)"
$ok = $false
for ($i = 0; $i -lt 180; $i++) {
  Start-Sleep -Seconds 1
  $recent = Get-Content $gdutLog -Tail 3 -Encoding UTF8 -ErrorAction SilentlyContinue
  if ($recent -match 'Dial succeeded') { $ok = $true; break }
}
if (-not $ok) { Log "no dial in 180s"; Get-Content $gdutLog -Tail 5 -Encoding UTF8 | Out-File $log -Append; RollbackToDesktop; exit 1 }
Log "Dial succeeded"

Log "C. Stability check 75s"
for ($i = 0; $i -lt 25; $i++) {
  Start-Sleep -Seconds 3
  $recent = Get-Content $gdutLog -Tail 4 -Encoding UTF8 -ErrorAction SilentlyContinue
  if ($recent -match 'considered dropped|Probe failed') { Log "unstable"; RollbackToDesktop; exit 1 }
}
$http = curl.exe -s -m 8 -o NUL -w '%{http_code}' http://www.gstatic.com/generate_204 2>&1
Log "SUCCESS: migrated to $install, service stable (http=$http)"
exit 0
```

`rollback-v4.ps1` (new home):

```powershell
# One-click rollback inside Program Files: restore the OLD gdut-net exe
# (not Dr.COM) + restart the service. Self-contained, works offline.
$ErrorActionPreference = 'Continue'
$dir = 'C:\Program Files\gdut-net'
$log = 'C:\ProgramData\gdut-net\logs\rollback-v4.log'
function Log($m) { "$(Get-Date -Format 'MM-dd HH:mm:ss') $m" | Out-File $log -Append }
"=== rollback to old gdut-net $(Get-Date) ===" | Out-File $log

Log "Stopping gdut-net + killing Dr.COM (release PPP port)"
Stop-Service gdut-net -Force -ErrorAction SilentlyContinue
Stop-Process -Name gdut-net -Force -ErrorAction SilentlyContinue
Stop-Process -Name DrMain,DrClient,DrUpdate,DrTray -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 800
rasdial 'Dr.COM' /d 2>$null
rasdial 'gdut' /d 2>$null
Start-Sleep -Seconds 2

Log "Restoring gdut-net-bak.exe -> gdut-net.exe"
Copy-Item (Join-Path $dir 'gdut-net-bak.exe') (Join-Path $dir 'gdut-net.exe') -Force

Log "Starting service"
sc.exe start gdut-net | Out-Null
$ok = $false
for ($i = 0; $i -lt 90; $i++) {
  Start-Sleep -Seconds 2
  $recent = Get-Content 'C:\ProgramData\gdut-net\logs\gdut-net_rCURRENT.log' -Tail 3 -Encoding UTF8 -ErrorAction SilentlyContinue
  if ($recent -match 'Dial succeeded') { $ok = $true; break }
}
if ($ok) { Log "ROLLBACK OK: old gdut-net online" } else { Log "No dial in 180s - last resort: C:\Drcom\DrUpdateClient\DrMain.exe" }
```

`rollback.bat`:
```bat
@echo off
echo Rolling back to old gdut-net ...
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0rollback-v4.ps1"
echo Done. Log: C:\ProgramData\gdut-net\logs\rollback-v4.log
pause
```

`一键切换.bat`:
```bat
@echo off
schtasks /Run /TN gdut-switch
echo Triggered. Check switch-v4.log for progress.
echo.
echo NOTE: a second window will appear while the script runs.
echo       Do NOT close it - closing kills the script mid-way.
echo.
timeout /t 3 /nobreak >nul
powershell -NoProfile -Command "Get-Content 'C:\ProgramData\gdut-net\logs\switch-v4.log' -Tail 20"
pause
```

`tun-watch.ps1`: copy the current Desktop version unchanged (read-only flight recorder; user tool).

- [ ] **Step 4: `packaging/dev/capture-window.ps1`** (dev-only screenshot for the finish review):

```powershell
param([string]$Title = 'GDUT Net', [string]$Out = 'shot.png')
Add-Type @"
using System; using System.Runtime.InteropServices;
public class W {
  [DllImport("user32.dll")] public static extern IntPtr FindWindowW(string c, string n);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  public struct RECT { public int L, T, R, B; }
}
"@
$h = [W]::FindWindowW($null, $Title)
if ($h -eq [IntPtr]::Zero) { Write-Error "window not found: $Title"; exit 1 }
[W]::SetForegroundWindow($h) | Out-Null
Start-Sleep -Milliseconds 500
$r = New-Object W+RECT
[W]::GetWindowRect($h, [ref]$r) | Out-Null
$w = $r.R - $r.L; $hh = $r.B - $r.T
Add-Type -AssemblyName System.Drawing
$bmp = New-Object System.Drawing.Bitmap($w, $hh)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size)
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
Write-Output "saved $Out ($w x $hh)"
```

- [ ] **Step 5: Local build + stage for migration (agent, from the repo).**

```bash
mise exec -- cargo xwin build --target x86_64-pc-windows-msvc --release
cargo build --release --bin gdut-net-pack
mkdir -p dist
./target/release/gdut-net-pack \
  --setup target/x86_64-pc-windows-msvc/release/gdut-net-setup.exe \
  --file target/x86_64-pc-windows-msvc/release/gdut-net.exe \
  --payload-dir packaging/payload \
  --out dist/gdut-net-setup.exe
```
Stage to the Desktop kit (verify each copy):
- `dist/gdut-net-setup.exe` → `C:\Users\Lemonawa\Desktop\gdut-net\gdut-net-setup.exe`
- `packaging/personal/*` → `C:\Users\Lemonawa\Desktop\gdut-net\personal\` (name it `personal` so the script finds it)
- overwrite `C:\Users\Lemonawa\Desktop\gdut-net\switch-v4.ps1` and `一键切换.bat` with the new versions.

- [ ] **Step 6: User runs `一键切换.bat`** (double-click; no UAC on this machine). Wait for the second window to finish on its own; then verify:
  - `sc.exe qc gdut-net` → BINARY_PATH_NAME under `C:\Program Files\gdut-net`.
  - `reg query "HKCU\...\Run" /v gdut-net-tray` → points to install dir.
  - Start Menu `%ProgramData%\Microsoft\Windows\Start Menu\Programs\GDUT Net\` has 10 `.lnk`s.
  - `schtasks /Query /TN gdut-switch /V /FO LIST` → Task To Run = install dir script.
  - switch log tail: `SUCCESS: migrated ...`.
  - Tray running from install dir (`Get-Process gdut-net | Select Path`).
  - Desktop folder untouched (still the fallback).

- [ ] **Step 7: Commit.**

```bash
git add .github packaging
git commit -m "feat: setup-based release pipeline + personal migration scripts"
```

---

### Task 13: Docs, acceptance, finish review, DESIGN.md

**Files:**
- Modify: `README.md`, `AGENTS.md`, `CONTEXT.md`, `docs/desktop-kit.md`, `docs/acceptance.md`, `PRODUCT.md`
- Create: `docs/adr/0007-installer-and-daily-gui.md`
- Create: `.impeccable/review/*.png` (screenshots), then DESIGN.md via the impeccable documenter

- [ ] **Step 1: `README.md`** — 安装节改为：从 Releases 下载 `gdut-net-setup.exe` → 右键以管理员运行（或双击，安装器自提权）→ 向导输入学号密码；卸载=开始菜单"卸载 GDUT Net"或"应用和功能"。补充：修复/改密 = 重跑安装器或开始菜单安装程序；内置无界面模式 `gdut-net-setup.exe --silent --keep-password`（迁移脚本用，英文输出）。CLI 一节保留，注明面向高级用户。构建一节加两个 bin 与打包命令。

- [ ] **Step 2: `AGENTS.md`** — 架构节加 `src/bin/gdut-net-setup.rs` 与 `gdut-net-pack`、`src/setup/`、`src/tray/gui.rs`、`src/shell.rs`、`src/payload.rs` 与 `packaging/`；命令节加本地打包命令；真机操作节把桌面工具包描述换成安装目录布局（个人脚本在 `C:\Program Files\gdut-net`）。

- [ ] **Step 3: `CONTEXT.md`** — 语言规则改写："CLI/日志/脚本回显英文（GBK 控制台）；GUI（安装器/日常窗口/托盘菜单/说明.txt）中文"。桌面工具包章节改写为新布局；新增坑位记录：命名管道 `gdut-net-tray-show` 与单实例 mutex、`tray_r*.log`/`setup_r*.log`、setup 载荷容器（magic/footer）。

- [ ] **Step 4: `docs/desktop-kit.md`** 重写为安装形态：入口在开始菜单、个人脚本在安装目录、迁移完成后的回滚链（rollback.bat → DrMain）。

- [ ] **Step 5: `docs/acceptance.md`** 新增验收条目（照 spec §10）：零键盘安装；修复（保留/改密）；卸载两档 + 残留检查；10 个开始菜单项；开机（无窗 + 服务 + 托盘 + GUI）；管道安装不回归（`switch` 依赖）；`--keep-password`；双击唤出与单实例；GUI 中文无方块；关窗=隐藏；托盘左键/右键行为。

- [ ] **Step 6: ADR-0007** — 内容按 spec §1–§5 提炼（决策：单文件 setup + 载荷容器；Program Files + 开始菜单 + 应用和功能；中文 GUI/英文控制台；单实例 + 左键开 GUI；`--keep-password`；个人脚本与产品脚本同居安装目录）。

- [ ] **Step 7: `PRODUCT.md`** — Operating Context：安装改为 setup 向导、发布物单文件；Brand Commitments 语言规则改为"控制台/日志英文；GUI 中文"；Uninstall/repair 描述更新。

- [ ] **Step 8: Screenshots** for the finish review (native platform: no detector). On the real machine: open the daily GUI → `capture-window.ps1 -Title 'GDUT Net' -Out .impeccable/review/gui.png`; stop service → `gui-service-down.png`; open installer → `setup-welcome.png`, `setup-account.png`, `setup-done.png`, `setup-maintenance.png`. Copy PNGs back to the repo workspace.

- [ ] **Step 9: Finish review** — spawn the shipped `impeccable-finish-reviewer` with: original request, confirmed answers, artifact paths (`src/tray/gui.rs`, `src/setup/ui.rs`), screenshot paths, direction contracts, craft-floor path, and one line stating the platform is native Windows (egui) so no web detector runs; the reviewer judges against the locked direction + craft floor. Act on its disposition (ship/fix/rebuild). Max two fix rounds; then stop.

- [ ] **Step 10: DESIGN.md** — spawn the shipped `impeccable-documenter` with project root, artifact paths, direction contracts, PRODUCT.md, and write boundary; verify `DESIGN.md` + `.impeccable/design.json` exist and describe the built world (tokens from the actual implementation).

- [ ] **Step 11: Final gates + commit.**

```bash
cargo test && cargo clippy -- -D warnings && cargo fmt --check
cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings
git add -A && git commit -m "docs: installer + GUI docs, ADR-0007, DESIGN.md"
```

---

## Self-Review

**Spec coverage:** §3 发布物 → Task 2/4/12；§4 布局/开始菜单/卸载项 → Task 7（10 快捷方式逐条列出）；§5 向导 4 屏/维护/silent/自提权/字体 → Task 6/8/9；§6 GUI 生命周期/单实例/菜单/日志 → Task 10，内容 → Task 11；§7 脚本 → Task 5 + 12；§8 迁移 → Task 12（含"用户点一次 一键切换.bat"的迁移通道）；§9 错误回滚 → Task 8（rollback_for）/7（幂等）/10（单实例）逐条落位；§10 测试 → Task 2/3/4/5（纯逻辑）+ Task 13（真机验收）；§11 文档/视觉 → Task 1/11/13；§12 不可变项 → Task 3 的 CLI 薄壳与全局约束。无缺口。

**Placeholder scan:** 无 TBD/TODO；每个代码步骤都给出可粘贴代码或"搬迁现有函数"的精确来源；Task 8 的重复闭包明确要求删一留一。

**Type consistency:** `InstallRequest { cfg_path, student_id, credential, service_exe, tray_exe }` 在 Task 3 定义、Task 8 使用一致；`Ev::{Step, StepDone, Done}` 在 Task 8 定义、Task 9 使用一致；`run_tray(bool)` 在 Task 10 改签名并同步 Task 10 Step 4 的调用点；`ShellIntegration` 无遗留（Task 7 只导出四个函数）。

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-09-10-installer-gui.md`.
Two execution options:

1. **Subagent-Driven (recommended)** — fresh subagent per task, review between tasks.
2. **Inline Execution** — execute tasks in this session with checkpoints.

Task 1 (impeccable visual direction) and Task 12 Step 6 (user runs `一键切换.bat`) are user-interactive; everything else is agent-executable.
