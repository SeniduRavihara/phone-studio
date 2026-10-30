use phone_studio::*;
use std::{
    collections::VecDeque,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

struct Expected {
    args: Vec<String>,
    output: Result<Output, String>,
}
struct FakeRunner {
    remaining: Mutex<VecDeque<Expected>>,
}
impl Runner for FakeRunner {
    fn run(&self, _: &Path, args: &[String], _: Duration, _: &Cancel) -> Result<Output, String> {
        let call = self
            .remaining
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected device command");
        assert_eq!(args, call.args, "wrong target, command, or command order");
        call.output
    }
}
fn output(stdout: &str, stderr: &str) -> Output {
    Output {
        success: true,
        stdout: stdout.into(),
        stderr: stderr.into(),
        truncated: false,
    }
}
fn call(args: &[&str], stdout: &str, stderr: &str) -> Expected {
    Expected {
        args: args.iter().map(|s| s.to_string()).collect(),
        output: Ok(output(stdout, stderr)),
    }
}
fn engine(calls: Vec<Expected>) -> Engine<FakeRunner> {
    Engine {
        tools: Tools {
            adb: Some("adb".into()),
            fastboot: Some("fastboot".into()),
        },
        runner: FakeRunner {
            remaining: Mutex::new(calls.into()),
        },
    }
}
fn done(engine: &Engine<FakeRunner>) {
    assert!(
        engine.runner.remaining.lock().unwrap().is_empty(),
        "expected command was never run"
    );
}
fn cancel() -> Cancel {
    Arc::new(AtomicBool::new(false))
}
fn fastboot() -> Device {
    Device {
        serial: "PHONE-1".into(),
        transport: Transport::Fastboot,
        state: "fastboot".into(),
        model: String::new(),
    }
}
fn adb() -> Device {
    Device {
        transport: Transport::Adb,
        state: "device".into(),
        ..fastboot()
    }
}
fn prior(device: &Device) -> Report {
    Report {
        device: Some(device.clone()),
        unlock_eligible: true,
        facts: [("Product".into(), "shiba".into())].into(),
        ..Default::default()
    }
}
fn live_report(product: &str, locked: &str, ability: &str) -> Vec<Expected> {
    vec![
        call(&["devices"], "PHONE-1\tfastboot\n", ""),
        call(
            &["-s", "PHONE-1", "getvar", "product"],
            "",
            &format!("(bootloader) product: {product}\nFinished. Total time: 0.001s"),
        ),
        call(
            &["-s", "PHONE-1", "getvar", "unlocked"],
            "",
            &format!("unlocked: {locked}"),
        ),
        call(&["-s", "PHONE-1", "getvar", "secure"], "", "secure: yes"),
        call(
            &["-s", "PHONE-1", "getvar", "version-bootloader"],
            "",
            "version-bootloader: test",
        ),
        call(
            &["-s", "PHONE-1", "flashing", "get_unlock_ability"],
            "",
            &format!("(bootloader) get_unlock_ability: {ability}"),
        ),
    ]
}

#[test]
fn discovery_keeps_unauthorized_offline_and_permission_errors_visible() {
    let devices = parse_adb(
        "* daemon started successfully *\nList of devices attached\nA device product:p model:Pixel_8 device:shiba\nB unauthorized usb:1\nC offline\nD no permissions (user in plugdev group)\nE sideload\n",
    );
    assert_eq!(devices.len(), 5);
    assert_eq!(devices[0].label(), "Pixel 8");
    assert!(!devices[1].ready());
    assert!(!devices[2].ready());
    assert!(!devices[3].ready());
    assert!(!devices[4].ready());
    assert_eq!(devices[3].state, "no permissions");
    assert_eq!(
        parse_fastboot("PHONE-1\tfastboot\n< waiting for any device >\n"),
        vec![fastboot()]
    );
}

#[test]
fn unavailable_transport_does_not_hide_the_other_transport() {
    let mut denied = call(&["devices", "-l"], "", "ADB service failed");
    denied.output.as_mut().unwrap().success = false;
    let e = engine(vec![denied, call(&["devices"], "PHONE-1 fastboot", "")]);
    let (devices, notes) = e.scan(&cancel());
    assert_eq!(devices, vec![fastboot()]);
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("ADB service failed"));
    done(&e);
}

#[test]
fn unauthorized_phone_never_receives_a_shell_command() {
    let e = engine(vec![]);
    let device = Device {
        state: "unauthorized".into(),
        ..adb()
    };
    assert!(
        e.inspect(&device, &cancel())
            .unwrap_err()
            .contains("approve")
    );
    done(&e);
}

#[test]
fn report_limits_properties_and_never_infers_unlock_eligibility_from_android_flags() {
    let e = engine(vec![
        call(&["devices", "-l"], "PHONE-1 device\nOTHER device", ""),
        call(
            &["-s", "PHONE-1", "shell", "getprop"],
            "[ro.product.model]: [Pixel 8]\n[ro.build.version.release]: [15]\n[sys.oem_unlock_allowed]: [1]\n[private.token]: [do-not-export]\n",
            "",
        ),
    ]);
    let r = e.inspect(&adb(), &cancel()).unwrap();
    assert_eq!(r.facts["Model"], "Pixel 8");
    assert!(!r.unlock_eligible);
    assert!(!serde_json::to_string(&r).unwrap().contains("do-not-export"));
    done(&e);
}

#[test]
fn switching_to_a_different_phone_does_not_redirect_a_reboot() {
    let e = engine(vec![call(&["devices", "-l"], "OTHER-PHONE device", "")]);
    let result = e.execute(&adb(), Action::Reboot, None, "PHONE-1", "", &cancel());
    assert!(result.unwrap_err().contains("disconnected"));
    done(&e);
}

#[test]
fn cancellation_during_scan_stops_before_the_second_tool() {
    struct Cancels(Cancel);
    impl Runner for Cancels {
        fn run(
            &self,
            _: &Path,
            args: &[String],
            _: Duration,
            _: &Cancel,
        ) -> Result<Output, String> {
            assert_eq!(args, ["devices", "-l"]);
            self.0.store(true, Ordering::Relaxed);
            Err("Cancelled".into())
        }
    }
    let c = cancel();
    let e = Engine {
        tools: Tools {
            adb: Some("adb".into()),
            fastboot: Some("fastboot".into()),
        },
        runner: Cancels(c.clone()),
    };
    let (devices, notes) = e.scan(&c);
    assert!(devices.is_empty());
    assert_eq!(notes.len(), 1);
}

#[test]
fn unlock_requires_both_the_selected_serial_and_erasure_confirmation() {
    for (serial, erase) in [
        ("WRONG-PHONE", "ERASE"),
        ("PHONE-1", "erase"),
        ("PHONE-1", ""),
    ] {
        let e = engine(vec![call(&["devices"], "PHONE-1 fastboot", "")]);
        assert!(
            e.execute(
                &fastboot(),
                Action::Unlock,
                Some(&prior(&fastboot())),
                serial,
                erase,
                &cancel()
            )
            .is_err()
        );
        done(&e);
    }
}

#[test]
fn unlock_rechecks_permission_and_product_and_requires_a_locked_state() {
    for (product, locked, ability) in [
        ("shiba", "no", "0"),
        ("different-phone", "no", "1"),
        ("shiba", "yes", "1"),
        ("shiba", "", "1"),
    ] {
        let mut calls = vec![call(&["devices"], "PHONE-1 fastboot", "")];
        calls.extend(live_report(product, locked, ability));
        let e = engine(calls);
        let error = e
            .execute(
                &fastboot(),
                Action::Unlock,
                Some(&prior(&fastboot())),
                "PHONE-1",
                "ERASE",
                &cancel(),
            )
            .unwrap_err();
        assert!(error.contains("changed"));
        done(&e);
    }
}

#[test]
fn eligible_unlock_targets_only_the_explicit_phone_after_fresh_checks() {
    let mut calls = vec![call(&["devices"], "PHONE-1 fastboot\nPHONE-2 fastboot", "")];
    calls.extend(live_report("shiba", "no", "1"));
    calls.push(call(
        &["-s", "PHONE-1", "flashing", "unlock"],
        "",
        "OKAY\nFinished.",
    ));
    let e = engine(calls);
    assert!(
        e.execute(
            &fastboot(),
            Action::Unlock,
            Some(&prior(&fastboot())),
            "PHONE-1",
            "ERASE",
            &cancel()
        )
        .unwrap()
        .contains("verify")
    );
    done(&e);
}

#[test]
fn failed_query_cannot_enable_unlock_even_with_a_plausible_value() {
    let mut calls = live_report("shiba", "no", "1");
    calls.last_mut().unwrap().output.as_mut().unwrap().success = false;
    let e = engine(calls);
    assert!(!e.inspect(&fastboot(), &cancel()).unwrap().unlock_eligible);
    done(&e);
}

#[test]
fn wrong_report_is_not_accepted_for_a_selected_device() {
    let mut report = prior(&fastboot());
    report.device.as_mut().unwrap().serial = "OTHER".into();
    let e = engine(vec![call(&["devices"], "PHONE-1 fastboot", "")]);
    assert!(
        e.execute(
            &fastboot(),
            Action::Unlock,
            Some(&report),
            "PHONE-1",
            "ERASE",
            &cancel()
        )
        .unwrap_err()
        .contains("eligible")
    );
    done(&e);
}

#[test]
fn reboot_and_log_commands_have_fixed_arguments_and_explicit_target() {
    let e = engine(vec![
        call(&["devices", "-l"], "PHONE-1 device", ""),
        call(&["-s", "PHONE-1", "reboot", "bootloader"], "", ""),
    ]);
    assert!(
        e.execute(&adb(), Action::Bootloader, None, "PHONE-1", "", &cancel())
            .is_ok()
    );
    done(&e);
    let e = engine(vec![
        call(&["devices", "-l"], "PHONE-1 device", ""),
        call(
            &["-s", "PHONE-1", "logcat", "-d", "-t", "200"],
            "sample logs",
            "",
        ),
    ]);
    assert!(
        e.execute(&adb(), Action::Logs, None, "", "", &cancel())
            .unwrap()
            .contains("sample logs")
    );
    done(&e);
}

#[test]
fn explicit_tool_folder_never_falls_back_to_unrelated_path_tools() {
    let dir = tempfile::tempdir().unwrap();
    let tools = Tools::discover(Some(dir.path()));
    assert!(tools.adb.is_none());
    assert!(tools.fastboot.is_none());
    std::fs::write(
        dir.path()
            .join(format!("adb{}", std::env::consts::EXE_SUFFIX)),
        b"test",
    )
    .unwrap();
    let tools = Tools::discover(Some(dir.path()));
    assert!(tools.adb.is_some());
    assert!(tools.fastboot.is_none());
}

#[test]
fn corrupt_settings_are_preserved_and_valid_settings_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("settings.json");
    std::fs::write(&file, "{broken").unwrap();
    assert!(load_settings(&file).is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{broken");
    save_settings(
        &file,
        &Settings {
            tools_folder: Some(dir.path().into()),
        },
    )
    .unwrap();
    assert_eq!(
        load_settings(&file).unwrap().tools_folder.as_deref(),
        Some(dir.path())
    );
}

#[cfg(unix)]
#[test]
fn real_runner_times_out_and_honors_pre_cancel_without_launching() {
    let start = Instant::now();
    let result = ProcessRunner.run(
        Path::new("/bin/sleep"),
        &["3".into()],
        Duration::from_millis(80),
        &cancel(),
    );
    assert!(result.unwrap_err().contains("timed out"));
    assert!(start.elapsed() < Duration::from_secs(2));
    let c = cancel();
    c.store(true, Ordering::Relaxed);
    let result = ProcessRunner.run(
        Path::new("/this-tool-does-not-exist"),
        &[],
        Duration::from_secs(1),
        &c,
    );
    assert!(result.unwrap_err().contains("Cancelled"));
}

#[cfg(unix)]
#[test]
fn real_runner_drains_stdout_and_stderr_and_preserves_nonzero_exit() {
    let result = ProcessRunner
        .run(
            Path::new("/bin/sh"),
            &[
                "-c".into(),
                "printf 'device-output'; printf 'tool-error' >&2; exit 7".into(),
            ],
            Duration::from_secs(2),
            &cancel(),
        )
        .unwrap();
    assert!(!result.success);
    assert_eq!(result.stdout, "device-output");
    assert_eq!(result.stderr, "tool-error");
}

#[cfg(unix)]
#[test]
fn inherited_output_pipes_do_not_defeat_a_timeout() {
    let start = Instant::now();
    let result = ProcessRunner.run(
        Path::new("/bin/sh"),
        &["-c".into(), "sleep 3 & wait".into()],
        Duration::from_millis(80),
        &cancel(),
    );
    assert!(result.unwrap_err().contains("timed out"));
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "waited for a descendant's inherited output pipe"
    );
}

#[cfg(unix)]
#[test]
fn running_process_can_be_cancelled_and_large_output_is_bounded() {
    let c = cancel();
    let trigger = c.clone();
    let setter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(60));
        trigger.store(true, Ordering::Relaxed);
    });
    let start = Instant::now();
    let result = ProcessRunner.run(
        Path::new("/bin/sleep"),
        &["3".into()],
        Duration::from_secs(5),
        &c,
    );
    setter.join().unwrap();
    assert!(result.unwrap_err().contains("Cancelled"));
    assert!(start.elapsed() < Duration::from_secs(2));
    let result = ProcessRunner
        .run(
            Path::new("/bin/sh"),
            &[
                "-c".into(),
                "dd if=/dev/zero bs=65536 count=40 2>/dev/null".into(),
            ],
            Duration::from_secs(5),
            &cancel(),
        )
        .unwrap();
    assert!(result.success);
    assert!(result.truncated);
    assert_eq!(result.stdout.len(), OUTPUT_LIMIT);
}
