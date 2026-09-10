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
    assert_eq!(
        parse(&["--uninstall"]).unwrap().mode().unwrap(),
        Mode::Uninstall
    );
    assert_eq!(
        parse(&["--start-service"]).unwrap().mode().unwrap(),
        Mode::StartService
    );
    assert_eq!(
        parse(&["--silent"]).unwrap().mode().unwrap(),
        Mode::SilentInstall
    );
    assert_eq!(
        parse(&["--silent", "--uninstall"]).unwrap().mode().unwrap(),
        Mode::SilentUninstall
    );
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
