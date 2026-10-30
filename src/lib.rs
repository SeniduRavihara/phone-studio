use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const OUTPUT_LIMIT: usize = 2 * 1024 * 1024;
pub type Cancel = Arc<AtomicBool>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transport {
    Adb,
    Fastboot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub serial: String,
    pub transport: Transport,
    pub state: String,
    pub model: String,
}

impl Device {
    pub fn ready(&self) -> bool {
        self.state == "device" || self.state == "fastboot"
    }
    pub fn label(&self) -> String {
        if self.model.is_empty() {
            self.serial.clone()
        } else {
            self.model.replace('_', " ")
        }
    }
    pub fn connection_help(&self) -> &str {
        match self.state.as_str() {
            "unauthorized" => {
                "Unlock the phone and approve this computer's USB debugging prompt, then scan again."
            }
            "offline" => {
                "Reconnect the cable and check the phone's USB debugging setting, then scan again."
            }
            "no permissions" => {
                "The computer cannot access this USB device. Check USB permissions and drivers."
            }
            "recovery" | "sideload" => {
                "This connection is in recovery/sideload mode. Return to Android or the bootloader to use these operations."
            }
            _ => {
                "Select Read device to collect a fresh report. Commands always target this serial explicitly."
            }
        }
    }
}

pub fn parse_adb(text: &str) -> Vec<Device> {
    text.lines()
        .filter_map(|line| {
            let parts: Vec<_> = line.split_whitespace().collect();
            if parts.len() < 2 || parts[0] == "List" || parts[0].starts_with('*') {
                return None;
            }
            let state = if parts[1] == "no" && parts.get(2) == Some(&"permissions") {
                "no permissions"
            } else {
                parts[1]
            };
            if !matches!(
                state,
                "device"
                    | "unauthorized"
                    | "offline"
                    | "recovery"
                    | "sideload"
                    | "no permissions"
                    | "bootloader"
            ) {
                return None;
            }
            Some(Device {
                serial: parts[0].into(),
                transport: Transport::Adb,
                state: state.into(),
                model: parts
                    .iter()
                    .find_map(|p| p.strip_prefix("model:"))
                    .unwrap_or("")
                    .into(),
            })
        })
        .collect()
}

pub fn parse_fastboot(text: &str) -> Vec<Device> {
    text.lines()
        .filter_map(|line| {
            let parts: Vec<_> = line.split_whitespace().collect();
            if parts.len() < 2 || parts[1] != "fastboot" {
                return None;
            }
            Some(Device {
                serial: parts[0].into(),
                transport: Transport::Fastboot,
                state: "fastboot".into(),
                model: String::new(),
            })
        })
        .collect()
}

pub fn parse_properties(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (key, value) = line.strip_prefix('[')?.split_once("]: [")?;
            Some((key.to_owned(), value.strip_suffix(']')?.to_owned()))
        })
        .collect()
}

pub fn boot_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line
            .trim()
            .strip_prefix("(bootloader)")
            .unwrap_or(line.trim())
            .trim();
        let value = line.strip_prefix(key)?.strip_prefix(':')?.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

#[derive(Clone, Debug, Default)]
pub struct Tools {
    pub adb: Option<PathBuf>,
    pub fastboot: Option<PathBuf>,
}

impl Tools {
    pub fn discover(folder: Option<&Path>) -> Self {
        // A deliberately selected folder is authoritative: never mix its tools with PATH.
        let dirs = if let Some(folder) = folder {
            vec![folder.to_owned()]
        } else {
            let mut dirs = Vec::new();
            for key in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
                if let Some(p) = std::env::var_os(key) {
                    dirs.push(PathBuf::from(p).join("platform-tools"));
                }
            }
            if let Some(p) = std::env::var_os("PATH") {
                dirs.extend(std::env::split_paths(&p));
            }
            if let Some(home) = std::env::var_os("HOME") {
                dirs.push(PathBuf::from(&home).join("Android/Sdk/platform-tools"));
                dirs.push(PathBuf::from(home).join("Library/Android/sdk/platform-tools"));
            }
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                dirs.push(PathBuf::from(local).join("Android/Sdk/platform-tools"));
            }
            dirs
        };
        let find = |name: &str| {
            dirs.iter()
                .map(|p| p.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
                .find(|p| p.is_file())
        };
        Self {
            adb: find("adb"),
            fastboot: find("fastboot"),
        }
    }
    fn path(&self, transport: &Transport) -> Result<&Path, String> {
        let p = match transport {
            Transport::Adb => &self.adb,
            Transport::Fastboot => &self.fastboot,
        };
        p.as_deref().ok_or_else(|| {
            format!(
                "{transport:?} tool is missing. Choose an official Platform-Tools folder in Setup."
            )
        })
    }
    pub fn summary(&self) -> String {
        format!(
            "ADB: {}\nFastboot: {}",
            self.adb
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or("Not found".into()),
            self.fastboot
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or("Not found".into())
        )
    }
}

#[derive(Clone, Debug)]
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}
impl Output {
    pub fn text(&self) -> String {
        format!("{}\n{}", self.stdout.trim(), self.stderr.trim())
            .trim()
            .to_owned()
    }
    fn checked(self) -> Result<Self, String> {
        if self.success {
            Ok(self)
        } else {
            let message = self.text();
            Err(if message.is_empty() {
                "Tool exited unsuccessfully without diagnostic output.".into()
            } else {
                message
            })
        }
    }
}

pub trait Runner: Send + Sync {
    fn run(
        &self,
        program: &Path,
        args: &[String],
        timeout: Duration,
        cancel: &Cancel,
    ) -> Result<Output, String>;
}
pub struct ProcessRunner;

fn drain(mut reader: impl Read) -> (Vec<u8>, bool) {
    let mut saved = Vec::new();
    let mut buffer = [0; 8192];
    let mut truncated = false;
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let take = n.min(OUTPUT_LIMIT.saturating_sub(saved.len()));
                saved.extend_from_slice(&buffer[..take]);
                truncated |= take < n;
            }
        }
    }
    (saved, truncated)
}

impl Runner for ProcessRunner {
    fn run(
        &self,
        program: &Path,
        args: &[String],
        timeout: Duration,
        cancel: &Cancel,
    ) -> Result<Output, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled before sending a command.".into());
        }
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Cannot start {}: {e}", program.display()))?;
        let out = child.stdout.take().ok_or("Cannot capture tool output")?;
        let err = child.stderr.take().ok_or("Cannot capture tool errors")?;
        let (out_tx, out_rx) = mpsc::channel();
        let (err_tx, err_rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = out_tx.send(drain(out));
        });
        thread::spawn(move || {
            let _ = err_tx.send(drain(err));
        });
        let start = Instant::now();
        let result = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status.success()),
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(format!("Cannot read tool status: {e}"));
                }
                _ => {}
            }
            let cancelled = cancel.load(Ordering::Relaxed);
            if cancelled || start.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                break Err(if cancelled {
                    "Cancelled. A command already accepted by the phone may still complete; check the phone before retrying.".into()
                } else {
                    "Tool timed out. Check the phone and USB connection before retrying.".into()
                });
            }
            thread::sleep(Duration::from_millis(40));
        };
        // A tool may spawn a daemon inheriting its pipes. Never let an inherited
        // descriptor defeat cancellation or hold the UI worker indefinitely.
        let success = result?;
        let (out, out_cut) = out_rx
            .recv_timeout(Duration::from_secs(1))
            .map_err(|_| "Tool output did not close after exit. Check tools and scan again.")?;
        let (err, err_cut) = err_rx.recv_timeout(Duration::from_secs(1)).map_err(
            |_| "Tool error output did not close after exit. Check tools and scan again.",
        )?;
        Ok(Output {
            success,
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
            truncated: out_cut || err_cut,
        })
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub generated_unix_seconds: u64,
    pub device: Option<Device>,
    pub facts: BTreeMap<String, String>,
    pub notes: Vec<String>,
    pub unlock_eligible: bool,
}
impl Report {
    pub fn text(&self) -> String {
        let mut text = String::new();
        if let Some(d) = &self.device {
            text.push_str(&format!(
                "{}\nSerial: {}\nConnection: {:?} / {}\n\n",
                d.label(),
                d.serial,
                d.transport,
                d.state
            ));
        }
        for (key, value) in &self.facts {
            text.push_str(&format!("{key}: {value}\n"));
        }
        text.push('\n');
        for note in &self.notes {
            text.push_str(&format!("• {note}\n"));
        }
        text
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Read,
    Logs,
    Reboot,
    Bootloader,
    Recovery,
    Unlock,
}
impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Self::Read => "Read device",
            Self::Logs => "Collect system logs",
            Self::Reboot => "Restart phone",
            Self::Bootloader => "Enter bootloader",
            Self::Recovery => "Enter recovery",
            Self::Unlock => "Official bootloader unlock",
        }
    }
    pub fn changes_device(self) -> bool {
        matches!(
            self,
            Self::Reboot | Self::Bootloader | Self::Recovery | Self::Unlock
        )
    }
}

pub struct Engine<R: Runner> {
    pub tools: Tools,
    pub runner: R,
}
impl<R: Runner> Engine<R> {
    fn command(
        &self,
        transport: &Transport,
        args: &[&str],
        timeout: u64,
        cancel: &Cancel,
    ) -> Result<Output, String> {
        self.runner.run(
            self.tools.path(transport)?,
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            Duration::from_secs(timeout),
            cancel,
        )
    }
    fn targeted(
        &self,
        device: &Device,
        args: &[&str],
        timeout: u64,
        cancel: &Cancel,
    ) -> Result<Output, String> {
        // Arguments are passed directly to the executable, never through a shell.
        let mut target = vec!["-s", device.serial.as_str()];
        target.extend_from_slice(args);
        self.command(&device.transport, &target, timeout, cancel)
    }
    pub fn scan(&self, cancel: &Cancel) -> (Vec<Device>, Vec<String>) {
        let mut devices = Vec::new();
        let mut notes = Vec::new();
        for (transport, args) in [
            (Transport::Adb, vec!["devices", "-l"]),
            (Transport::Fastboot, vec!["devices"]),
        ] {
            match self
                .command(&transport, &args, 15, cancel)
                .and_then(Output::checked)
            {
                Ok(out) => {
                    let found = match transport {
                        Transport::Adb => parse_adb(&out.stdout),
                        Transport::Fastboot => parse_fastboot(&out.stdout),
                    };
                    devices.extend(found);
                }
                Err(e) => notes.push(format!("{transport:?}: {e}")),
            }
            if cancel.load(Ordering::Relaxed) {
                break;
            }
        }
        devices.sort_by(|a, b| a.serial.cmp(&b.serial));
        (devices, notes)
    }
    fn assert_connected(&self, device: &Device, cancel: &Cancel) -> Result<(), String> {
        if !device.ready() {
            return Err(device.connection_help().into());
        }
        let args = if device.transport == Transport::Adb {
            vec!["devices", "-l"]
        } else {
            vec!["devices"]
        };
        let out = self
            .command(&device.transport, &args, 15, cancel)?
            .checked()?;
        let devices = if device.transport == Transport::Adb {
            parse_adb(&out.stdout)
        } else {
            parse_fastboot(&out.stdout)
        };
        if devices
            .iter()
            .any(|d| d.serial == device.serial && d.transport == device.transport && d.ready())
        {
            Ok(())
        } else {
            Err("The selected phone disconnected or changed connection state. Scan again; no operation was sent.".into())
        }
    }
    pub fn inspect(&self, device: &Device, cancel: &Cancel) -> Result<Report, String> {
        self.assert_connected(device, cancel)?;
        let mut report = Report {
            schema_version: 1,
            generated_unix_seconds: now(),
            device: Some(device.clone()),
            ..Default::default()
        };
        if device.transport == Transport::Adb {
            let out = self
                .targeted(device, &["shell", "getprop"], 20, cancel)?
                .checked()?;
            let props = parse_properties(&out.stdout);
            // Export only the intended diagnostic properties, not every property on a phone.
            for (key, title) in [
                ("ro.product.manufacturer", "Manufacturer"),
                ("ro.product.model", "Model"),
                ("ro.product.device", "Product"),
                ("ro.build.version.release", "Android"),
                ("ro.build.version.security_patch", "Security patch"),
                ("ro.build.display.id", "Build"),
                ("ro.boot.verifiedbootstate", "Verified boot"),
                ("ro.boot.flash.locked", "Bootloader lock flag"),
                ("ro.oem_unlock_supported", "OEM unlock support flag"),
                ("sys.oem_unlock_allowed", "OEM unlock setting flag"),
            ] {
                if let Some(value) = props.get(key) {
                    report.facts.insert(title.into(), value.clone());
                }
            }
            if report.facts.is_empty() {
                return Err("The phone returned no readable device properties. Check debugging authorization.".into());
            }
            report.notes.push("Android property flags are advisory. Enter the bootloader and read the device again for an unlock-ability check.".into());
            if out.truncated {
                report
                    .notes
                    .push("Property output exceeded the collection limit.".into());
            }
        } else {
            for (key, title) in [
                ("product", "Product"),
                ("unlocked", "Bootloader unlocked"),
                ("secure", "Secure boot"),
                ("version-bootloader", "Bootloader version"),
            ] {
                let out = self.targeted(device, &["getvar", key], 15, cancel)?;
                if out.success {
                    if let Some(value) = boot_value(&out.text(), key) {
                        report.facts.insert(title.into(), value);
                    } else {
                        report
                            .notes
                            .push(format!("{title}: device returned no recognized value."));
                    }
                } else {
                    report
                        .notes
                        .push(format!("{title}: query unavailable. {}", out.text()));
                }
            }
            let ability = self.targeted(device, &["flashing", "get_unlock_ability"], 15, cancel)?;
            let allowed = ability.success
                && boot_value(&ability.text(), "get_unlock_ability").as_deref() == Some("1");
            report.facts.insert(
                "Unlock permission".into(),
                if allowed {
                    "Allowed by bootloader"
                } else {
                    "Not confirmed"
                }
                .into(),
            );
            report.unlock_eligible = allowed
                && report.facts.contains_key("Product")
                && report
                    .facts
                    .get("Bootloader unlocked")
                    .is_some_and(|v| v == "no");
            if !report.unlock_eligible {
                report.notes.push("Unlock stays unavailable unless the phone reports a product, unlocked: no, and get_unlock_ability: 1. Some manufacturers use a separate official process.".into());
            }
            report.notes.push("This is the standard Fastboot workflow. Physical model compatibility has not been certified by Phone Studio. Unlocking erases user data and requires confirmation on the phone.".into());
        }
        Ok(report)
    }
    pub fn execute(
        &self,
        device: &Device,
        action: Action,
        previous: Option<&Report>,
        confirmation: &str,
        erase_confirmation: &str,
        cancel: &Cancel,
    ) -> Result<String, String> {
        if action == Action::Read {
            return Err("Use inspect to read a device.".into());
        }
        self.assert_connected(device, cancel)?;
        if action.changes_device() && confirmation != device.serial {
            return Err("Confirmation does not match the selected phone's serial.".into());
        }
        match action {
            Action::Logs => {
                if device.transport != Transport::Adb {
                    return Err("System logs require an authorized Android connection.".into());
                }
                let out = self
                    .targeted(device, &["logcat", "-d", "-t", "200"], 25, cancel)?
                    .checked()?;
                Ok(format!(
                    "{}{}",
                    out.text(),
                    if out.truncated {
                        "\n[Output truncated at collection limit]"
                    } else {
                        ""
                    }
                ))
            }
            Action::Unlock => {
                if erase_confirmation != "ERASE" {
                    return Err(
                        "Data-erasure confirmation is required for bootloader unlocking.".into(),
                    );
                }
                if device.transport != Transport::Fastboot {
                    return Err("Read the phone in Fastboot mode first.".into());
                }
                let old =
                    previous.ok_or("Read the selected device before requesting an unlock.")?;
                if old.device.as_ref() != Some(device) || !old.unlock_eligible {
                    return Err(
                        "The selected phone has no eligible unlock report. Read it again.".into(),
                    );
                }
                let fresh = self.inspect(device, cancel)?;
                if !fresh.unlock_eligible || fresh.facts.get("Product") != old.facts.get("Product")
                {
                    return Err("Unlock eligibility or product changed. No unlock command was sent; read the phone again.".into());
                }
                self.targeted(device, &["flashing", "unlock"], 180, cancel)?
                    .checked()?;
                Ok("The bootloader command completed. Read the phone's confirmation screen and verify its final state. User data may have been erased. Scan and read again before any further operation.".into())
            }
            Action::Reboot | Action::Bootloader | Action::Recovery => {
                if device.transport == Transport::Fastboot && action == Action::Recovery {
                    return Err(
                        "Recovery reboot is available through ADB only in this version.".into(),
                    );
                }
                let args = match action {
                    Action::Bootloader => vec!["reboot", "bootloader"],
                    Action::Recovery => vec!["reboot", "recovery"],
                    _ => vec!["reboot"],
                };
                self.targeted(device, &args, 25, cancel)?.checked()?;
                Ok(
                    "Reboot command accepted. Wait for the phone to reconnect, then scan again."
                        .into(),
                )
            }
            Action::Read => unreachable!(),
        }
    }
    pub fn versions(&self, cancel: &Cancel) -> String {
        [Transport::Adb, Transport::Fastboot]
            .iter()
            .map(|t| {
                let args = if *t == Transport::Adb {
                    vec!["version"]
                } else {
                    vec!["--version"]
                };
                match self.command(t, &args, 10, cancel).and_then(Output::checked) {
                    Ok(out) => out.text(),
                    Err(e) => format!("{t:?}: {e}"),
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    pub tools_folder: Option<PathBuf>,
}
pub fn data_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("PHONE_STUDIO_DATA") {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    if let Some(p) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(p).join("PhoneStudio");
    }
    if let Some(p) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(p).join("phone-studio");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".local/share/phone-studio")
}
pub fn load_settings(path: &Path) -> Result<Settings, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("Settings could not be read; file preserved: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(e.to_string()),
    }
}
pub fn save_settings(path: &Path, settings: &Settings) -> Result<(), String> {
    let data = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    // The settings file contains paths only. Reports and logs are saved only on request.
    fs::write(path, data).map_err(|e| e.to_string())
}

pub fn demo_devices() -> Vec<Device> {
    vec![
        Device {
            serial: "DEMO-PIXEL-01".into(),
            transport: Transport::Adb,
            state: "device".into(),
            model: "Pixel_8".into(),
        },
        Device {
            serial: "DEMO-USB-02".into(),
            transport: Transport::Adb,
            state: "unauthorized".into(),
            model: String::new(),
        },
    ]
}
pub fn demo_report(device: &Device) -> Report {
    Report {
        schema_version: 1,
        generated_unix_seconds: now(),
        device: Some(device.clone()),
        facts: [
            ("Manufacturer", "Google"),
            ("Model", "Pixel 8 (sample)"),
            ("Android", "15"),
            ("Product", "shiba"),
            ("Verified boot", "green"),
            ("Bootloader lock flag", "1"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect(),
        notes: vec![
            "DEMO DATA — no phone or tool was contacted. Device-changing operations are disabled."
                .into(),
        ],
        unlock_eligible: false,
    }
}
