//! `cmdline::first_token`：Windows 服务 lpBinaryPathName 的首 token 解析。

use gdut_net::cmdline::first_token;

#[test]
fn quoted_path_with_spaces_and_args() {
    assert_eq!(
        first_token(
            r#""C:\Program Files\gdut-net\gdut-net.exe" --config C:\ProgramData\gdut-net\config.toml run"#
        ),
        r"C:\Program Files\gdut-net\gdut-net.exe"
    );
}

#[test]
fn unquoted_path_without_spaces_and_args() {
    assert_eq!(
        first_token(r"C:\gdut-net\gdut-net.exe --config C:\ProgramData\gdut-net\config.toml run"),
        r"C:\gdut-net\gdut-net.exe"
    );
}

#[test]
fn quoted_path_only() {
    assert_eq!(
        first_token(r#""C:\Program Files\gdut-net\gdut-net.exe""#),
        r"C:\Program Files\gdut-net\gdut-net.exe"
    );
}

#[test]
fn plain_path_without_quotes_or_args() {
    assert_eq!(
        first_token(r"C:\gdut-net\gdut-net.exe"),
        r"C:\gdut-net\gdut-net.exe"
    );
}

#[test]
fn empty_input() {
    assert_eq!(first_token(""), "");
}

#[test]
fn quoted_path_with_quotes_in_args() {
    assert_eq!(
        first_token(r#""C:\a b\gdut-net.exe" --config "C:\c d\config.toml" run"#),
        r"C:\a b\gdut-net.exe"
    );
}
