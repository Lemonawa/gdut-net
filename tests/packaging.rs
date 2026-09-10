use gdut_net::packaging::pack_into_file;
use gdut_net::payload::unpack;
use std::fs;

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

    pack_into_file(&setup, std::slice::from_ref(&exe), &payload, &out).unwrap();

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
