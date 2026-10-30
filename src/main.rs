use phone_studio::*;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
slint::include_modules!();

struct State {
    demo: bool,
    tools: Tools,
    settings: Settings,
    settings_path: PathBuf,
    devices: Vec<Device>,
    selected: Option<usize>,
    report: Option<Report>,
    diagnostics: Option<String>,
    detail: String,
    versions: String,
    activity: Vec<String>,
    status: String,
    busy: bool,
    cancel: Cancel,
    pending: Option<(Device, Action)>,
    page: i32,
    close_requested: bool,
}
type Shared = Arc<Mutex<State>>;

fn request_close(state: &mut State) -> bool {
    state.close_requested = true;
    if state.busy {
        state.cancel.store(true, Ordering::Relaxed);
        state.note("Closing: stopping the active tool first. A command already accepted by the phone may still complete.");
        false
    } else {
        true
    }
}

impl State {
    fn device(&self) -> Option<&Device> {
        self.selected.and_then(|i| self.devices.get(i))
    }
    fn note(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.status = text
            .lines()
            .next()
            .unwrap_or("Ready")
            .chars()
            .take(180)
            .collect();
        self.activity.push(format!(
            "TASK {:02}  {}",
            self.activity.len() + 1,
            text.chars().take(5000).collect::<String>()
        ));
        if self.activity.len() > 100 {
            self.activity.remove(0);
        }
    }
    fn clear_selection(&mut self) {
        self.selected = None;
        self.report = None;
        self.diagnostics = None;
        self.detail = "Scan devices and select a phone to begin a fresh report.".into();
    }
}

fn job(
    shared: &Shared,
    title: &str,
    run: impl FnOnce(Shared, Tools, Cancel, bool) + Send + 'static,
) {
    let mut state = shared.lock().unwrap();
    if state.busy || state.pending.is_some() {
        return;
    }
    state.busy = true;
    state.cancel = Arc::new(AtomicBool::new(false));
    state.note(format!("{title}…"));
    let (tools, cancel, demo) = (state.tools.clone(), state.cancel.clone(), state.demo);
    let shared = shared.clone();
    drop(state);
    thread::spawn(move || {
        run(shared.clone(), tools, cancel, demo);
        shared.lock().unwrap().busy = false;
    });
}

fn scan(shared: &Shared) {
    job(
        shared,
        "Scanning USB devices",
        |shared, tools, cancel, demo| {
            let (devices, notes) = if demo {
                (
                    demo_devices(),
                    vec!["Demo scan: simulated phones; no commands sent.".into()],
                )
            } else {
                Engine {
                    tools,
                    runner: ProcessRunner,
                }
                .scan(&cancel)
            };
            let mut s = shared.lock().unwrap();
            let previous = s.device().map(|d| (d.serial.clone(), d.transport.clone()));
            s.clear_selection();
            s.selected = previous
                .and_then(|(serial, transport)| {
                    devices
                        .iter()
                        .position(|d| d.serial == serial && d.transport == transport)
                })
                .or_else(|| (!devices.is_empty()).then_some(0));
            s.devices = devices;
            let count = s.devices.len();
            s.note(format!(
                "Scan complete · {} connection(s).{}",
                count,
                if notes.is_empty() {
                    String::new()
                } else {
                    format!("\n{}", notes.join("\n"))
                }
            ));
            if cancel.load(Ordering::Relaxed) {
                s.note("Scan cancelled. Results may be incomplete.");
            }
        },
    );
}

fn perform(shared: &Shared, device: Device, action: Action, serial: String, erase: String) {
    let previous = shared.lock().unwrap().report.clone();
    job(shared, action.name(), move |shared, tools, cancel, demo| {
        let engine = Engine {
            tools,
            runner: ProcessRunner,
        };
        if action == Action::Read {
            let result = if demo {
                if device.ready() {
                    Ok(demo_report(&device))
                } else {
                    Err(device.connection_help().into())
                }
            } else {
                engine.inspect(&device, &cancel)
            };
            let mut s = shared.lock().unwrap();
            // A failed reread must not leave an older eligible report enabled.
            s.report = None;
            match result {
                Ok(report) => {
                    s.detail = report.text();
                    s.report = Some(report);
                    s.page = 1;
                    s.note(format!("Read device completed · {}", device.serial));
                }
                Err(e) => {
                    s.detail = e.clone();
                    s.note(format!("Read device failed\n{e}"));
                }
            }
        } else {
            let result = if demo {
                if action == Action::Logs && device.ready() {
                    Ok("DEMO LOG SAMPLE\nPhone Studio sample diagnostic output. No phone was contacted.\n".into())
                } else {
                    Err("Device-changing operations are disabled in demo mode.".into())
                }
            } else {
                engine.execute(&device, action, previous.as_ref(), &serial, &erase, &cancel)
            };
            let mut s = shared.lock().unwrap();
            match result {
                Ok(output) => {
                    if action == Action::Logs {
                        s.diagnostics = Some(format!(
                            "Phone Studio system logs\nDevice: {}\nCaptured (Unix seconds): {}\n\n{output}",
                            device.serial,
                            now()
                        ));
                        s.page = 4;
                        s.note("System logs collected. View or save them in Logs. Your device report is preserved. Logs may contain private app information; review before sharing.");
                    } else {
                        s.devices.clear();
                        s.clear_selection();
                        s.detail = output.clone();
                        s.page = 1;
                        s.note(format!("{} · {output}", action.name()));
                    }
                }
                Err(e) => {
                    if action == Action::Unlock {
                        s.report = None;
                    }
                    if action == Action::Logs {
                        s.diagnostics = None;
                        s.page = 4;
                    } else {
                        s.detail = e.clone();
                        s.page = 1;
                    }
                    s.note(format!("{} failed\n{e}", action.name()));
                }
            }
        }
    });
}

fn action_from_id(id: i32) -> Option<Action> {
    match id {
        0 => Some(Action::Read),
        1 => Some(Action::Logs),
        2 => Some(Action::Reboot),
        3 => Some(Action::Bootloader),
        4 => Some(Action::Recovery),
        5 => Some(Action::Unlock),
        _ => None,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Phone Studio\n  --demo         Open a simulated workspace; no phone commands\n  --check-tools  Print detected ADB/Fastboot versions without scanning phones\n  --help         Show this help\n\nPHONE_STUDIO_DATA overrides the local settings directory."
        );
        return Ok(());
    }
    for arg in &args {
        if !["--demo", "--check-tools", "--smoke-ui"].contains(&arg.as_str()) {
            return Err(format!("Unknown option: {arg}").into());
        }
    }
    let smoke_ui = args.iter().any(|a| a == "--smoke-ui");
    let demo = smoke_ui || args.iter().any(|a| a == "--demo");
    let dir = data_dir();
    fs::create_dir_all(&dir)?;
    let lock_path = dir.join(if demo { "demo.lock" } else { "session.lock" });
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .map_err(|_| "Phone Studio already has an active session in this data directory.")?;
    let settings_path = dir.join("settings.json");
    let (settings, error) = match load_settings(&settings_path) {
        Ok(s) => (s, None),
        Err(e) => (Settings::default(), Some(e)),
    };
    let tools = if demo {
        Tools::default()
    } else {
        Tools::discover(settings.tools_folder.as_deref())
    };
    if args.iter().any(|a| a == "--check-tools") {
        if demo {
            println!("Demo mode: tools disabled.");
        } else {
            println!(
                "{}\n\n{}",
                tools.summary(),
                Engine {
                    tools,
                    runner: ProcessRunner
                }
                .versions(&Arc::new(AtomicBool::new(false)))
            );
        }
        return Ok(());
    }
    let mut state = State { demo, tools, settings, settings_path, devices: Vec::new(), selected: None, report: None, diagnostics: None,
        detail: "Read a selected phone to collect its device report.\n\nA report contains selected device properties and bootloader query results. No firmware is downloaded or flashed by this release.".into(),
        versions: "Choose Check tools to verify the installed versions.".into(), activity: vec![], status: "Ready · scan to connect".into(), busy: false, cancel: Arc::new(AtomicBool::new(false)), pending: None, page: 0, close_requested: false };
    state.note(if demo {
        "Demo session. All phones and reports are simulated; no tools will run."
    } else {
        "Local session ready. Scan USB devices to begin. No commands have been sent."
    });
    if let Some(error) = error {
        state.note(error);
    }
    if demo {
        state.devices = demo_devices();
        state.selected = Some(0);
        state.report = Some(demo_report(&state.devices[0]));
        state.detail = state.report.as_ref().unwrap().text();
    }
    let shared: Shared = Arc::new(Mutex::new(state));
    let ui = PhoneStudio::new()?;
    {
        let s = shared.clone();
        let weak = ui.as_weak();
        ui.on_close_app(move || {
            if request_close(&mut s.lock().unwrap()) {
                if let Some(ui) = weak.upgrade() {
                    let _ = ui.hide();
                }
            }
        });
    }
    {
        let s = shared.clone();
        ui.on_scan(move || scan(&s));
    }
    {
        let s = shared.clone();
        ui.on_cancel(move || { let mut s = s.lock().unwrap(); if s.busy { s.cancel.store(true, Ordering::Relaxed); s.note("Cancellation requested. Waiting for the current tool to stop; check the phone before retrying."); } });
    }
    {
        let s = shared.clone();
        ui.on_select_device(move |i| {
            let mut s = s.lock().unwrap();
            if s.busy || s.pending.is_some() || i < 0 || i as usize >= s.devices.len() {
                return;
            }
            s.clear_selection();
            s.selected = Some(i as usize);
            s.page = 0;
        });
    }
    {
        let s = shared.clone();
        ui.on_navigate(move |i| {
            if (0..=4).contains(&i) {
                s.lock().unwrap().page = i;
            }
        });
    }
    {
        let s = shared.clone();
        ui.on_action(move |id| {
            let Some(action) = action_from_id(id) else {
                return;
            };
            let mut state = s.lock().unwrap();
            if state.busy || state.pending.is_some() {
                return;
            }
            let Some(device) = state.device().cloned() else {
                return;
            };
            if !device.ready() {
                let help = device.connection_help();
                state.note(help);
                return;
            }
            if action.changes_device() {
                if state.demo {
                    return;
                }
                if action == Action::Unlock
                    && !state
                        .report
                        .as_ref()
                        .is_some_and(|r| r.device.as_ref() == Some(&device) && r.unlock_eligible)
                {
                    state.note("Read an eligible Fastboot device before requesting an unlock.");
                    return;
                }
                state.pending = Some((device, action));
            } else {
                drop(state);
                perform(&s, device, action, String::new(), String::new());
            }
        });
    }
    {
        let s = shared.clone();
        ui.on_dismiss_confirm(move || {
            s.lock().unwrap().pending = None;
        });
    }
    {
        let s = shared.clone();
        ui.on_confirm_action(move |serial, erase| {
            let mut state = s.lock().unwrap();
            if state.busy {
                return;
            }
            let Some((device, action)) = state.pending.clone() else {
                return;
            };
            if serial.as_str() != device.serial
                || (action == Action::Unlock && erase.as_str() != "ERASE")
            {
                state.note("The confirmation fields do not match.");
                return;
            }
            if state.device() != Some(&device) {
                state.pending = None;
                state.note("Selection changed. Review the device again.");
                return;
            }
            state.pending = None;
            drop(state);
            perform(&s, device, action, serial.to_string(), erase.to_string());
        });
    }
    {
        let s = shared.clone();
        ui.on_choose_tools(move || {
            {
                let state = s.lock().unwrap();
                if state.busy || state.demo || state.pending.is_some() {
                    return;
                }
            }
            if let Some(folder) = rfd::FileDialog::new()
                .set_title("Choose official Android Platform-Tools folder")
                .pick_folder()
            {
                configure_tools(&s, Some(folder));
            }
        });
    }
    {
        let s = shared.clone();
        ui.on_auto_tools(move || configure_tools(&s, None));
    }
    {
        let s = shared.clone();
        ui.on_check_tools(move || {
            job(&s, "Checking tools", |shared, tools, cancel, demo| {
                let versions = if demo {
                    "Demo mode — no ADB or Fastboot process was started.".into()
                } else {
                    Engine {
                        tools,
                        runner: ProcessRunner,
                    }
                    .versions(&cancel)
                };
                let mut s = shared.lock().unwrap();
                s.versions = versions;
                s.note("Tool check finished. See Setup for versions or errors.");
            })
        });
    }
    {
        let s = shared.clone();
        ui.on_export_report(move || {
            let data = {
                let state = s.lock().unwrap();
                if state.busy || state.pending.is_some() {
                    return;
                }
                state.report.clone()
            };
            if let Some(report) = data {
                let data = serde_json::to_vec_pretty(&report).expect("serialize report");
                export(&s, "phone-report.json", "JSON report", "json", &data);
            }
        });
    }
    {
        let s = shared.clone();
        ui.on_export_diagnostics(move || {
            let data = {
                let state = s.lock().unwrap();
                if state.busy || state.pending.is_some() {
                    return;
                }
                state.diagnostics.clone()
            };
            if let Some(data) = data {
                export(
                    &s,
                    "phone-system-logs.txt",
                    "Text logs",
                    "txt",
                    data.as_bytes(),
                );
            }
        });
    }
    let timer = slint::Timer::default();
    {
        let s = shared.clone();
        let weak = ui.as_weak();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(120),
            move || {
                if let Some(ui) = weak.upgrade() {
                    let state = s.lock().unwrap();
                    render(&ui, &state);
                    let close = state.close_requested && !state.busy;
                    drop(state);
                    if close {
                        let _ = ui.hide();
                    }
                }
            },
        );
    }
    {
        let s = shared.clone();
        ui.window().on_close_requested(move || {
            if request_close(&mut s.lock().unwrap()) {
                slint::CloseRequestResponse::HideWindow
            } else {
                slint::CloseRequestResponse::KeepWindowShown
            }
        });
    }
    let smoke_timer = slint::Timer::default();
    let smoke_passed = Arc::new(AtomicBool::new(false));
    let smoke_error = Arc::new(Mutex::new(None::<String>));
    if smoke_ui {
        let s = shared.clone();
        let weak = ui.as_weak();
        let passed = smoke_passed.clone();
        let error = smoke_error.clone();
        let mut stage = 0;
        let mut before_up = 0.;
        smoke_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(400),
            move || {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                let check = (|| -> Result<(), String> {
                    let _ = ui.window().take_snapshot().map_err(|e| e.to_string())?;
                    match stage {
                        0 => ui.window().dispatch_event(
                            slint::platform::WindowEvent::PointerScrolled {
                                position: slint::LogicalPosition::new(700., 400.),
                                delta_x: 0.,
                                delta_y: -240.,
                            },
                        ),
                        1 => {
                            before_up = ui.get_workspace_scroll_y();
                            if before_up >= 0. {
                                return Err("Workspace wheel scrolling failed".into());
                            }
                            ui.invoke_scroll_page(1);
                        }
                        2 => {
                            if ui.get_workspace_scroll_y() <= before_up {
                                return Err("Workspace Scroll up button failed".into());
                            }
                            let mut state = s.lock().unwrap();
                            state.page = 4;
                            state.diagnostics = Some(
                                "Native desktop scrolling test — simulated log line.\n".repeat(300),
                            );
                            render(&ui, &state);
                        }
                        3 => ui.window().dispatch_event(
                            slint::platform::WindowEvent::PointerScrolled {
                                position: slint::LogicalPosition::new(700., 400.),
                                delta_x: 0.,
                                delta_y: -240.,
                            },
                        ),
                        4 => {
                            before_up = ui.get_logs_scroll_y();
                            if before_up >= 0. {
                                return Err("Logs wheel scrolling failed".into());
                            }
                            ui.invoke_scroll_page(1);
                        }
                        5 => {
                            if ui.get_logs_scroll_y() <= before_up {
                                return Err("Logs Scroll up button failed".into());
                            }
                        }
                        6 => {
                            passed.store(true, Ordering::Relaxed);
                            let position = slint::LogicalPosition::new(
                                ui.window().size().width as f32 / ui.window().scale_factor() - 65.,
                                36.,
                            );
                            for pressed in [true, false] {
                                let button = slint::platform::PointerEventButton::Left;
                                ui.window().dispatch_event(if pressed {
                                    slint::platform::WindowEvent::PointerPressed {
                                        position,
                                        button,
                                    }
                                } else {
                                    slint::platform::WindowEvent::PointerReleased {
                                        position,
                                        button,
                                    }
                                });
                            }
                        }
                        8.. => return Err("Close button did not close the window".into()),
                        _ => {}
                    }
                    Ok(())
                })();
                stage += 1;
                if let Err(message) = check {
                    *error.lock().unwrap() = Some(message);
                    let _ = ui.hide();
                }
            },
        );
    }
    render(&ui, &shared.lock().unwrap());
    ui.run()?;
    if smoke_ui {
        if let Some(error) = smoke_error.lock().unwrap().take() {
            return Err(error.into());
        }
        if !smoke_passed.load(Ordering::Relaxed) || !shared.lock().unwrap().close_requested {
            return Err(
                "Native UI smoke test ended before its close-button check completed".into(),
            );
        }
        println!(
            "Native UI smoke passed: wheel scrolling, scroll buttons, refresh retention and Close button. No phones were contacted."
        );
    }
    Ok(())
}

fn configure_tools(shared: &Shared, folder: Option<PathBuf>) {
    let mut s = shared.lock().unwrap();
    if s.busy || s.demo || s.pending.is_some() {
        return;
    }
    let settings = Settings {
        tools_folder: folder,
    };
    let tools = Tools::discover(settings.tools_folder.as_deref());
    if let Err(e) = save_settings(&s.settings_path, &settings) {
        s.note(format!("Could not save tools configuration: {e}"));
        return;
    }
    s.settings = settings;
    s.tools = tools;
    s.devices.clear();
    s.clear_selection();
    s.versions = "Tools configuration changed. Check tools to verify these executables.".into();
    s.note("Tools configuration saved. Scan devices to connect.");
}

fn export(shared: &Shared, name: &str, title: &str, extension: &str, bytes: &[u8]) {
    if let Some(path) = rfd::FileDialog::new()
        .set_title(title)
        .set_file_name(name)
        .add_filter(title, &[extension])
        .save_file()
    {
        let result = fs::write(&path, bytes);
        let mut s = shared.lock().unwrap();
        match result {
            Ok(()) => s.note(format!("Saved {}", path.display())),
            Err(e) => s.note(format!("Export failed: {e}")),
        }
    }
}

fn render(ui: &PhoneStudio, s: &State) {
    let rows = s
        .devices
        .iter()
        .map(|d| DeviceRow {
            name: d.label().into(),
            serial: d.serial.clone().into(),
            state: d.state.clone().into(),
            transport: format!("{:?}", d.transport).into(),
            ready: d.ready(),
        })
        .collect::<Vec<_>>();
    // Preserve row instances, hover state and scroll position while polling.
    if ui.get_devices().iter().collect::<Vec<_>>() != rows {
        ui.set_devices(ModelRc::new(VecModel::from(rows)));
    }
    ui.set_selected(s.selected.map(|i| i as i32).unwrap_or(-1));
    ui.set_page(s.page);
    ui.set_busy(s.busy);
    ui.set_demo(s.demo);
    let device = s.device();
    ui.set_ready(device.is_some_and(Device::ready));
    ui.set_adb_ready(device.is_some_and(|d| d.ready() && d.transport == Transport::Adb));
    ui.set_needs_authorization(device.is_some_and(|d| d.state == "unauthorized"));
    ui.set_device_title(
        device
            .map(Device::label)
            .unwrap_or("Your next repair starts here".into())
            .into(),
    );
    ui.set_device_serial(
        device
            .map(|d| d.serial.clone())
            .unwrap_or("Connect an Android phone over USB".into())
            .into(),
    );
    ui.set_connection(
        device
            .map(|d| {
                format!(
                    "{:?} / {}{}",
                    d.transport,
                    d.state,
                    if s.demo { " / simulated" } else { "" }
                )
            })
            .unwrap_or("Waiting for device".into())
            .into(),
    );
    ui.set_help_text(device.map(|d|d.connection_help().to_owned()).unwrap_or("Enable USB debugging on your phone, connect a data cable, and approve this computer when prompted. Then scan USB devices.".into()).into());
    ui.set_status(s.status.clone().into());
    let detail = if let Some(d) = device.filter(|d| !d.ready()) {
        format!(
            "Device information is not available yet.\n\n{}\n\n{}\n\nRead device becomes available when the connection is authorized and online.",
            d.serial,
            d.connection_help()
        )
    } else if device.is_some()
        && s.report.is_none()
        && s.detail.starts_with("Scan devices and select")
    {
        "Phone connected. No device report has been read yet.\n\nOpen Workspace and click Read device to collect the model, Android version and build information. System messages appear separately in Logs.".into()
    } else {
        s.detail.clone()
    };
    ui.set_detail(detail.into());
    let logs = s.diagnostics.as_deref().unwrap_or("No system logs collected yet.\n\nOpen Workspace and choose Collect system logs. Collection errors appear in Activity.");
    let mut preview: String = logs.chars().take(60000).collect();
    if preview.len() < logs.len() {
        preview
            .push_str("\n\n[Preview truncated. Save system logs to get the complete collection.]");
    }
    ui.set_logs_text(preview.into());
    ui.set_activity(
        s.activity
            .iter()
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
            .into(),
    );
    ui.set_tools_text(if s.demo {
        "Demo mode: tools are disabled. Restart without --demo to connect real phones.".into()
    } else {
        s.tools.summary().into()
    });
    ui.set_versions_text(s.versions.clone().into());
    ui.set_has_report(s.report.is_some());
    ui.set_has_diagnostics(s.diagnostics.is_some());
    let eligible = s
        .report
        .as_ref()
        .is_some_and(|r| r.unlock_eligible && r.device.as_ref() == device);
    ui.set_can_unlock(eligible);
    ui.set_unlock_text(if eligible { "The bootloader reports unlocking is allowed and is currently locked. Review the request to confirm this phone and permanent data erasure." } else { "Enter Fastboot mode and read the device. Unlock requires confirmed product identity, a locked bootloader, and explicit unlock permission from the phone." }.into());
    ui.set_confirm_open(s.pending.is_some());
    if let Some((device, action)) = &s.pending {
        ui.set_confirm_title(action.name().into());
        ui.set_confirm_serial(device.serial.clone().into());
        ui.set_confirm_erase(*action == Action::Unlock);
        ui.set_confirm_body(if *action == Action::Unlock { "This requests the official bootloader unlock and permanently erases user data. Back up your phone first. Eligibility will be checked again before sending the request. You must also confirm on the phone. This does not recover a forgotten PIN or remove an account lock." } else { "This will restart the selected phone and interrupt its current activity. Save any open work on the phone first. Scan again after the phone reconnects." }.into());
    }
}

#[cfg(test)]
mod ui_tests {
    use super::*;
    #[test]
    fn close_cancels_an_active_task_and_finishes_when_it_stops() {
        let mut state = demo_state();
        state.busy = true;
        assert!(!request_close(&mut state));
        assert!(state.close_requested);
        assert!(state.cancel.load(Ordering::Relaxed));
        state.busy = false;
        assert!(request_close(&mut state));
    }
    #[test]
    fn visible_close_and_scroll_buttons_receive_real_pointer_clicks() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                ..Default::default()
            },
        )))
        .unwrap();
        let ui = PhoneStudio::new().unwrap();
        ui.window().set_size(slint::LogicalSize::new(1120., 680.));
        ui.show().unwrap();
        let closed = std::rc::Rc::new(std::cell::Cell::new(false));
        let flag = closed.clone();
        ui.on_close_app(move || flag.set(true));
        let click = |x, y| {
            for pressed in [true, false] {
                let position = slint::LogicalPosition::new(x, y);
                let button = slint::platform::PointerEventButton::Left;
                ui.window().dispatch_event(if pressed {
                    slint::platform::WindowEvent::PointerPressed { position, button }
                } else {
                    slint::platform::WindowEvent::PointerReleased { position, button }
                });
            }
            let _ = ui.window().take_snapshot().unwrap();
        };
        let _ = ui.window().take_snapshot().unwrap();
        click(1055., 36.);
        assert!(closed.get(), "The visible header Close button must work");
        click(908., 618.);
        assert!(
            ui.get_workspace_scroll_y() < 0.,
            "The visible Scroll down button must move the viewport"
        );
        click(776., 618.);
        assert_eq!(
            ui.get_workspace_scroll_y(),
            0.,
            "The visible Scroll up button must return to the top"
        );
    }
    fn demo_state() -> State {
        let devices = demo_devices();
        let report = demo_report(&devices[0]);
        State {
            demo: true,
            tools: Tools::default(),
            settings: Settings::default(),
            settings_path: PathBuf::new(),
            devices,
            selected: Some(0),
            report: Some(report.clone()),
            diagnostics: None,
            detail: report.text(),
            versions: String::new(),
            activity: vec![],
            status: String::new(),
            busy: false,
            cancel: Arc::new(AtomicBool::new(false)),
            pending: None,
            page: 0,
            close_requested: false,
        }
    }
    #[test]
    fn collecting_logs_preserves_the_device_report_and_opens_the_logs_tab() {
        let state = demo_state();
        let expected_report = state.detail.clone();
        let device = state.devices[0].clone();
        let shared = Arc::new(Mutex::new(state));
        perform(&shared, device, Action::Logs, String::new(), String::new());
        let start = std::time::Instant::now();
        while shared.lock().unwrap().busy {
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "log collection worker did not finish"
            );
            thread::sleep(Duration::from_millis(5));
        }
        let state = shared.lock().unwrap();
        assert!(
            state
                .diagnostics
                .as_ref()
                .unwrap()
                .contains("DEMO LOG SAMPLE")
        );
        assert_eq!(
            state.detail, expected_report,
            "System logs must not replace the model and Android-version report"
        );
        assert_eq!(state.page, 4);
    }
    #[test]
    fn workspace_fits_small_desktop_and_scrolls_with_the_mouse_wheel() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                ..Default::default()
            },
        )))
        .unwrap();
        let ui = PhoneStudio::new().unwrap();
        ui.window().set_size(slint::LogicalSize::new(1100., 640.));
        ui.show().unwrap();
        let _ = ui.window().take_snapshot().unwrap();
        let before = ui.get_workspace_scroll_y();
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(700., 400.),
                delta_x: 0.,
                delta_y: -240.,
            });
        let _ = ui.window().take_snapshot().unwrap();
        let after = ui.get_workspace_scroll_y();
        assert!(
            ui.get_minimum_window_height() <= 640.,
            "The minimum window height prevents the app fitting a 640px-high desktop"
        );
        assert!(
            after < before,
            "Mouse-wheel scrolling must reveal the controls below the viewport"
        );
        for page in [1, 4] {
            ui.set_page(page);
            let text = "Diagnostic entry with enough lines to exceed the visible report area.\n"
                .repeat(300);
            if page == 1 {
                ui.set_detail(text.into());
            } else {
                ui.set_logs_text(text.into());
            }
            let _ = ui.window().take_snapshot().unwrap();
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                    position: slint::LogicalPosition::new(700., 400.),
                    delta_x: 0.,
                    delta_y: -240.,
                });
            let _ = ui.window().take_snapshot().unwrap();
            let y = if page == 1 {
                ui.get_report_scroll_y()
            } else {
                ui.get_logs_scroll_y()
            };
            assert!(
                y < 0.,
                "Long text in page {page} must scroll with the mouse wheel"
            );
            // The regular render loop repeats unchanged values; it must not reset scrolling.
            if page == 1 {
                ui.set_detail(ui.get_detail());
            } else {
                ui.set_logs_text(ui.get_logs_text());
            }
            let _ = ui.window().take_snapshot().unwrap();
            let retained = if page == 1 {
                ui.get_report_scroll_y()
            } else {
                ui.get_logs_scroll_y()
            };
            assert_eq!(retained, y, "UI refresh must preserve scroll position");
        }
        ui.set_page(0);
        ui.set_workspace_scroll_y(0.);
        ui.window().set_size(slint::LogicalSize::new(960., 520.));
        let snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            std::env::temp_dir().join("phone-studio-compact.png"),
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(700., 400.),
                delta_x: 0.,
                delta_y: -2000.,
            });
        let snapshot = ui.window().take_snapshot().unwrap();
        assert!(
            ui.get_workspace_scroll_y() < 0.,
            "The workspace must scroll at the smallest supported size"
        );
        image::save_buffer(
            std::env::temp_dir().join("phone-studio-compact-scrolled.png"),
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
    #[test]
    fn unauthorized_device_shows_the_reason_instead_of_an_empty_report() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                ..Default::default()
            },
        )))
        .unwrap();
        let ui = PhoneStudio::new().unwrap();
        let mut state = demo_state();
        state.devices[0].state = "unauthorized".into();
        state.report = None;
        render(&ui, &state);
        assert!(ui.get_needs_authorization());
        assert!(!ui.get_ready());
        assert!(!ui.get_has_report());
        assert!(ui.get_detail().contains("USB debugging"));
        assert!(ui.get_detail().contains("not available"));
    }
    #[test]
    fn render_native_workspace_and_confirmation() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                ..Default::default()
            },
        )))
        .unwrap();
        let ui = PhoneStudio::new().unwrap();
        let devices = demo_devices();
        let report = demo_report(&devices[0]);
        let mut s = State {
            demo: true,
            tools: Tools::default(),
            settings: Settings::default(),
            settings_path: PathBuf::new(),
            devices,
            selected: Some(0),
            report: Some(report.clone()),
            diagnostics: None,
            detail: report.text(),
            versions: "Demo tools disabled".into(),
            activity: vec!["Demo session started. No phone contacted.".into()],
            status: "Demo workspace · simulated device selected".into(),
            busy: false,
            cancel: Arc::new(AtomicBool::new(false)),
            pending: None,
            page: 0,
            close_requested: false,
        };
        render(&ui, &s);
        assert!(ui.get_ready());
        assert!(!ui.get_can_unlock());
        assert!(!ui.get_confirm_open());
        ui.window().set_size(slint::LogicalSize::new(1280., 900.));
        ui.show().unwrap();
        let snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            std::env::temp_dir().join("phone-studio-preview.png"),
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        ui.set_confirm_open(true);
        ui.set_confirm_erase(true);
        ui.set_confirm_title("Official bootloader unlock".into());
        ui.set_confirm_serial("TEST-SERIAL".into());
        ui.set_confirm_body(
            "This operation erases user data and requires on-device confirmation.".into(),
        );
        ui.window().set_size(slint::LogicalSize::new(1100., 780.));
        let snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            std::env::temp_dir().join("phone-studio-confirmation.png"),
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
        ui.set_confirm_open(false);
        for (page, name) in [(1, "report"), (2, "activity"), (3, "setup")] {
            ui.set_page(page);
            let snapshot = ui.window().take_snapshot().unwrap();
            image::save_buffer(
                std::env::temp_dir().join(format!("phone-studio-{name}.png")),
                snapshot.as_bytes(),
                snapshot.width(),
                snapshot.height(),
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
        s.devices.clear();
        s.clear_selection();
        s.page = 0;
        render(&ui, &s);
        let snapshot = ui.window().take_snapshot().unwrap();
        image::save_buffer(
            std::env::temp_dir().join("phone-studio-empty.png"),
            snapshot.as_bytes(),
            snapshot.width(),
            snapshot.height(),
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
}
