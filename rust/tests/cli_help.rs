use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_tianshell"))
        .args(args)
        .output()
        .expect("启动 tianshell")
}

#[test]
fn help_flag_prints_usage_and_does_not_start_the_workbench() {
    let output = run(&["--help"]);
    assert!(output.status.success(), "status {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("用法"), "{stdout}");
    assert!(stdout.contains("不启动工作台"), "{stdout}");
    assert!(!stdout.contains("工作台已启动"), "{stdout}");
    assert!(!stderr.contains("工作台已启动"), "{stderr}");
}

#[test]
fn short_help_flag_also_exits() {
    let output = run(&["-h"]);
    assert!(output.status.success(), "status {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--port"), "{stdout}");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("工作台已启动"));
}
