use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn sigint_terminates_active_solver_process_tree() {
    let dir = tempfile::tempdir().unwrap();
    let params = dir.path().join("solver.params");
    let instances = dir.path().join("instances.txt");
    let wrapper = dir.path().join("wrapper.sh");
    let solver_pid = dir.path().join("solver.pid");
    let scenario = dir.path().join("scenario.yaml");

    fs::write(&params, "alpha {1} [1]\n").unwrap();
    fs::write(&instances, "instance.cnf\n").unwrap();
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nsleep 300 &\nchild=$!\necho \"$child\" > '{}'\ntrap 'kill \"$child\" 2>/dev/null; wait \"$child\" 2>/dev/null; exit 143' INT TERM\nwait \"$child\"\n",
            solver_pid.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&wrapper).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&wrapper, permissions).unwrap();
    fs::write(
        &scenario,
        format!(
            "algo: '{}'\nparamfile: '{}'\ninstance_file: '{}'\ncutoff_time: 300\ntuner_timeout: 300\napproach: focused\ninitial_fidelity: 1\nfidelity_step: 1\ncores: 1\n",
            wrapper.display(),
            params.display(),
            instances.display(),
        ),
    )
    .unwrap();

    let mut ramparils = Command::new(env!("CARGO_BIN_EXE_ramparils"))
        .arg("--scenariofile")
        .arg(&scenario)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let pid_deadline = Instant::now() + Duration::from_secs(5);
    while !solver_pid.exists() {
        assert!(Instant::now() < pid_deadline, "solver process did not start");
        thread::sleep(Duration::from_millis(20));
    }
    let solver_pid: libc::pid_t =
        fs::read_to_string(&solver_pid).unwrap().trim().parse().unwrap();

    unsafe {
        assert_eq!(libc::kill(ramparils.id() as libc::pid_t, libc::SIGINT), 0);
    }

    let exit_deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = ramparils.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < exit_deadline,
            "ramparils did not exit after SIGINT"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(130));

    let process_exists = unsafe {
        libc::kill(solver_pid, 0) == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    };
    assert!(!process_exists, "solver process {solver_pid} was orphaned");
}

#[test]
fn llm2smt_wrapper_forwards_sigterm_to_solver_session() {
    let dir = tempfile::tempdir().unwrap();
    let solver = dir.path().join("solver.sh");
    let solver_pid = dir.path().join("solver.pid");
    let instance = dir.path().join("instance.smt2");
    fs::write(&instance, "(set-logic QF_EUF)\n").unwrap();
    fs::write(
        &solver,
        "#!/bin/sh\necho \"$$\" > \"$SOLVER_PID_FILE\"\nsleep 300\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&solver).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&solver, permissions).unwrap();

    let wrapper = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/llm2smt/llm2smt_wrapper.py");
    let mut process = Command::new("python3")
        .arg(wrapper)
        .arg(&instance)
        .arg("300")
        .args([
            "-preprocess_passes", "1",
            "-nary", "true",
            "-flatten", "true",
            "-finite_domain_amo", "true",
            "-finite_domain_eq_defs", "true",
            "-theory_prop", "true",
            "-prop_interval", "1",
            "-prop_assign_threshold", "1",
            "-prop_delivery_budget", "1",
        ])
        .env("LLM2SMT", &solver)
        .env("SOLVER_PID_FILE", &solver_pid)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let pid_deadline = Instant::now() + Duration::from_secs(5);
    while !solver_pid.exists() {
        assert!(Instant::now() < pid_deadline, "fake llm2smt did not start");
        thread::sleep(Duration::from_millis(20));
    }
    let solver_pid: libc::pid_t =
        fs::read_to_string(&solver_pid).unwrap().trim().parse().unwrap();

    unsafe {
        assert_eq!(libc::kill(process.id() as libc::pid_t, libc::SIGTERM), 0);
    }
    let exit_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if process.try_wait().unwrap().is_some() {
            break;
        }
        assert!(
            Instant::now() < exit_deadline,
            "llm2smt wrapper did not exit after SIGTERM"
        );
        thread::sleep(Duration::from_millis(20));
    }

    let process_exists = unsafe {
        libc::kill(solver_pid, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    };
    assert!(!process_exists, "llm2smt solver process {solver_pid} was orphaned");
}
