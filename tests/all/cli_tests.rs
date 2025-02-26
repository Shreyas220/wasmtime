#![cfg(not(miri))]

use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use tempfile::{NamedTempFile, TempDir};

// Run the wasmtime CLI with the provided args and return the `Output`.
// If the `stdin` is `Some`, opens the file and redirects to the child's stdin.
pub fn run_wasmtime_for_output(args: &[&str], stdin: Option<&Path>) -> Result<Output> {
    let mut cmd = get_wasmtime_command()?;
    let stdin = stdin
        .map(File::open)
        .transpose()
        .context("Cannot open a file to use as stdin")?;

    if let Some(mut f) = stdin {
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;

        let mut child = cmd
            .stdout(Stdio::piped())
            .stdin(Stdio::piped())
            .args(args)
            .spawn()?;

        let mut stdin = child.stdin.take().unwrap();
        std::thread::spawn(move || {
            stdin
                .write_all(&buf)
                .expect("failed to write module to child stdin")
        });
        child.wait_with_output().map_err(Into::into)
    } else {
        cmd.args(args).output().map_err(Into::into)
    }
}

/// Get the Wasmtime CLI as a [Command].
pub fn get_wasmtime_command() -> Result<Command> {
    // Figure out the Wasmtime binary from the current executable.
    let runner = std::env::vars()
        .filter(|(k, _v)| k.starts_with("CARGO_TARGET") && k.ends_with("RUNNER"))
        .next();
    let mut me = std::env::current_exe()?;
    me.pop(); // chop off the file name
    me.pop(); // chop off `deps`
    me.push("wasmtime");

    // If we're running tests with a "runner" then we might be doing something
    // like cross-emulation, so spin up the emulator rather than the tests
    // itself, which may not be natively executable.
    let cmd = if let Some((_, runner)) = runner {
        let mut parts = runner.split_whitespace();
        let mut cmd = Command::new(parts.next().unwrap());
        for arg in parts {
            cmd.arg(arg);
        }
        cmd.arg(&me);
        cmd
    } else {
        Command::new(&me)
    };

    Ok(cmd)
}

// Run the wasmtime CLI with the provided args and, if it succeeds, return
// the standard output in a `String`.
fn run_wasmtime(args: &[&str]) -> Result<String> {
    let output = run_wasmtime_for_output(args, None)?;
    if !output.status.success() {
        bail!(
            "Failed to execute wasmtime with: {:?}\n{}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout).unwrap())
}

fn build_wasm(wat_path: impl AsRef<Path>) -> Result<NamedTempFile> {
    let mut wasm_file = NamedTempFile::new()?;
    let wasm = wat::parse_file(wat_path)?;
    wasm_file.write(&wasm)?;
    Ok(wasm_file)
}

// Very basic use case: compile binary wasm file and run specific function with arguments.
#[test]
fn run_wasmtime_simple() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/simple.wat")?;
    run_wasmtime(&[
        "run",
        wasm.path().to_str().unwrap(),
        "--invoke",
        "simple",
        "--disable-cache",
        "4",
    ])?;
    Ok(())
}

// Wasmtime shakk when not enough arguments were provided.
#[test]
fn run_wasmtime_simple_fail_no_args() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/simple.wat")?;
    assert!(
        run_wasmtime(&[
            "run",
            wasm.path().to_str().unwrap(),
            "--disable-cache",
            "--invoke",
            "simple",
        ])
        .is_err(),
        "shall fail"
    );
    Ok(())
}

// Running simple wat
#[test]
fn run_wasmtime_simple_wat() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/simple.wat")?;
    run_wasmtime(&[
        "run",
        wasm.path().to_str().unwrap(),
        "--invoke",
        "simple",
        "--disable-cache",
        "4",
    ])?;
    assert_eq!(
        run_wasmtime(&[
            "run",
            wasm.path().to_str().unwrap(),
            "--invoke",
            "get_f32",
            "--disable-cache",
        ])?,
        "100\n"
    );
    assert_eq!(
        run_wasmtime(&[
            "run",
            wasm.path().to_str().unwrap(),
            "--invoke",
            "get_f64",
            "--disable-cache",
        ])?,
        "100\n"
    );
    Ok(())
}

// Running a wat that traps.
#[test]
fn run_wasmtime_unreachable_wat() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/unreachable.wat")?;
    let output =
        run_wasmtime_for_output(&[wasm.path().to_str().unwrap(), "--disable-cache"], None)?;

    assert_ne!(output.stderr, b"");
    assert_eq!(output.stdout, b"");
    assert!(!output.status.success());

    let code = output
        .status
        .code()
        .expect("wasmtime process should exit normally");

    // Test for the specific error code Wasmtime uses to indicate a trap return.
    #[cfg(unix)]
    assert_eq!(code, 128 + libc::SIGABRT);
    #[cfg(windows)]
    assert_eq!(code, 3);
    Ok(())
}

// Run a simple WASI hello world, snapshot0 edition.
#[test]
fn hello_wasi_snapshot0() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/hello_wasi_snapshot0.wat")?;
    let stdout = run_wasmtime(&[wasm.path().to_str().unwrap(), "--disable-cache"])?;
    assert_eq!(stdout, "Hello, world!\n");
    Ok(())
}

// Run a simple WASI hello world, snapshot1 edition.
#[test]
fn hello_wasi_snapshot1() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/hello_wasi_snapshot1.wat")?;
    let stdout = run_wasmtime(&[wasm.path().to_str().unwrap(), "--disable-cache"])?;
    assert_eq!(stdout, "Hello, world!\n");
    Ok(())
}

#[test]
fn timeout_in_start() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/iloop-start.wat")?;
    let output = run_wasmtime_for_output(
        &[
            "run",
            wasm.path().to_str().unwrap(),
            "--wasm-timeout",
            "1ms",
            "--disable-cache",
        ],
        None,
    )?;
    assert!(!output.status.success());
    assert_eq!(output.stdout, b"");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("wasm trap: interrupt"),
        "bad stderr: {}",
        stderr
    );
    Ok(())
}

#[test]
fn timeout_in_invoke() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/iloop-invoke.wat")?;
    let output = run_wasmtime_for_output(
        &[
            "run",
            wasm.path().to_str().unwrap(),
            "--wasm-timeout",
            "1ms",
            "--disable-cache",
        ],
        None,
    )?;
    assert!(!output.status.success());
    assert_eq!(output.stdout, b"");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("wasm trap: interrupt"),
        "bad stderr: {}",
        stderr
    );
    Ok(())
}

// Exit with a valid non-zero exit code, snapshot0 edition.
#[test]
fn exit2_wasi_snapshot0() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/exit2_wasi_snapshot0.wat")?;
    let output =
        run_wasmtime_for_output(&[wasm.path().to_str().unwrap(), "--disable-cache"], None)?;
    assert_eq!(output.status.code().unwrap(), 2);
    Ok(())
}

// Exit with a valid non-zero exit code, snapshot1 edition.
#[test]
fn exit2_wasi_snapshot1() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/exit2_wasi_snapshot1.wat")?;
    let output =
        run_wasmtime_for_output(&[wasm.path().to_str().unwrap(), "--disable-cache"], None)?;
    assert_eq!(output.status.code().unwrap(), 2);
    Ok(())
}

// Exit with a valid non-zero exit code, snapshot0 edition.
#[test]
fn exit125_wasi_snapshot0() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/exit125_wasi_snapshot0.wat")?;
    let output =
        run_wasmtime_for_output(&[wasm.path().to_str().unwrap(), "--disable-cache"], None)?;
    if cfg!(windows) {
        assert_eq!(output.status.code().unwrap(), 1);
    } else {
        assert_eq!(output.status.code().unwrap(), 125);
    }
    Ok(())
}

// Exit with a valid non-zero exit code, snapshot1 edition.
#[test]
fn exit125_wasi_snapshot1() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/exit125_wasi_snapshot1.wat")?;
    let output =
        run_wasmtime_for_output(&[wasm.path().to_str().unwrap(), "--disable-cache"], None)?;
    if cfg!(windows) {
        assert_eq!(output.status.code().unwrap(), 1);
    } else {
        assert_eq!(output.status.code().unwrap(), 125);
    }
    Ok(())
}

// Exit with an invalid non-zero exit code, snapshot0 edition.
#[test]
fn exit126_wasi_snapshot0() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/exit126_wasi_snapshot0.wat")?;
    let output =
        run_wasmtime_for_output(&[wasm.path().to_str().unwrap(), "--disable-cache"], None)?;
    assert_eq!(output.status.code().unwrap(), 1);
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid exit status"));
    Ok(())
}

// Exit with an invalid non-zero exit code, snapshot1 edition.
#[test]
fn exit126_wasi_snapshot1() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/exit126_wasi_snapshot1.wat")?;
    let output =
        run_wasmtime_for_output(&[wasm.path().to_str().unwrap(), "--disable-cache"], None)?;
    assert_eq!(output.status.code().unwrap(), 1);
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid exit status"));
    Ok(())
}

// Run a minimal command program.
#[test]
fn minimal_command() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/minimal-command.wat")?;
    let stdout = run_wasmtime(&[wasm.path().to_str().unwrap(), "--disable-cache"])?;
    assert_eq!(stdout, "");
    Ok(())
}

// Run a minimal reactor program.
#[test]
fn minimal_reactor() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/minimal-reactor.wat")?;
    let stdout = run_wasmtime(&[wasm.path().to_str().unwrap(), "--disable-cache"])?;
    assert_eq!(stdout, "");
    Ok(())
}

// Attempt to call invoke on a command.
#[test]
fn command_invoke() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/minimal-command.wat")?;
    run_wasmtime(&[
        "run",
        wasm.path().to_str().unwrap(),
        "--invoke",
        "_start",
        "--disable-cache",
    ])?;
    Ok(())
}

// Attempt to call invoke on a command.
#[test]
fn reactor_invoke() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/minimal-reactor.wat")?;
    run_wasmtime(&[
        "run",
        wasm.path().to_str().unwrap(),
        "--invoke",
        "_initialize",
        "--disable-cache",
    ])?;
    Ok(())
}

// Run the greeter test, which runs a preloaded reactor and a command.
#[test]
fn greeter() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/greeter_command.wat")?;
    let stdout = run_wasmtime(&[
        "run",
        wasm.path().to_str().unwrap(),
        "--disable-cache",
        "--preload",
        "reactor=tests/all/cli_tests/greeter_reactor.wat",
    ])?;
    assert_eq!(
        stdout,
        "Hello _initialize\nHello _start\nHello greet\nHello done\n"
    );
    Ok(())
}

// Run the greeter test, but this time preload a command.
#[test]
fn greeter_preload_command() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/greeter_reactor.wat")?;
    let stdout = run_wasmtime(&[
        "run",
        wasm.path().to_str().unwrap(),
        "--disable-cache",
        "--preload",
        "reactor=tests/all/cli_tests/hello_wasi_snapshot1.wat",
    ])?;
    assert_eq!(stdout, "Hello _initialize\n");
    Ok(())
}

// Run the greeter test, which runs a preloaded reactor and a command.
#[test]
fn greeter_preload_callable_command() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/greeter_command.wat")?;
    let stdout = run_wasmtime(&[
        "run",
        wasm.path().to_str().unwrap(),
        "--disable-cache",
        "--preload",
        "reactor=tests/all/cli_tests/greeter_callable_command.wat",
    ])?;
    assert_eq!(stdout, "Hello _start\nHello callable greet\nHello done\n");
    Ok(())
}

// Ensure successful WASI exit call with FPR saving frames on stack for Windows x64
// See https://github.com/bytecodealliance/wasmtime/issues/1967
#[test]
fn exit_with_saved_fprs() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/exit_with_saved_fprs.wat")?;
    let output =
        run_wasmtime_for_output(&[wasm.path().to_str().unwrap(), "--disable-cache"], None)?;
    assert_eq!(output.status.code().unwrap(), 0);
    assert!(output.stdout.is_empty());
    Ok(())
}

#[test]
fn run_cwasm() -> Result<()> {
    let td = TempDir::new()?;
    let cwasm = td.path().join("foo.cwasm");
    let stdout = run_wasmtime(&[
        "compile",
        "tests/all/cli_tests/simple.wat",
        "-o",
        cwasm.to_str().unwrap(),
    ])?;
    assert_eq!(stdout, "");
    let stdout = run_wasmtime(&["run", "--allow-precompiled", cwasm.to_str().unwrap()])?;
    assert_eq!(stdout, "");
    Ok(())
}

#[cfg(unix)]
#[test]
fn hello_wasi_snapshot0_from_stdin() -> Result<()> {
    // Run a simple WASI hello world, snapshot0 edition.
    // The module is piped from standard input.
    let wasm = build_wasm("tests/all/cli_tests/hello_wasi_snapshot0.wat")?;
    let stdout = {
        let path = wasm.path();
        let args: &[&str] = &["-", "--disable-cache"];
        let output = run_wasmtime_for_output(args, Some(path))?;
        if !output.status.success() {
            bail!(
                "Failed to execute wasmtime with: {:?}\n{}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok::<_, anyhow::Error>(String::from_utf8(output.stdout).unwrap())
    }?;
    assert_eq!(stdout, "Hello, world!\n");
    Ok(())
}

#[cfg(unix)]
#[test]
fn run_cwasm_from_stdin() -> Result<()> {
    let td = TempDir::new()?;
    let cwasm = td.path().join("foo.cwasm");
    let stdout = run_wasmtime(&[
        "compile",
        "tests/all/cli_tests/simple.wat",
        "-o",
        cwasm.to_str().unwrap(),
    ])?;
    assert_eq!(stdout, "");
    let args: &[&str] = &["run", "--allow-precompiled", "-"];
    let output = run_wasmtime_for_output(args, Some(&cwasm))?;
    if output.status.success() {
        bail!("wasmtime should fail loading precompiled modules from piped files, but suceeded");
    }
    Ok(())
}

#[cfg(feature = "wasi-threads")]
#[test]
fn run_threads() -> Result<()> {
    let wasm = build_wasm("tests/all/cli_tests/threads.wat")?;
    let stdout = run_wasmtime(&[
        "run",
        "--wasi-modules",
        "experimental-wasi-threads",
        "--wasm-features",
        "threads",
        "--disable-cache",
        wasm.path().to_str().unwrap(),
    ])?;

    assert!(
        stdout
            == "Called _start\n\
    Running wasi_thread_start\n\
    Running wasi_thread_start\n\
    Running wasi_thread_start\n\
    Done\n"
    );
    Ok(())
}
